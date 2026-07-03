use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    SampleFormat, Stream, StreamConfig,
};
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager, State};

const MAX_CAPTURE_SECONDS: u32 = 300;
const WORKER_REPLY_TIMEOUT: Duration = Duration::from_secs(8);

pub struct AudioCaptureState {
    commands: Sender<AudioCaptureCommand>,
}

impl Default for AudioCaptureState {
    fn default() -> Self {
        let (commands, receiver) = mpsc::channel();
        thread::spawn(move || run_audio_worker(receiver));
        Self { commands }
    }
}

enum AudioCaptureCommand {
    GetStatus {
        reply: Sender<Result<AudioCaptureStatus, String>>,
    },
    Start {
        reply: Sender<Result<AudioCaptureStatus, String>>,
    },
    Stop {
        capture_dir: PathBuf,
        reply: Sender<Result<AudioCaptureStatus, String>>,
    },
}

#[derive(Default)]
struct AudioWorker {
    active: Option<ActiveCapture>,
    last_preview: Option<AudioCapturePreview>,
}

struct ActiveCapture {
    _stream: Stream,
    buffer: SharedCaptureBuffer,
    device_name: String,
    sample_rate: u32,
    channels: u16,
    started_at_ms: u128,
    started_at: Instant,
}

type SharedCaptureBuffer = Arc<Mutex<CaptureBuffer>>;

struct CaptureBuffer {
    samples: Vec<f32>,
    max_samples: usize,
    dropped_samples: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct AudioCaptureStatus {
    is_recording: bool,
    device_name: Option<String>,
    sample_rate: Option<u32>,
    channels: Option<u16>,
    captured_samples: u64,
    dropped_samples: u64,
    elapsed_ms: Option<u128>,
    started_at_ms: Option<u128>,
    last_preview: Option<AudioCapturePreview>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AudioCapturePreview {
    path: String,
    duration_ms: u128,
    sample_rate: u32,
    channels: u16,
    samples: u64,
}

#[tauri::command]
pub fn get_audio_capture_status(
    state: State<'_, AudioCaptureState>,
) -> Result<AudioCaptureStatus, String> {
    request_worker(&state.commands, |reply| AudioCaptureCommand::GetStatus {
        reply,
    })
}

#[tauri::command]
pub fn start_audio_capture(
    state: State<'_, AudioCaptureState>,
) -> Result<AudioCaptureStatus, String> {
    request_worker(&state.commands, |reply| AudioCaptureCommand::Start {
        reply,
    })
}

#[tauri::command]
pub fn stop_audio_capture(
    app: AppHandle,
    state: State<'_, AudioCaptureState>,
) -> Result<AudioCaptureStatus, String> {
    let capture_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("Failed to resolve app data directory: {error}"))?
        .join("audio-captures");

    request_worker(&state.commands, |reply| AudioCaptureCommand::Stop {
        capture_dir,
        reply,
    })
}

fn request_worker(
    commands: &Sender<AudioCaptureCommand>,
    build: impl FnOnce(Sender<Result<AudioCaptureStatus, String>>) -> AudioCaptureCommand,
) -> Result<AudioCaptureStatus, String> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(build(reply))
        .map_err(|_| "audio capture worker is unavailable".to_string())?;
    receiver
        .recv_timeout(WORKER_REPLY_TIMEOUT)
        .map_err(|_| "audio capture worker did not respond".to_string())?
}

fn run_audio_worker(receiver: Receiver<AudioCaptureCommand>) {
    let mut worker = AudioWorker::default();

    while let Ok(command) = receiver.recv() {
        match command {
            AudioCaptureCommand::GetStatus { reply } => {
                let _ = reply.send(Ok(status_from_worker(&worker)));
            }
            AudioCaptureCommand::Start { reply } => {
                let result =
                    start_worker_capture(&mut worker).map(|()| status_from_worker(&worker));
                let _ = reply.send(result);
            }
            AudioCaptureCommand::Stop { capture_dir, reply } => {
                let result = stop_worker_capture(&mut worker, &capture_dir)
                    .map(|()| status_from_worker(&worker));
                let _ = reply.send(result);
            }
        }
    }
}

fn start_worker_capture(worker: &mut AudioWorker) -> Result<(), String> {
    if worker.active.is_some() {
        return Ok(());
    }

    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| "No default microphone input device was found".to_string())?;
    let device_name = device
        .name()
        .unwrap_or_else(|_| "Default microphone".to_string());
    let supported_config = device
        .default_input_config()
        .map_err(|error| format!("Failed to read default microphone config: {error}"))?;
    let sample_format = supported_config.sample_format();
    let config: StreamConfig = supported_config.into();
    let sample_rate = config.sample_rate.0;
    let channels = config.channels;
    let max_samples = sample_rate as usize * channels as usize * MAX_CAPTURE_SECONDS as usize;
    let buffer = Arc::new(Mutex::new(CaptureBuffer {
        samples: Vec::with_capacity(sample_rate as usize * channels as usize * 10),
        max_samples,
        dropped_samples: 0,
    }));

    let stream = build_input_stream(&device, &config, sample_format, Arc::clone(&buffer))?;
    stream
        .play()
        .map_err(|error| format!("Failed to start microphone capture: {error}"))?;

    worker.active = Some(ActiveCapture {
        _stream: stream,
        buffer,
        device_name,
        sample_rate,
        channels,
        started_at_ms: now_ms(),
        started_at: Instant::now(),
    });

    Ok(())
}

