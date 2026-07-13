use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Once,
    },
    time::{Instant, SystemTime},
};

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use super::{stt_status, SttConfig, SttSegment, SttState, SttTranscription};
use crate::audio_preprocess::{load_wav_for_whisper, WhisperAudio};

static WHISPER_LOG_HOOKS: Once = Once::new();
const SILENCE_RMS_DB: f32 = -50.0;
const VAD_FRAME_SAMPLES: usize = 480; // 30 ms at 16 kHz.
const VAD_SPEECH_DB: f32 = -42.0;
const VAD_MIN_SPEECH_FRAMES: usize = 4;

#[derive(Clone, Default)]
pub(crate) struct SttRuntime {
    inner: Arc<SttRuntimeInner>,
}

#[derive(Default)]
struct SttRuntimeInner {
    engine: Mutex<WhisperEngine>,
    shutting_down: AtomicBool,
}

impl SttRuntime {
    pub(crate) async fn transcribe(
        &self,
        config: SttConfig,
        audio_path: PathBuf,
    ) -> Result<SttTranscription, String> {
        if self.inner.shutting_down.load(Ordering::Acquire) {
            return Err("Whisper is shutting down".to_string());
        }

        let inner = self.inner.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let mut engine = inner
                .engine
                .lock()
                .map_err(|_| "Whisper engine lock was poisoned".to_string())?;
            if inner.shutting_down.load(Ordering::Acquire) {
                return Err("Whisper is shutting down".to_string());
            }
            engine.transcribe(config, audio_path)
        })
        .await
        .map_err(|error| format!("STT worker failed: {error}"))?
    }

    /// Tauri exits via `std::process::exit`, which skips Rust destructors. The
    /// Whisper context must therefore be released explicitly before ggml's
    /// Metal global state is torn down.
    pub(crate) fn shutdown(&self) {
        self.inner.shutting_down.store(true, Ordering::Release);
        if let Ok(mut engine) = self.inner.engine.lock() {
            engine.loaded = None;
        }
    }
}

#[derive(Default)]
struct WhisperEngine {
    loaded: Option<LoadedWhisperModel>,
}

