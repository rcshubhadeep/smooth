mod engine;

use crate::{
    app_data_dir, app_meta_value,
    audio_capture::{current_audio_capture_status, AudioCaptureState},
    audio_preprocess::WhisperAudioInfo,
    db_error, now_string, open_database,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs, path::PathBuf, time::Duration};
use tauri::{AppHandle, Emitter, Manager, State};

pub(crate) use engine::SttRuntime;

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
    pub preprocessing_ms: u128,
    pub model_load_ms: u128,
    pub inference_ms: u128,
    pub real_time_factor: f64,
    pub model_reloaded: bool,
    pub model_path: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct SttJob {
    pub id: i64,
    pub note_id: String,
    pub source: String,
    pub chunk_path: String,
    pub sequence: i64,
    pub chunk_started_at_ms: Option<i64>,
    pub duration_ms: Option<i64>,
    pub status: String,
    pub attempts: u32,
    pub created_at: String,
    pub queue_wait_ms: Option<u64>,
    pub inference_ms: Option<u64>,
    pub real_time_factor: Option<f64>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SttQueueStatus {
    pub pending_mic: u32,
    pub pending_system: u32,
    pub processing: u32,
    pub failed: u32,
    pub oldest_pending_ms: u64,
    pub recent_average_real_time_factor: Option<f64>,
    pub last_real_time_factor: Option<f64>,
    pub last_inference_ms: Option<u64>,
    pub last_model_load_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnqueueSttJobInput {
    pub note_id: String,
    pub source: String,
    pub path: String,
    pub sequence: i64,
    pub chunk_started_at_ms: Option<i64>,
    pub duration_ms: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SttJobEvent {
    pub job_id: i64,
    pub note_id: String,
    pub source: String,
    pub path: String,
    pub sequence: i64,
    pub chunk_started_at_ms: Option<i64>,
    pub duration_ms: Option<i64>,
    pub transcription: Option<SttTranscription>,
    pub error: Option<String>,
}

pub fn init_schema(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "
            CREATE TABLE IF NOT EXISTS stt_jobs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                note_id TEXT NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
                source TEXT NOT NULL CHECK (source IN ('mic', 'system')),
                chunk_path TEXT NOT NULL,
                sequence INTEGER NOT NULL,
                chunk_started_at_ms INTEGER,
                duration_ms INTEGER,
                status TEXT NOT NULL DEFAULT 'pending'
                    CHECK (status IN ('pending', 'processing', 'done', 'failed')),
                attempts INTEGER NOT NULL DEFAULT 0,
                max_attempts INTEGER NOT NULL DEFAULT 2,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                started_at TEXT,
                completed_at TEXT,
                last_error TEXT
            );

            CREATE INDEX IF NOT EXISTS stt_jobs_claim_idx
                ON stt_jobs(status, created_at, id);
            CREATE INDEX IF NOT EXISTS stt_jobs_note_idx
                ON stt_jobs(note_id, source, sequence);
            ",
        )
        .map_err(db_error)?;
    let columns = table_columns(connection, "stt_jobs")?;
    ensure_column(connection, &columns, "queue_wait_ms", "INTEGER")?;
    ensure_column(connection, &columns, "preprocessing_ms", "INTEGER")?;
    ensure_column(connection, &columns, "model_load_ms", "INTEGER")?;
    ensure_column(connection, &columns, "inference_ms", "INTEGER")?;
    ensure_column(connection, &columns, "real_time_factor", "REAL")?;
    connection
        .execute_batch(
            "CREATE INDEX IF NOT EXISTS stt_jobs_source_claim_idx
                ON stt_jobs(status, source, created_at, id);",
        )
        .map_err(db_error)
}

fn table_columns(connection: &Connection, table: &str) -> Result<HashSet<String>, String> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(db_error)?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(db_error)?
        .collect::<Result<HashSet<_>, _>>()
        .map_err(db_error)?;
    Ok(columns)
}

