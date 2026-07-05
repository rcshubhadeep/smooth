use crate::{
    app_data_dir, app_meta_value,
    audio_capture::{current_audio_capture_status, AudioCaptureState},
    audio_preprocess::{load_wav_for_whisper, WhisperAudioInfo},
    db_error, open_database,
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf, sync::Once, time::Instant};
use tauri::{AppHandle, State};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

static WHISPER_LOG_HOOKS: Once = Once::new();

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SttConfig {
    pub model_path: String,
    pub language: Option<String>,
    pub threads: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SttState {
    NotConfigured,
    Ready,
    Error,
}

#[derive(Clone, Debug, Serialize)]
pub struct SttStatus {
    pub state: SttState,
    pub message: String,
    pub model_path: String,
    pub language: Option<String>,
    pub threads: u32,
    pub acceleration: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SttSegment {
    pub text: String,
    pub start_ms: i64,
    pub end_ms: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct SttTranscription {
    pub text: String,
    pub segments: Vec<SttSegment>,
    pub raw_segment_count: i32,
    pub language_id: i32,
    pub language: Option<String>,
    pub audio: WhisperAudioInfo,
    pub elapsed_ms: u128,
    pub model_path: String,
}

#[tauri::command]
pub fn get_stt_config(app: AppHandle) -> Result<SttConfig, String> {
    let connection = open_database(&app)?;
    load_stt_config(&app, &connection)
}

#[tauri::command]
pub fn save_stt_config(app: AppHandle, config: SttConfig) -> Result<SttConfig, String> {
    let config = normalize_stt_config(&app, config)?;
    let mut connection = open_database(&app)?;
    let transaction = connection.transaction().map_err(db_error)?;

    let language_value = config.language.as_deref().unwrap_or("auto");
    transaction
        .execute(
            "
            INSERT INTO app_meta (key, value)
            VALUES ('stt_model_path', ?1)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value
            ",
            params![config.model_path],
        )
        .map_err(db_error)?;
    transaction
        .execute(
            "
            INSERT INTO app_meta (key, value)
            VALUES ('stt_language', ?1)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value
            ",
            params![language_value],
        )
        .map_err(db_error)?;
    transaction
        .execute(
            "
            INSERT INTO app_meta (key, value)
            VALUES ('stt_threads', ?1)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value
            ",
            params![config.threads.to_string()],
        )
        .map_err(db_error)?;
    transaction.commit().map_err(db_error)?;

    Ok(config)
}

#[tauri::command]
pub fn get_stt_status(app: AppHandle) -> Result<SttStatus, String> {
    let connection = open_database(&app)?;
    let config = load_stt_config(&app, &connection)?;
    Ok(stt_status(config))
}

#[tauri::command]
pub async fn transcribe_last_capture(
    app: AppHandle,
    audio_state: State<'_, AudioCaptureState>,
) -> Result<SttTranscription, String> {
    let status = current_audio_capture_status(&audio_state)?;
    if status.is_recording {
        return Err("Stop audio capture before transcribing".to_string());
    }

    let preview = status
        .last_preview
        .ok_or_else(|| "No audio capture is available to transcribe".to_string())?;
    let audio_path = PathBuf::from(preview.path);
    let config = {
        let connection = open_database(&app)?;
        load_stt_config(&app, &connection)?
    };

    tauri::async_runtime::spawn_blocking(move || transcribe_wav(config, audio_path))
        .await
        .map_err(|error| format!("STT worker failed: {error}"))?
}

#[tauri::command]
pub async fn transcribe_capture_file(
    app: AppHandle,
    path: String,
) -> Result<SttTranscription, String> {
    let audio_path = validate_capture_audio_path(&app, path)?;
    let config = {
        let connection = open_database(&app)?;
        load_stt_config(&app, &connection)?
    };

    tauri::async_runtime::spawn_blocking(move || transcribe_wav(config, audio_path))
        .await
        .map_err(|error| format!("STT worker failed: {error}"))?
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

fn load_stt_config(
    app: &AppHandle,
    connection: &rusqlite::Connection,
) -> Result<SttConfig, String> {
    let model_path = app_meta_value(connection, "stt_model_path")?
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default_model_path(app).to_string_lossy().into_owned());
    let language = match app_meta_value(connection, "stt_language")? {
        Some(value) if value.trim().eq_ignore_ascii_case("auto") => None,
        Some(value) if !value.trim().is_empty() => Some(value.trim().to_string()),
        _ => Some("en".to_string()),
    };
    let threads = app_meta_value(connection, "stt_threads")?
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|threads| *threads > 0)
        .unwrap_or_else(default_thread_count);

    normalize_stt_config(
        app,
        SttConfig {
            model_path,
            language,
            threads,
        },
    )
}

fn normalize_stt_config(app: &AppHandle, config: SttConfig) -> Result<SttConfig, String> {
    let model_path = if config.model_path.trim().is_empty() {
        default_model_path(app).to_string_lossy().into_owned()
    } else {
        config.model_path.trim().to_string()
    };
    let language = match config.language {
        Some(value) if value.trim().eq_ignore_ascii_case("auto") => None,
        Some(value) if !value.trim().is_empty() => Some(value.trim().to_string()),
        _ => Some("en".to_string()),
    };
    let threads = config.threads.clamp(1, 32);

    Ok(SttConfig {
        model_path,
        language,
        threads,
    })
}

fn default_model_path(app: &AppHandle) -> PathBuf {
    let models_dir = app_data_dir(app)
        .map(|dir| dir.join("models"))
        .unwrap_or_else(|_| PathBuf::from("models"));
    let _ = fs::create_dir_all(&models_dir);
    models_dir.join("ggml-base.en.bin")
}

fn default_thread_count() -> u32 {
    std::thread::available_parallelism()
        .map(|count| count.get().min(4) as u32)
        .unwrap_or(4)
        .max(1)
}

fn stt_status(config: SttConfig) -> SttStatus {
    let model_path = PathBuf::from(&config.model_path);
    let (state, message) = if !model_path.exists() {
        (
            SttState::NotConfigured,
            "Whisper model file was not found".to_string(),
        )
    } else if !model_path.is_file() {
        (
            SttState::Error,
            "Whisper model path is not a file".to_string(),
        )
    } else {
        (SttState::Ready, "Whisper model is ready".to_string())
    };

    SttStatus {
        state,
        message,
        model_path: config.model_path,
        language: config.language,
        threads: config.threads,
        acceleration: acceleration_features(),
    }
}

fn acceleration_features() -> Vec<String> {
    let mut features = Vec::new();
    if cfg!(feature = "metal") {
        features.push("metal".to_string());
    }
    if cfg!(feature = "coreml") {
        features.push("coreml".to_string());
    }
    if cfg!(feature = "cuda") {
        features.push("cuda".to_string());
    }
    if cfg!(feature = "hipblas") {
        features.push("hipblas".to_string());
    }
    if cfg!(feature = "openblas") {
        features.push("openblas".to_string());
    }
    if cfg!(feature = "openmp") {
        features.push("openmp".to_string());
    }
    if cfg!(feature = "vulkan") {
        features.push("vulkan".to_string());
    }
    if features.is_empty() {
        features.push("cpu".to_string());
    }
    features
}

/// Chunks quieter than this (dBFS RMS) are treated as silence and skipped —
/// Whisper hallucinates filler ("you", "[Silence]", "Thank you") on quiet audio.
const SILENCE_RMS_DB: f32 = -50.0;

/// True when a transcribed segment is a Whisper hallucination / non-speech tag
/// rather than real speech.
fn is_noise_segment(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return true;
    }
    // Drop leading ">>" speaker-change markers some models emit.
    let stripped = trimmed.trim_start_matches('>').trim();
    if stripped.is_empty() {
        return true;
    }
    // Bracketed / parenthesized annotations: [silence], (muffled speaking), [music]…
    if (stripped.starts_with('[') && stripped.ends_with(']'))
        || (stripped.starts_with('(') && stripped.ends_with(')'))
    {
        return true;
    }
    // Normalize to lowercase alphanumerics and match a known-hallucination set.
    let normalized = stripped
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if normalized.is_empty() {
        return true;
    }
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
    NOISE.contains(&normalized.as_str())
}

fn transcribe_wav(config: SttConfig, audio_path: PathBuf) -> Result<SttTranscription, String> {
    WHISPER_LOG_HOOKS.call_once(whisper_rs::install_logging_hooks);

    let status = stt_status(config.clone());
    if !matches!(status.state, SttState::Ready) {
        return Err(status.message);
    }

    let audio = load_wav_for_whisper(&audio_path)?;
    if audio.samples.is_empty() {
        return Err("Captured audio is empty".to_string());
    }

    // Silence gate: don't transcribe near-silent chunks (kills silence hallucinations).
    if audio
        .info
        .rms_db
        .map(|db| db < SILENCE_RMS_DB)
        .unwrap_or(false)
    {
        return Ok(SttTranscription {
            text: String::new(),
            segments: Vec::new(),
            raw_segment_count: 0,
            language_id: -1,
            language: config.language,
            audio: audio.info,
            elapsed_ms: 0,
            model_path: config.model_path,
        });
    }

    let started_at = Instant::now();
    let context =
        WhisperContext::new_with_params(&config.model_path, WhisperContextParameters::default())
            .map_err(|error| format!("Failed to load Whisper model: {error}"))?;
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

    state
        .full(params, &audio.samples)
        .map_err(|error| format!("Failed to run Whisper transcription: {error}"))?;

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

    Ok(SttTranscription {
        text,
        segments,
        raw_segment_count,
        language_id,
        language: config.language,
        audio: audio.info,
        elapsed_ms: started_at.elapsed().as_millis(),
        model_path: config.model_path,
    })
}
