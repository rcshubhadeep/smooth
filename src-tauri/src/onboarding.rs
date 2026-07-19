//! First-run onboarding support.
//!
//! Two concerns live here, both deliberately small:
//! 1. Wizard state — persisted in `app_meta` so a half-finished onboarding
//!    resumes where the user left off (and never shows again once completed).
//! 2. Whisper model download — the STT engine only consumes a `model_path`;
//!    this module fetches the chosen ggml model into the app-support `models/`
//!    directory with resume support and progress events, then points the STT
//!    config at it. The heavy gemma download is NOT handled here — llama.cpp's
//!    managed server does that itself.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::{app_data_dir, app_meta_value, db_error, open_database, stt};

const DOWNLOAD_EVENT: &str = "stt-model-download";

/// Progress payload streamed to the frontend while a model downloads.
#[derive(Clone, Serialize)]
struct DownloadProgress<'a> {
    kind: &'a str,
    downloaded: u64,
    total: Option<u64>,
    done: bool,
    error: Option<&'a str>,
}

#[derive(Serialize)]
pub(crate) struct OnboardingStatus {
    pub completed: bool,
    pub step: u32,
}

#[tauri::command]
pub(crate) fn get_onboarding_status(app: AppHandle) -> Result<OnboardingStatus, String> {
    let connection = open_database(&app)?;
    let completed = app_meta_value(&connection, "onboarding_completed")?
        .map(|value| value == "true")
        .unwrap_or(false);
    let step = app_meta_value(&connection, "onboarding_step")?
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    Ok(OnboardingStatus { completed, step })
}

#[tauri::command]
pub(crate) fn set_onboarding_status(
    app: AppHandle,
    completed: bool,
    step: u32,
) -> Result<(), String> {
    let connection = open_database(&app)?;
    connection
        .execute(
            "INSERT INTO app_meta (key, value) VALUES ('onboarding_completed', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [if completed { "true" } else { "false" }],
        )
        .map_err(db_error)?;
    connection
        .execute(
            "INSERT INTO app_meta (key, value) VALUES ('onboarding_step', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [step.to_string()],
        )
        .map_err(db_error)?;
    Ok(())
}

/// Which whisper model to install. English-only is a fraction of the size;
/// multilingual (medium) covers the roman-script languages we expose.
fn model_file(kind: &str) -> Result<&'static str, String> {
    match kind {
        "english" => Ok("ggml-base.en.bin"),
        "multilingual" => Ok("ggml-medium.bin"),
        other => Err(format!("Unknown STT model kind: {other}")),
    }
}

fn models_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app_data_dir(app)?.join("models");
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir)
}

fn emit_progress(app: &AppHandle, kind: &str, downloaded: u64, total: Option<u64>, done: bool) {
    let _ = app.emit(
        DOWNLOAD_EVENT,
        DownloadProgress {
            kind,
            downloaded,
            total,
            done,
            error: None,
        },
    );
}

/// Download the chosen whisper model (resuming any partial file), then point
/// the STT config at it and store the chosen default language. Returns the
/// final model path.
#[tauri::command]
pub(crate) async fn download_stt_model(
    app: AppHandle,
    kind: String,
    language: Option<String>,
) -> Result<String, String> {
    let file_name = model_file(&kind)?;
    let final_path = models_dir(&app)?.join(file_name);

    if !final_path.exists() {
        let url =
            format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{file_name}");
        let part_path = final_path.with_extension("bin.part");
        let resume_from = fs::metadata(&part_path).map(|meta| meta.len()).unwrap_or(0);

        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|error| error.to_string())?;
        let mut request = client.get(&url);
        if resume_from > 0 {
            request = request.header("Range", format!("bytes={resume_from}-"));
        }
        let response = request.send().await.map_err(|error| error.to_string())?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("Model download failed: HTTP {status}"));
        }

        // 206 means the server honored our Range header; anything else restarts.
        let resuming = status == reqwest::StatusCode::PARTIAL_CONTENT;
        let mut downloaded = if resuming { resume_from } else { 0 };
        let total = response.content_length().map(|len| len + downloaded);

        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(resuming)
            .write(true)
            .truncate(!resuming)
            .open(&part_path)
            .map_err(|error| error.to_string())?;

        emit_progress(&app, &kind, downloaded, total, false);
        let mut response = response;
        let mut last_emitted = downloaded;
        while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
            file.write_all(&chunk).map_err(|error| error.to_string())?;
            downloaded += chunk.len() as u64;
            // Throttle events to roughly every 4 MB.
            if downloaded - last_emitted >= 4 * 1024 * 1024 {
                emit_progress(&app, &kind, downloaded, total, false);
                last_emitted = downloaded;
            }
        }
        file.flush().map_err(|error| error.to_string())?;
        drop(file);
        fs::rename(&part_path, &final_path).map_err(|error| error.to_string())?;
    }

    // Point the STT engine at the model and remember the chosen language.
    let mut config = stt::get_stt_config(app.clone())?;
    config.model_path = final_path.to_string_lossy().into_owned();
    if let Some(language) = language {
        config.language = Some(language);
    }
    stt::save_stt_config(app.clone(), config)?;

    emit_progress(&app, &kind, 0, None, true);
    Ok(final_path.to_string_lossy().into_owned())
}