struct LoadedWhisperModel {
    key: ModelKey,
    context: WhisperContext,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ModelKey {
    path: PathBuf,
    size: u64,
    modified: Option<SystemTime>,
}

impl WhisperEngine {
    fn transcribe(
        &mut self,
        config: SttConfig,
        audio_path: PathBuf,
    ) -> Result<SttTranscription, String> {
        WHISPER_LOG_HOOKS.call_once(whisper_rs::install_logging_hooks);

        let status = stt_status(config.clone());
        if !matches!(status.state, SttState::Ready) {
            return Err(status.message);
        }

        let started_at = Instant::now();
        let preprocessing_started = Instant::now();
        let audio = load_wav_for_whisper(&audio_path)?;
        let preprocessing_ms = preprocessing_started.elapsed().as_millis();
        if audio.samples.is_empty() {
            return Err("Captured audio is empty".to_string());
        }

        if is_silent(&audio) {
            return Ok(empty_transcription(
                config,
                audio,
                started_at.elapsed().as_millis(),
                preprocessing_ms,
            ));
        }

        let model_key = model_key(Path::new(&config.model_path))?;
        let model_load_started = Instant::now();
        let model_reloaded = self
            .loaded
            .as_ref()
            .map(|model| model.key != model_key)
            .unwrap_or(true);
        if model_reloaded {
            self.loaded = None;
            let context = WhisperContext::new_with_params(
                &config.model_path,
                WhisperContextParameters::default(),
            )
            .map_err(|error| format!("Failed to load Whisper model: {error}"))?;
            self.loaded = Some(LoadedWhisperModel {
                key: model_key,
                context,
            });
        }
        let model_load_ms = if model_reloaded {
            model_load_started.elapsed().as_millis()
        } else {
            0
        };

        let context = &self
            .loaded
            .as_ref()
            .ok_or_else(|| "Whisper model was not loaded".to_string())?
            .context;
        let mut state = context
            .create_state()
            .map_err(|error| format!("Failed to create Whisper state: {error}"))?;
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(config.threads as i32);
        params.set_language(config.language.as_deref());
        params.set_detect_language(config.language.is_none());
        params.set_no_context(true);
        params.set_temperature(0.0);
        params.set_no_speech_thold(0.6);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        let inference_started = Instant::now();
        state
            .full(params, &audio.samples)
            .map_err(|error| format!("Failed to run Whisper transcription: {error}"))?;
        let inference_ms = inference_started.elapsed().as_millis();

        let mut segments = Vec::new();
        let raw_segment_count = state.full_n_segments();
        let language_id = state.full_lang_id_from_state();
        for segment_index in 0..raw_segment_count {
            let Some(segment) = state.get_segment(segment_index) else {
                continue;
            };
            let text = segment
                .to_str_lossy()
                .map_err(|error| format!("Failed to read Whisper segment: {error}"))?
                .trim()
                .to_string();
            if text.is_empty() || is_noise_segment(&text) {
                continue;
            }
            segments.push(SttSegment {
                text,
                start_ms: segment.start_timestamp() * 10,
                end_ms: segment.end_timestamp() * 10,
            });
        }
        let text = segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_string();
        let real_time_factor = if audio.info.duration_ms == 0 {
            0.0
        } else {
            inference_ms as f64 / audio.info.duration_ms as f64
        };

        Ok(SttTranscription {
            text,
            segments,
            raw_segment_count,
            language_id,
            language: config.language,
            audio: audio.info,
            elapsed_ms: started_at.elapsed().as_millis(),
            preprocessing_ms,
            model_load_ms,
            inference_ms,
            real_time_factor,
            model_reloaded,
            model_path: config.model_path,
        })
    }
}

fn model_key(path: &Path) -> Result<ModelKey, String> {
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("Failed to resolve Whisper model: {error}"))?;
    let metadata = fs::metadata(&canonical)
        .map_err(|error| format!("Failed to inspect Whisper model: {error}"))?;
    Ok(ModelKey {
        path: canonical,
        size: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

fn is_silent(audio: &WhisperAudio) -> bool {
    if audio
        .info
        .rms_db
        .map(|db| db < SILENCE_RMS_DB)
        .unwrap_or(false)
    {
        return true;
    }

    // A short energy VAD avoids invoking Whisper for chunks containing only
    // incidental clicks or capture noise. It does not trim audio, preserving
    // segment timestamps for diarization.
    let speech_frames = audio
        .samples
        .chunks(VAD_FRAME_SAMPLES)
        .filter(|frame| frame_db(frame).is_some_and(|db| db >= VAD_SPEECH_DB))
        .count();
    speech_frames < VAD_MIN_SPEECH_FRAMES
}

fn frame_db(samples: &[f32]) -> Option<f32> {
    if samples.is_empty() {
        return None;
    }
    let mean_square =
        samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32;
    if mean_square <= f32::EPSILON {
        None
    } else {
        Some(20.0 * mean_square.sqrt().log10())
    }
}

fn empty_transcription(
    config: SttConfig,
    audio: WhisperAudio,
    elapsed_ms: u128,
    preprocessing_ms: u128,
) -> SttTranscription {
    SttTranscription {
        text: String::new(),
        segments: Vec::new(),
        raw_segment_count: 0,
        language_id: -1,
        language: config.language,
        audio: audio.info,
        elapsed_ms,
        preprocessing_ms,
        model_load_ms: 0,
        inference_ms: 0,
        real_time_factor: 0.0,
        model_reloaded: false,
        model_path: config.model_path,
    }
}

fn is_noise_segment(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return true;
    }
    let stripped = trimmed.trim_start_matches('>').trim();
    if stripped.is_empty() {
        return true;
    }
    if (stripped.starts_with('[') && stripped.ends_with(']'))
        || (stripped.starts_with('(') && stripped.ends_with(')'))
    {
        return true;
    }
    let normalized = stripped
        .chars()
        .map(|character| {
            if character.is_alphanumeric() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    const NOISE: &[&str] = &[
        "you",
        "thank you",
        "thank you for watching",
        "thanks for watching",
        "silence",
        "music",
        "applause",
        "inaudible",
        "blank audio",
        "muffled speaking",
        "no speech",
        "uh",
        "um",
        "uh huh",
        "mm",
        "mm hmm",
        "hmm",
    ];
    normalized.is_empty() || NOISE.contains(&normalized.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn energy_vad_rejects_silence_and_accepts_sustained_audio() {
        assert_eq!(frame_db(&vec![0.0; VAD_FRAME_SAMPLES]), None);
        assert!(frame_db(&vec![0.1; VAD_FRAME_SAMPLES]).is_some_and(|db| db > -42.0));
    }

    #[test]
    fn filters_common_whisper_noise_segments() {
        assert!(is_noise_segment("[Silence]"));
        assert!(is_noise_segment("Thank you for watching."));
        assert!(!is_noise_segment("We agreed to send the proposal."));
    }
}
