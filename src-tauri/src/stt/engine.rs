use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Once,
    },
    time::{Instant, SystemTime},
};

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use super::{stt_status, SttConfig, SttSegment, SttState, SttTranscription, TranscriptionSession};
use crate::audio_preprocess::{load_wav_for_whisper, WhisperAudio};

static WHISPER_LOG_HOOKS: Once = Once::new();
const SILENCE_RMS_DB: f32 = -50.0;
const VAD_FRAME_SAMPLES: usize = 480; // 30 ms at 16 kHz.
const VAD_SPEECH_DB: f32 = -42.0;
const VAD_MIN_SPEECH_FRAMES: usize = 4;
const CHUNK_OVERLAP_MS: usize = 750;
const CONTEXT_PROMPT_CHARS: usize = 600;
const SESSION_IDLE_TTL: std::time::Duration = std::time::Duration::from_secs(2 * 60 * 60);

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
        session: Option<TranscriptionSession>,
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
            engine.transcribe(config, audio_path, session)
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
            engine.sessions.clear();
        }
    }
}

#[derive(Default)]
struct WhisperEngine {
    loaded: Option<LoadedWhisperModel>,
    sessions: HashMap<String, WhisperSession>,
}

struct WhisperSession {
    last_sequence: i64,
    prompt: String,
    audio_tail: Vec<f32>,
    last_used: Instant,
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
        session: Option<TranscriptionSession>,
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
            if let Some(session) = &session {
                // A full quiet chunk is an utterance boundary. Do not carry old
                // audio or decoder prompting across it, as that encourages
                // Whisper to repeat the previous sentence into silence.
                self.sessions.remove(&session.key);
            }
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
            self.sessions.clear();
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

        self.sessions
            .retain(|_, value| value.last_used.elapsed() < SESSION_IDLE_TTL);
        let previous = session.as_ref().and_then(|requested| {
            self.sessions
                .get(&requested.key)
                .filter(|stored| requested.sequence > stored.last_sequence)
                .map(|stored| (stored.prompt.clone(), stored.audio_tail.clone()))
        });
        let (previous_prompt, previous_tail) = previous.unwrap_or_default();
        let overlap_samples = previous_tail.len();
        let overlap_ms =
            (overlap_samples * 1000 / crate::audio_preprocess::WHISPER_SAMPLE_RATE as usize) as i64;
        let inference_samples = if previous_tail.is_empty() {
            audio.samples.clone()
        } else {
            let mut samples = Vec::with_capacity(previous_tail.len() + audio.samples.len());
            samples.extend_from_slice(&previous_tail);
            samples.extend_from_slice(&audio.samples);
            samples
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
        params.set_no_context(false);
        if !previous_prompt.is_empty() {
            params.set_initial_prompt(&previous_prompt);
        }
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
            .full(params, &inference_samples)
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
            let start_ms = segment.start_timestamp() * 10 - overlap_ms;
            let end_ms = segment.end_timestamp() * 10 - overlap_ms;
            if end_ms <= 0 {
                continue;
            }
            segments.push(SttSegment {
                text,
                start_ms: start_ms.max(0),
                end_ms: end_ms.max(0),
            });
        }
        remove_repeated_prefix(&previous_prompt, &mut segments);
        let text = segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_string();

        if let Some(requested) = session {
            let overlap_samples =
                (crate::audio_preprocess::WHISPER_SAMPLE_RATE as usize * CHUNK_OVERLAP_MS) / 1000;
            let tail_start = audio.samples.len().saturating_sub(overlap_samples);
            let prompt = append_prompt(&previous_prompt, &text);
            self.sessions.insert(
                requested.key,
                WhisperSession {
                    last_sequence: requested.sequence,
                    prompt,
                    audio_tail: audio.samples[tail_start..].to_vec(),
                    last_used: Instant::now(),
                },
            );
        }
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

fn append_prompt(previous: &str, current: &str) -> String {
    let combined = match (previous.trim(), current.trim()) {
        ("", current) => current.to_string(),
        (previous, "") => previous.to_string(),
        (previous, current) => format!("{previous} {current}"),
    };
    if combined.chars().count() <= CONTEXT_PROMPT_CHARS {
        return combined;
    }
    combined
        .chars()
        .rev()
        .take(CONTEXT_PROMPT_CHARS)
        .collect::<String>()
        .chars()
        .rev()
        .collect()
}

fn normalized_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|character| character.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|word| !word.is_empty())
        .collect()
}