fn ensure_column(
    connection: &Connection,
    columns: &HashSet<String>,
    name: &str,
    sql_type: &str,
) -> Result<(), String> {
    if columns.contains(name) {
        return Ok(());
    }
    connection
        .execute(
            &format!("ALTER TABLE stt_jobs ADD COLUMN {name} {sql_type}"),
            [],
        )
        .map(|_| ())
        .map_err(db_error)
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
pub fn get_stt_queue_status(app: AppHandle) -> Result<SttQueueStatus, String> {
    let connection = open_database(&app)?;
    let (pending_mic, pending_system, processing, failed, oldest_pending): (
        i64,
        i64,
        i64,
        i64,
        Option<String>,
    ) = connection
        .query_row(
            "SELECT
                COALESCE(SUM(CASE WHEN status = 'pending' AND source = 'mic' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status = 'pending' AND source = 'system' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status = 'processing' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END), 0),
                MIN(CASE WHEN status = 'pending' THEN created_at END)
             FROM stt_jobs",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .map_err(db_error)?;
    let recent_average_real_time_factor = connection
        .query_row(
            "SELECT AVG(real_time_factor) FROM (
                SELECT real_time_factor FROM stt_jobs
                WHERE status = 'done' AND inference_ms > 0
                ORDER BY CAST(completed_at AS INTEGER) DESC LIMIT 20
             )",
            [],
            |row| row.get::<_, Option<f64>>(0),
        )
        .map_err(db_error)?;
    let last_metrics = connection
        .query_row(
            "SELECT real_time_factor, inference_ms, model_load_ms
             FROM stt_jobs
             WHERE status = 'done' AND inference_ms > 0
             ORDER BY CAST(completed_at AS INTEGER) DESC LIMIT 1",
            [],
            |row| {
                Ok((
                    row.get::<_, Option<f64>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            },
        )
        .optional()
        .map_err(db_error)?
        .unwrap_or((None, None, None));
    let now = now_string().parse::<u64>().unwrap_or_default();
    let oldest_pending_ms = oldest_pending
        .and_then(|value| value.parse::<u64>().ok())
        .map(|created| now.saturating_sub(created))
        .unwrap_or_default();

    Ok(SttQueueStatus {
        pending_mic: pending_mic as u32,
        pending_system: pending_system as u32,
        processing: processing as u32,
        failed: failed as u32,
        oldest_pending_ms,
        recent_average_real_time_factor,
        last_real_time_factor: last_metrics.0,
        last_inference_ms: last_metrics.1.map(|value| value as u64),
        last_model_load_ms: last_metrics.2.map(|value| value as u64),
    })
}

#[tauri::command]
pub fn enqueue_stt_job(app: AppHandle, input: EnqueueSttJobInput) -> Result<SttJob, String> {
    let source = match input.source.as_str() {
        "mic" | "system" => input.source,
        _ => return Err("Unsupported STT source".to_string()),
    };
    let audio_path = validate_capture_audio_path(&app, input.path)?;
    let chunk_path = audio_path.to_string_lossy().into_owned();
    let now = now_string();
    let connection = open_database(&app)?;
    connection
        .execute(
            "
            INSERT INTO stt_jobs (
                note_id, source, chunk_path, sequence, chunk_started_at_ms,
                duration_ms, status, attempts, max_attempts, created_at, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', 0, 2, ?7, ?7)
            ",
            params![
                input.note_id,
                source,
                chunk_path,
                input.sequence,
                input.chunk_started_at_ms,
                input.duration_ms,
                now
            ],
        )
        .map_err(db_error)?;
    let id = connection.last_insert_rowid();
    load_stt_job(&connection, id)
}

pub fn recover_interrupted_stt_jobs(connection: &Connection) -> Result<(), String> {
    let now = now_string();
    connection
        .execute(
            "
            UPDATE stt_jobs
            SET status = 'pending',
                updated_at = ?1,
                started_at = NULL,
                last_error = NULL
            WHERE status = 'processing'
            ",
            params![now],
        )
        .map_err(db_error)?;
    Ok(())
}

pub async fn stt_worker(app: AppHandle) {
    let runtime = app.state::<SttRuntime>().inner().clone();
    let mut last_source: Option<String> = None;
    loop {
        let preferred_source = match last_source.as_deref() {
            Some("mic") => Some("system"),
            Some("system") => Some("mic"),
            _ => None,
        };
        match claim_stt_job(&app, preferred_source) {
            Ok(Some(job)) => {
                last_source = Some(job.source.clone());
                process_stt_job(app.clone(), runtime.clone(), job).await;
            }
            Ok(None) => tokio::time::sleep(Duration::from_millis(500)).await,
            Err(error) => {
                eprintln!("[smooth:stt-worker] {error}");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}

fn load_stt_job(connection: &Connection, id: i64) -> Result<SttJob, String> {
    connection
        .query_row(
            "
            SELECT id, note_id, source, chunk_path, sequence, chunk_started_at_ms,
                   duration_ms, status, attempts, created_at, queue_wait_ms,
                   inference_ms, real_time_factor, last_error
            FROM stt_jobs
            WHERE id = ?1
            ",
            params![id],
            |row| {
                Ok(SttJob {
                    id: row.get(0)?,
                    note_id: row.get(1)?,
                    source: row.get(2)?,
                    chunk_path: row.get(3)?,
                    sequence: row.get(4)?,
                    chunk_started_at_ms: row.get(5)?,
                    duration_ms: row.get(6)?,
                    status: row.get(7)?,
                    attempts: row.get(8)?,
                    created_at: row.get(9)?,
                    queue_wait_ms: row.get::<_, Option<i64>>(10)?.map(|value| value as u64),
                    inference_ms: row.get::<_, Option<i64>>(11)?.map(|value| value as u64),
                    real_time_factor: row.get(12)?,
                    last_error: row.get(13)?,
                })
            },
        )
        .map_err(db_error)
}

fn claim_stt_job(
    app: &AppHandle,
    preferred_source: Option<&str>,
) -> Result<Option<SttJob>, String> {
    let mut connection = open_database(app)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error)?;
    let job_id = transaction
        .query_row(
            "
            SELECT id
            FROM stt_jobs
            WHERE status = 'pending'
            ORDER BY
                CASE WHEN source = ?1 THEN 0 ELSE 1 END,
                CAST(created_at AS INTEGER) ASC,
                id ASC
            LIMIT 1
            ",
            params![preferred_source],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(db_error)?;
    let Some(job_id) = job_id else {
        transaction.commit().map_err(db_error)?;
        return Ok(None);
    };

    let now = now_string();
    let created_at = transaction
        .query_row(
            "SELECT created_at FROM stt_jobs WHERE id = ?1",
            params![job_id],
            |row| row.get::<_, String>(0),
        )
        .map_err(db_error)?;
    let queue_wait_ms = now
        .parse::<u64>()
        .unwrap_or_default()
        .saturating_sub(created_at.parse::<u64>().unwrap_or_default());
    let changed = transaction
        .execute(
            "
            UPDATE stt_jobs
            SET status = 'processing',
                attempts = attempts + 1,
                updated_at = ?1,
                started_at = ?1,
                queue_wait_ms = ?3,
                last_error = NULL
            WHERE id = ?2 AND status = 'pending'
            ",
            params![now, job_id, queue_wait_ms],
        )
        .map_err(db_error)?;
    if changed == 0 {
        transaction.commit().map_err(db_error)?;
        return Ok(None);
    }
    transaction.commit().map_err(db_error)?;
    load_stt_job(&connection, job_id).map(Some)
}

async fn process_stt_job(app: AppHandle, runtime: SttRuntime, job: SttJob) {
    let config = {
        let connection = match open_database(&app) {
            Ok(connection) => connection,
            Err(error) => {
                fail_stt_job(&app, &job, error);
                return;
            }
        };
        match load_stt_config(&app, &connection) {
            Ok(config) => config,
            Err(error) => {
                fail_stt_job(&app, &job, error);
                return;
            }
        }
    };
    let audio_path = PathBuf::from(&job.chunk_path);
    let result = runtime.transcribe(config, audio_path).await;

    match result {
        Ok(transcription) => complete_stt_job(&app, &job, transcription),
        Err(error) => fail_stt_job(&app, &job, error),
    }
}

fn complete_stt_job(app: &AppHandle, job: &SttJob, transcription: SttTranscription) {
    let now = now_string();
    if let Ok(connection) = open_database(app) {
        let _ = connection.execute(
            "
            UPDATE stt_jobs
            SET status = 'done',
                updated_at = ?1,
                completed_at = ?1,
                preprocessing_ms = ?3,
                model_load_ms = ?4,
                inference_ms = ?5,
                real_time_factor = ?6,
                last_error = NULL
            WHERE id = ?2
            ",
            params![
                now,
                job.id,
                transcription.preprocessing_ms as i64,
                transcription.model_load_ms as i64,
                transcription.inference_ms as i64,
                transcription.real_time_factor,
            ],
        );
    }
    let _ = app.emit(
        "stt-job-completed",
        SttJobEvent {
            job_id: job.id,
            note_id: job.note_id.clone(),
            source: job.source.clone(),
            path: job.chunk_path.clone(),
            sequence: job.sequence,
            chunk_started_at_ms: job.chunk_started_at_ms,
            duration_ms: job.duration_ms,
            transcription: Some(transcription),
            error: None,
        },
    );
}

fn fail_stt_job(app: &AppHandle, job: &SttJob, error: String) {
    let now = now_string();
    let status = if job.attempts >= 2 {
        "failed"
    } else {
        "pending"
    };
    if let Ok(connection) = open_database(app) {
        let _ = connection.execute(
            "
            UPDATE stt_jobs
            SET status = ?1,
                updated_at = ?2,
                completed_at = CASE WHEN ?1 = 'failed' THEN ?2 ELSE completed_at END,
                last_error = ?3
            WHERE id = ?4
            ",
            params![status, now, error, job.id],
        );
    }
    if status == "failed" {
        let _ = app.emit(
            "stt-job-completed",
            SttJobEvent {
                job_id: job.id,
                note_id: job.note_id.clone(),
                source: job.source.clone(),
                path: job.chunk_path.clone(),
                sequence: job.sequence,
                chunk_started_at_ms: job.chunk_started_at_ms,
                duration_ms: job.duration_ms,
                transcription: None,
                error: Some(error),
            },
        );
    }
}

#[tauri::command]
pub async fn transcribe_last_capture(
    app: AppHandle,
    audio_state: State<'_, AudioCaptureState>,
    runtime: State<'_, SttRuntime>,
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

    runtime.transcribe(config, audio_path).await
}

#[tauri::command]
pub async fn transcribe_capture_file(
    app: AppHandle,
    runtime: State<'_, SttRuntime>,
    path: String,
) -> Result<SttTranscription, String> {
    let audio_path = validate_capture_audio_path(&app, path)?;
    let config = {
        let connection = open_database(&app)?;
        load_stt_config(&app, &connection)?
    };

    runtime.transcribe(config, audio_path).await
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_migration_adds_stt_performance_columns() {
        let connection = Connection::open_in_memory().expect("in-memory database");
        connection
            .execute_batch(
                "CREATE TABLE notes (id TEXT PRIMARY KEY);
                 CREATE TABLE stt_jobs (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    note_id TEXT NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
                    source TEXT NOT NULL CHECK (source IN ('mic', 'system')),
                    chunk_path TEXT NOT NULL,
                    sequence INTEGER NOT NULL,
                    chunk_started_at_ms INTEGER,
                    duration_ms INTEGER,
                    status TEXT NOT NULL DEFAULT 'pending',
                    attempts INTEGER NOT NULL DEFAULT 0,
                    max_attempts INTEGER NOT NULL DEFAULT 2,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    started_at TEXT,
                    completed_at TEXT,
                    last_error TEXT
                 );",
            )
            .expect("legacy schema");

        init_schema(&connection).expect("migrate STT schema");
        let columns = table_columns(&connection, "stt_jobs").expect("table columns");
        for name in [
            "queue_wait_ms",
            "preprocessing_ms",
            "model_load_ms",
            "inference_ms",
            "real_time_factor",
        ] {
            assert!(columns.contains(name), "missing {name}");
        }
    }

    #[test]
    fn enqueue_input_accepts_frontend_camel_case_payload() {
        let input = serde_json::from_value::<EnqueueSttJobInput>(serde_json::json!({
            "noteId": "note-1",
            "source": "mic",
            "path": "/tmp/capture.wav",
            "sequence": 7,
            "chunkStartedAtMs": 1234,
            "durationMs": 5000
        }))
        .expect("deserialize enqueue input");

        assert_eq!(input.note_id, "note-1");
        assert_eq!(input.source, "mic");
        assert_eq!(input.sequence, 7);
        assert_eq!(input.chunk_started_at_ms, Some(1234));
        assert_eq!(input.duration_ms, Some(5000));
    }
}