fn stop_worker_capture(worker: &mut AudioWorker, capture_dir: &Path) -> Result<(), String> {
    let active = match worker.active.take() {
        Some(active) => active,
        None => return Ok(()),
    };

    let elapsed_ms = active.started_at.elapsed().as_millis();
    let captured = snapshot_capture_buffer(&active.buffer)?;
    let preview = write_preview_wav(
        capture_dir,
        &captured.samples,
        active.sample_rate,
        active.channels,
        elapsed_ms,
    )?;
    worker.last_preview = Some(preview);

    Ok(())
}

fn build_input_stream(
    device: &cpal::Device,
    config: &StreamConfig,
    sample_format: SampleFormat,
    buffer: SharedCaptureBuffer,
) -> Result<Stream, String> {
    let error_handler = |error| eprintln!("audio capture stream error: {error}");

    match sample_format {
        SampleFormat::F32 => device
            .build_input_stream(
                config,
                move |data: &[f32], _| append_samples(&buffer, data.iter().copied()),
                error_handler,
                None,
            )
            .map_err(|error| format!("Failed to build f32 microphone stream: {error}")),
        SampleFormat::I16 => device
            .build_input_stream(
                config,
                move |data: &[i16], _| {
                    append_samples(
                        &buffer,
                        data.iter().map(|sample| *sample as f32 / i16::MAX as f32),
                    )
                },
                error_handler,
                None,
            )
            .map_err(|error| format!("Failed to build i16 microphone stream: {error}")),
        SampleFormat::U16 => device
            .build_input_stream(
                config,
                move |data: &[u16], _| {
                    append_samples(
                        &buffer,
                        data.iter()
                            .map(|sample| (*sample as f32 / u16::MAX as f32) * 2.0 - 1.0),
                    )
                },
                error_handler,
                None,
            )
            .map_err(|error| format!("Failed to build u16 microphone stream: {error}")),
        other => Err(format!("Unsupported microphone sample format: {other:?}")),
    }
}

fn append_samples<I>(buffer: &SharedCaptureBuffer, samples: I)
where
    I: IntoIterator<Item = f32>,
{
    let Ok(mut capture) = buffer.lock() else {
        return;
    };

    for sample in samples {
        if capture.samples.len() < capture.max_samples {
            capture.samples.push(sample);
        } else {
            capture.dropped_samples += 1;
        }
    }
}

fn snapshot_capture_buffer(buffer: &SharedCaptureBuffer) -> Result<CaptureBufferSnapshot, String> {
    let capture = buffer
        .lock()
        .map_err(|_| "audio capture buffer is unavailable")?;
    Ok(CaptureBufferSnapshot {
        samples: capture.samples.clone(),
    })
}

struct CaptureBufferSnapshot {
    samples: Vec<f32>,
}

fn write_preview_wav(
    capture_dir: &Path,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
    duration_ms: u128,
) -> Result<AudioCapturePreview, String> {
    fs::create_dir_all(capture_dir)
        .map_err(|error| format!("Failed to create audio capture directory: {error}"))?;
    let path = capture_dir.join(format!("capture-{}.wav", now_ms()));
    write_i16_wav(&path, samples, sample_rate, channels)?;

    Ok(AudioCapturePreview {
        path: path.to_string_lossy().into_owned(),
        duration_ms,
        sample_rate,
        channels,
        samples: samples.len() as u64,
    })
}

fn write_i16_wav(
    path: &Path,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .map_err(|error| format!("Failed to create WAV preview: {error}"))?;
    for sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        let pcm = (clamped * i16::MAX as f32) as i16;
        writer
            .write_sample(pcm)
            .map_err(|error| format!("Failed to write WAV preview: {error}"))?;
    }
    writer
        .finalize()
        .map_err(|error| format!("Failed to finalize WAV preview: {error}"))
}

fn status_from_worker(worker: &AudioWorker) -> AudioCaptureStatus {
    let Some(active) = &worker.active else {
        return AudioCaptureStatus {
            is_recording: false,
            device_name: None,
            sample_rate: None,
            channels: None,
            captured_samples: 0,
            dropped_samples: 0,
            elapsed_ms: None,
            started_at_ms: None,
            last_preview: worker.last_preview.clone(),
        };
    };

    let (captured_samples, dropped_samples) = match active.buffer.lock() {
        Ok(buffer) => (buffer.samples.len() as u64, buffer.dropped_samples),
        Err(_) => (0, 0),
    };

    AudioCaptureStatus {
        is_recording: true,
        device_name: Some(active.device_name.clone()),
        sample_rate: Some(active.sample_rate),
        channels: Some(active.channels),
        captured_samples,
        dropped_samples,
        elapsed_ms: Some(active.started_at.elapsed().as_millis()),
        started_at_ms: Some(active.started_at_ms),
        last_preview: worker.last_preview.clone(),
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_browser_playable_pcm_wav() {
        let path = std::env::temp_dir().join(format!("smooth-audio-test-{}.wav", now_ms()));
        write_i16_wav(&path, &[0.0, 0.5, -0.5, 1.0, -1.0], 48_000, 1).expect("write wav");

        let reader = hound::WavReader::open(&path).expect("open wav");
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, 48_000);
        assert_eq!(spec.bits_per_sample, 16);
        assert_eq!(reader.duration(), 5);

        let _ = fs::remove_file(path);
    }
}