fn repeated_prefix_word_count(previous: &str, current: &str) -> usize {
    let previous = normalized_words(previous);
    let current = normalized_words(current);
    let max_overlap = previous.len().min(current.len()).min(24);
    (2..=max_overlap)
        .rev()
        .find(|count| previous[previous.len() - count..] == current[..*count])
        .unwrap_or(0)
}

fn remove_repeated_prefix(previous: &str, segments: &mut Vec<SttSegment>) {
    if previous.is_empty() || segments.is_empty() {
        return;
    }
    let current = segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let mut words_to_remove = repeated_prefix_word_count(previous, &current);
    if words_to_remove == 0 {
        return;
    }

    for segment in segments.iter_mut() {
        if words_to_remove == 0 {
            break;
        }
        let words = segment.text.split_whitespace().collect::<Vec<_>>();
        let removable = words_to_remove.min(words.len());
        segment.text = words[removable..].join(" ");
        words_to_remove -= removable;
    }
    segments.retain(|segment| !segment.text.trim().is_empty());
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

    fn test_audio(samples: Vec<f32>, rms_db: Option<f32>) -> WhisperAudio {
        let sample_count = samples.len();
        WhisperAudio {
            samples,
            info: crate::audio_preprocess::WhisperAudioInfo {
                source_sample_rate: crate::audio_preprocess::WHISPER_SAMPLE_RATE,
                source_channels: 1,
                sample_rate: crate::audio_preprocess::WHISPER_SAMPLE_RATE,
                channels: 1,
                duration_ms: sample_count as u128 * 1000
                    / crate::audio_preprocess::WHISPER_SAMPLE_RATE as u128,
                samples: sample_count,
                rms_db,
                peak_db: None,
            },
        }
    }

    #[test]
    fn energy_vad_rejects_silence_and_accepts_sustained_audio() {
        assert_eq!(frame_db(&vec![0.0; VAD_FRAME_SAMPLES]), None);
        assert!(frame_db(&vec![0.1; VAD_FRAME_SAMPLES]).is_some_and(|db| db > -42.0));

        assert!(is_silent(&test_audio(
            vec![0.0; VAD_FRAME_SAMPLES * 20],
            None,
        )));
        assert!(is_silent(&test_audio(
            vec![0.1; VAD_FRAME_SAMPLES * (VAD_MIN_SPEECH_FRAMES - 1)],
            Some(-20.0),
        )));
        assert!(!is_silent(&test_audio(
            vec![0.1; VAD_FRAME_SAMPLES * VAD_MIN_SPEECH_FRAMES],
            Some(-20.0),
        )));
    }

    #[test]
    fn removes_words_repeated_by_the_audio_overlap() {
        let mut segments = vec![
            SttSegment {
                text: "à Goa depuis 2011".to_string(),
                start_ms: 0,
                end_ms: 500,
            },
            SttSegment {
                text: "et je suis très content ici".to_string(),
                start_ms: 500,
                end_ms: 2_000,
            },
        ];

        remove_repeated_prefix(
            "Je suis indien et j'habite à Goa depuis 2011.",
            &mut segments,
        );

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "et je suis très content ici");
    }

    #[test]
    fn keeps_a_single_repeated_word_to_avoid_eating_real_speech() {
        let mut segments = vec![SttSegment {
            text: "Voilà une nouvelle idée".to_string(),
            start_ms: 0,
            end_ms: 1_000,
        }];

        remove_repeated_prefix("Et voilà", &mut segments);

        assert_eq!(segments[0].text, "Voilà une nouvelle idée");
    }

    #[test]
    fn filters_common_whisper_noise_segments() {
        assert!(is_noise_segment("[Silence]"));
        assert!(is_noise_segment("Thank you for watching."));
        assert!(!is_noise_segment("We agreed to send the proposal."));
    }
}
