use crate::{app_data_dir, audio_preprocess::load_wav_for_whisper};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager, State};

const SESSION_SAMPLE_RATE: u32 = 16_000;
// Keep enough overlap to reconcile Polyvoice's per-run speaker IDs without
// repeatedly analyzing the entire meeting. This also bounds session memory.
const SESSION_CONTEXT_MS: i64 = 30_000;

#[derive(Default)]
pub struct DiarizationState {
    sessions: Mutex<HashMap<String, DiarizationSession>>,
}

struct DiarizationSession {
    path: PathBuf,
    context_samples: Vec<i16>,
    total_samples: usize,
    stable_turns: Vec<DiarizationTurn>,
    next_speaker_index: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DiarizationTurn {
    pub speaker_id: String,
    pub start_ms: i64,
    pub end_ms: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiarizationResult {
    pub turns: Vec<DiarizationTurn>,
    pub engine: String,
    pub session_id: Option<String>,
    pub chunk_start_ms: i64,
    pub chunk_end_ms: i64,
}

#[derive(Clone, Debug, Deserialize)]
struct HelperDiarizeOutput {
    turns: Vec<DiarizationTurn>,
    engine: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiarizationSessionStarted {
    pub session_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiarizationCaptureInput {
    pub path: String,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[tauri::command]
pub fn start_diarization_session(
    app: AppHandle,
    state: State<'_, DiarizationState>,
) -> Result<DiarizationSessionStarted, String> {
    let session_id = format!("diarization-{}", now_ms());
    let session_dir = app_data_dir(&app)?.join("audio-captures");
    fs::create_dir_all(&session_dir)
        .map_err(|error| format!("Failed to create audio capture directory: {error}"))?;
    let path = session_dir.join(format!("{session_id}.wav"));

    let mut sessions = state
        .sessions
        .lock()
        .map_err(|_| "Diarization state is unavailable".to_string())?;
    sessions.insert(
        session_id.clone(),
        DiarizationSession {
            path,
            context_samples: Vec::new(),
            total_samples: 0,
            stable_turns: Vec::new(),
            next_speaker_index: 0,
        },
    );

    Ok(DiarizationSessionStarted { session_id })
}

#[tauri::command]
pub fn stop_diarization_session(
    state: State<'_, DiarizationState>,
    session_id: String,
) -> Result<(), String> {
    let mut sessions = state
        .sessions
        .lock()
        .map_err(|_| "Diarization state is unavailable".to_string())?;
    if let Some(session) = sessions.remove(&session_id) {
        let _ = fs::remove_file(session.path);
    }
    Ok(())
}

#[tauri::command]
pub async fn diarize_capture_file(
    app: AppHandle,
    state: State<'_, DiarizationState>,
    input: DiarizationCaptureInput,
) -> Result<DiarizationResult, String> {
    let audio_path = validate_capture_audio_path(&app, input.path)?;
    let (diarization_path, session_id, window_start_ms, chunk_start_ms, chunk_end_ms) =
        if let Some(session_id) = input.session_id {
            append_to_session(&state, &session_id, &audio_path)?
        } else {
            let duration_ms = wav_duration_ms(&audio_path)?;
            (audio_path, None, 0, 0, duration_ms)
        };
    let helper = diarization_helper_path(&app)?;
    let models_cache = app_data_dir(&app)?.join("models").join("polyvoice");
    fs::create_dir_all(&models_cache)
        .map_err(|error| format!("Failed to create diarization model cache: {error}"))?;

    let raw_turns = tauri::async_runtime::spawn_blocking(move || {
        run_diarization_helper(helper, diarization_path, models_cache)
    })
    .await
    .map_err(|error| format!("Diarization worker failed: {error}"))??;
    let engine = raw_turns.engine;
    let raw_turns = raw_turns
        .turns
        .into_iter()
        .map(|turn| DiarizationTurn {
            speaker_id: turn.speaker_id,
            start_ms: turn.start_ms + window_start_ms,
            end_ms: turn.end_ms + window_start_ms,
        })
        .collect();
    let turns = if let Some(session_id) = session_id.as_deref() {
        reconcile_session_turns(&state, session_id, window_start_ms, raw_turns)?
    } else {
        raw_turns
    };

    Ok(DiarizationResult {
        turns,
        engine,
        session_id,
        chunk_start_ms,
        chunk_end_ms,
    })
}

fn run_diarization_helper(
    helper: PathBuf,
    audio_path: PathBuf,
    models_cache: PathBuf,
) -> Result<HelperDiarizeOutput, String> {
    let output = Command::new(&helper)
        .arg("diarize")
        .arg("--input")
        .arg(&audio_path)
        .arg("--models-cache")
        .arg(&models_cache)
        .arg("--profile")
        .arg("balanced")
        .output()
        .map_err(|error| format!("Failed to run diarization helper: {error}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let message = if !stderr.is_empty() { stderr } else { stdout };
        return Err(if message.is_empty() {
            "Diarization helper failed".to_string()
        } else {
            format!("Diarization helper failed: {message}")
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str::<HelperDiarizeOutput>(stdout.trim())
        .map_err(|error| format!("Failed to parse diarization helper output: {error}"))
}

fn reconcile_session_turns(
    state: &State<'_, DiarizationState>,
    session_id: &str,
    window_start_ms: i64,
    raw_turns: Vec<DiarizationTurn>,
) -> Result<Vec<DiarizationTurn>, String> {
    let mut sessions = state
        .sessions
        .lock()
        .map_err(|_| "Diarization state is unavailable".to_string())?;
    let session = sessions
        .get_mut(session_id)
        .ok_or_else(|| "Diarization session was not found".to_string())?;

    let mut raw_speakers = Vec::<String>::new();
    for turn in &raw_turns {
        if !raw_speakers.contains(&turn.speaker_id) {
            raw_speakers.push(turn.speaker_id.clone());
        }
    }

    let mut mapping = HashMap::<String, String>::new();
    let mut claimed_stable_speakers = Vec::<String>::new();
    for raw_speaker in raw_speakers {
        let mut best_stable_speaker: Option<String> = None;
        let mut best_overlap = 0_i64;
        for previous in &session.stable_turns {
            if claimed_stable_speakers.contains(&previous.speaker_id) {
                continue;
            }
            let overlap = overlap_for_speakers(
                &raw_turns,
                &raw_speaker,
                &session.stable_turns,
                &previous.speaker_id,
            );
            if overlap > best_overlap {
                best_overlap = overlap;
                best_stable_speaker = Some(previous.speaker_id.clone());
            }
        }

        let stable_speaker = if best_overlap > 0 {
            best_stable_speaker.expect("best speaker exists when overlap is positive")
        } else {
            let speaker = format!("speaker_{}", session.next_speaker_index);
            session.next_speaker_index += 1;
            speaker
        };
        claimed_stable_speakers.push(stable_speaker.clone());
        mapping.insert(raw_speaker, stable_speaker);
    }

    let stable_turns = raw_turns
        .into_iter()
        .filter_map(|turn| {
            mapping
                .get(&turn.speaker_id)
                .map(|stable_speaker| DiarizationTurn {
                    speaker_id: stable_speaker.clone(),
                    start_ms: turn.start_ms,
                    end_ms: turn.end_ms,
                })
        })
        .collect::<Vec<_>>();
    let mut history = session
        .stable_turns
        .iter()
        .filter_map(|turn| {
            if turn.start_ms >= window_start_ms {
                return None;
            }
            let mut turn = turn.clone();
            turn.end_ms = turn.end_ms.min(window_start_ms);
            (turn.end_ms > turn.start_ms).then_some(turn)
        })
        .collect::<Vec<_>>();
    history.extend(stable_turns.iter().cloned());
    session.stable_turns = history;

    Ok(stable_turns)
}

fn overlap_for_speakers(
    current_turns: &[DiarizationTurn],
    current_speaker: &str,
    previous_turns: &[DiarizationTurn],
    previous_speaker: &str,
) -> i64 {
    current_turns
        .iter()
        .filter(|turn| turn.speaker_id == current_speaker)
        .map(|current| {
            previous_turns
                .iter()
                .filter(|turn| turn.speaker_id == previous_speaker)
                .map(|previous| {
                    (current.end_ms.min(previous.end_ms) - current.start_ms.max(previous.start_ms))
                        .max(0)
                })
                .sum::<i64>()
        })
        .sum()
}

fn append_to_session(
    state: &State<'_, DiarizationState>,
    session_id: &str,
    audio_path: &Path,
) -> Result<(PathBuf, Option<String>, i64, i64, i64), String> {
    let audio = load_wav_for_whisper(audio_path)?;
    let chunk_samples = audio
        .samples
        .iter()
        .map(|sample| (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
        .collect::<Vec<_>>();
    let chunk_duration_ms = duration_ms_for_samples(chunk_samples.len());

    let mut sessions = state
        .sessions
        .lock()
        .map_err(|_| "Diarization state is unavailable".to_string())?;
    let session = sessions
        .get_mut(session_id)
        .ok_or_else(|| "Diarization session was not found".to_string())?;
    let chunk_start_ms = duration_ms_for_samples(session.total_samples);
    session.total_samples += chunk_samples.len();
    session.context_samples.extend(chunk_samples);

    let max_context_samples = samples_for_duration_ms(SESSION_CONTEXT_MS);
    if session.context_samples.len() > max_context_samples {
        let excess = session.context_samples.len() - max_context_samples;
        session.context_samples.drain(..excess);
    }

    let window_start_samples = session.total_samples - session.context_samples.len();
    let window_start_ms = duration_ms_for_samples(window_start_samples);
    let chunk_end_ms =
        duration_ms_for_samples(session.total_samples).max(chunk_start_ms + chunk_duration_ms);
    write_session_wav(&session.path, &session.context_samples)?;

    Ok((
        session.path.clone(),
        Some(session_id.to_string()),
        window_start_ms,
        chunk_start_ms,
        chunk_end_ms,
    ))
}

fn write_session_wav(path: &Path, samples: &[i16]) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SESSION_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .map_err(|error| format!("Failed to create diarization session WAV: {error}"))?;
    for sample in samples {
        writer
            .write_sample(*sample)
            .map_err(|error| format!("Failed to write diarization session WAV: {error}"))?;
    }
    writer
        .finalize()
        .map_err(|error| format!("Failed to finalize diarization session WAV: {error}"))
}

fn wav_duration_ms(path: &Path) -> Result<i64, String> {
    let reader =
        hound::WavReader::open(path).map_err(|error| format!("Failed to open WAV: {error}"))?;
    let spec = reader.spec();
    let channels = spec.channels.max(1) as u64;
    let frames = reader.duration() as u64 / channels;
    Ok(((frames * 1000) / spec.sample_rate.max(1) as u64) as i64)
}

fn duration_ms_for_samples(samples: usize) -> i64 {
    ((samples as u128 * 1000) / SESSION_SAMPLE_RATE as u128) as i64
}

fn samples_for_duration_ms(duration_ms: i64) -> usize {
    ((duration_ms.max(0) as u128 * SESSION_SAMPLE_RATE as u128) / 1000) as usize
}

fn validate_capture_audio_path(app: &AppHandle, path: String) -> Result<PathBuf, String> {
    let requested = PathBuf::from(path);
    let capture_dir = app_data_dir(app)?.join("audio-captures");
    let canonical_dir = capture_dir
        .canonicalize()
        .map_err(|error| format!("Failed to resolve audio capture directory: {error}"))?;
    let canonical_path = requested
        .canonicalize()
        .map_err(|error| format!("Failed to resolve audio capture file: {error}"))?;
    if !canonical_path.starts_with(&canonical_dir) {
        return Err("Audio capture file is outside the capture directory".to_string());
    }
    if canonical_path.extension().and_then(|value| value.to_str()) != Some("wav") {
        return Err("Audio capture file must be a WAV file".to_string());
    }
    Ok(canonical_path)
}

fn diarization_helper_path(app: &AppHandle) -> Result<PathBuf, String> {
    let name = helper_binary_name();

    if let Ok(resource_dir) = app.path().resource_dir() {
        for candidate in [
            resource_dir.join(&name),
            resource_dir.join("binaries").join(&name),
        ] {
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }

    let dev_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("binaries")
        .join(&name);
    if dev_path.exists() {
        return Ok(dev_path);
    }

    Err(format!(
        "Diarization helper binary was not found. Expected {}",
        dev_path.display()
    ))
}

fn helper_binary_name() -> String {
    format!("smooth-diarize-{}", target_triple())
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn target_triple() -> &'static str {
    "aarch64-apple-darwin"
}

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
fn target_triple() -> &'static str {
    "x86_64-apple-darwin"
}

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
fn target_triple() -> &'static str {
    "aarch64-unknown-linux-gnu"
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn target_triple() -> &'static str {
    "x86_64-unknown-linux-gnu"
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn target_triple() -> &'static str {
    "x86_64-pc-windows-msvc"
}

#[cfg(all(target_os = "windows", target_arch = "aarch64"))]
fn target_triple() -> &'static str {
    "aarch64-pc-windows-msvc"
}

#[cfg(not(any(
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "linux", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "windows", target_arch = "x86_64"),
    all(target_os = "windows", target_arch = "aarch64")
)))]
fn target_triple() -> &'static str {
    "unsupported"
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_speaker_overlap_across_diarization_runs() {
        let current = vec![
            DiarizationTurn {
                speaker_id: "raw_a".to_string(),
                start_ms: 0,
                end_ms: 1000,
            },
            DiarizationTurn {
                speaker_id: "raw_b".to_string(),
                start_ms: 1000,
                end_ms: 2000,
            },
        ];
        let previous = vec![DiarizationTurn {
            speaker_id: "speaker_0".to_string(),
            start_ms: 250,
            end_ms: 1250,
        }];

        assert_eq!(
            overlap_for_speakers(&current, "raw_a", &previous, "speaker_0"),
            750
        );
        assert_eq!(
            overlap_for_speakers(&current, "raw_b", &previous, "speaker_0"),
            250
        );
    }

    #[test]
    fn converts_diarization_context_duration_to_samples() {
        assert_eq!(samples_for_duration_ms(SESSION_CONTEXT_MS), 480_000);
        assert_eq!(duration_ms_for_samples(480_000), SESSION_CONTEXT_MS);
    }
}
