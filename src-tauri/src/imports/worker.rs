use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};

use super::{
    claim_job, converters, imported_folder_id, job_by_id,
    types::{ImportFailure, ImportJobClaim, ImportLimits},
};
use crate::{
    app_data_dir, create_standard_note, db_error, now_string, open_database, save_note_internal,
};

const JOB_UPDATED_EVENT: &str = "import-job-updated";
const NOTE_CREATED_EVENT: &str = "import-note-created";

pub(crate) async fn worker(app: AppHandle) {
    loop {
        let claim = open_database(&app).and_then(|mut connection| claim_job(&mut connection));
        match claim {
            Ok(Some(job)) => {
                emit_job(&app, &job.id);
                let worker_app = app.clone();
                let job_id = job.id.clone();
                let result =
                    tauri::async_runtime::spawn_blocking(move || process_job(&worker_app, &job))
                        .await;

                if let Err(error) = result {
                    fail_job(
                        &app,
                        &job_id,
                        ImportFailure::retryable(format!(
                            "Import worker stopped unexpectedly: {error}"
                        )),
                    );
                }
                emit_job(&app, &job_id);
            }
            Ok(None) => tokio::time::sleep(Duration::from_millis(900)).await,
            Err(error) => {
                eprintln!("Document import worker could not claim a job: {error}");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}

fn process_job(app: &AppHandle, job: &ImportJobClaim) {
    if let Err(error) = process_job_inner(app, job) {
        fail_job(app, &job.id, error);
    }
}

fn process_job_inner(app: &AppHandle, job: &ImportJobClaim) -> Result<(), ImportFailure> {
    let path = PathBuf::from(&job.source_path);
    let metadata = path.metadata().map_err(|error| {
        ImportFailure::retryable(format!("Could not read selected file: {error}"))
    })?;
    if !metadata.is_file() {
        return Err(ImportFailure::permanent(
            "The selected path is no longer a file",
        ));
    }
    if metadata.len() != job.source_size {
        return Err(ImportFailure::permanent(
            "The selected file changed after it was queued; select it again",
        ));
    }

    let source_hash = hash_file(&path)?;
    let connection = open_database(app).map_err(ImportFailure::retryable)?;
    let duplicate_note_id: Option<String> = connection
        .query_row(
            "SELECT note_id FROM note_imports WHERE source_hash = ?1 ORDER BY imported_at LIMIT 1",
            params![source_hash],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| ImportFailure::retryable(db_error(error)))?;
    if let Some(note_id) = duplicate_note_id {
        if !job.allow_duplicate {
            mark_duplicate(app, job, &source_hash, &note_id)?;
            return Ok(());
        }
    }
    drop(connection);

    let limits = ImportLimits::default();
    let mut converted = converters::convert(&path, &job.format, limits)?;
    if converted.markdown.trim().is_empty() {
        return Err(ImportFailure::permanent(
            "No readable text was found in this document",
        ));
    }
    let markdown_bytes = converted.markdown.len();
    if markdown_bytes > limits.max_markdown_bytes {
        return Err(ImportFailure::permanent(format!(
            "Converted Markdown is {} MiB; the limit is {} MiB",
            markdown_bytes / 1024 / 1024,
            limits.max_markdown_bytes / 1024 / 1024
        )));
    }
    if markdown_bytes > limits.markdown_warning_bytes {
        converted.warnings.push(format!(
            "Large note: converted Markdown is {:.1} MiB",
            markdown_bytes as f64 / 1024.0 / 1024.0
        ));
    }
    validate_assets(&converted.assets, limits)?;

    let connection = open_database(app).map_err(ImportFailure::retryable)?;
    let folder_id = imported_folder_id(&connection).map_err(ImportFailure::retryable)?;
    drop(connection);

    let title = import_title(converted.title.as_deref(), &path, &job.source_name);
    let note = create_standard_note(app.clone(), Some(title.clone()), Some(folder_id))
        .map_err(ImportFailure::retryable)?;
    let asset_dir = app_data_dir(app)
        .map_err(ImportFailure::retryable)?
        .join("import-assets")
        .join(&note.id);

    let import_result = (|| {
        let markdown = save_assets_and_rewrite(&asset_dir, converted.markdown, converted.assets)?;
        save_note_internal(
            app.clone(),
            note.id.clone(),
            title,
            markdown,
            note.folder_id.clone(),
            false,
        )
        .map_err(ImportFailure::retryable)?;

        let connection = open_database(app).map_err(ImportFailure::retryable)?;
        let now = now_string();
        connection
            .execute(
                "INSERT INTO note_imports (note_id, source_name, source_format, source_hash, imported_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![note.id, job.source_name, job.format, source_hash, now],
            )
            .map_err(|error| ImportFailure::retryable(db_error(error)))?;
        let warnings_json = serde_json::to_string(&converted.warnings)
            .map_err(|error| ImportFailure::permanent(error.to_string()))?;
        connection
            .execute(
                "UPDATE import_jobs
                 SET status = 'succeeded', source_path = NULL, source_hash = ?1,
                     note_id = ?2, warnings_json = ?3, error = NULL,
                     completed_at = ?4, updated_at = ?4
                 WHERE id = ?5",
                params![source_hash, note.id, warnings_json, now, job.id],
            )
            .map_err(|error| ImportFailure::retryable(db_error(error)))?;
        Ok::<(), ImportFailure>(())
    })();

    if let Err(error) = import_result {
        cleanup_incomplete_note(app, &note.id, &asset_dir);
        return Err(error);
    }

    let _ = app.emit(NOTE_CREATED_EVENT, &note.id);
    Ok(())
}

fn hash_file(path: &Path) -> Result<String, ImportFailure> {
    let mut file = fs::File::open(path)
        .map_err(|error| ImportFailure::retryable(format!("Could not open file: {error}")))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| ImportFailure::retryable(format!("Could not read file: {error}")))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn validate_assets(
    assets: &[converters::ConvertedAsset],
    limits: ImportLimits,
) -> Result<(), ImportFailure> {
    let mut total = 0_usize;
    for asset in assets {
        if asset.bytes.len() > limits.max_asset_bytes {
            return Err(ImportFailure::permanent(format!(
                "Embedded asset '{}' exceeds the {} MiB per-asset limit",
                asset.suggested_name,
                limits.max_asset_bytes / 1024 / 1024
            )));
        }
        total = total.saturating_add(asset.bytes.len());
    }
    if total > limits.max_total_asset_bytes {
        return Err(ImportFailure::permanent(format!(
            "Embedded assets exceed the {} MiB total limit",
            limits.max_total_asset_bytes / 1024 / 1024
        )));
    }
    Ok(())
}

fn save_assets_and_rewrite(
    asset_dir: &Path,
    mut markdown: String,
    assets: Vec<converters::ConvertedAsset>,
) -> Result<String, ImportFailure> {
    if assets.is_empty() {
        return Ok(markdown);
    }
    fs::create_dir_all(asset_dir).map_err(|error| {
        ImportFailure::retryable(format!("Could not create asset folder: {error}"))
    })?;

    for (index, asset) in assets.into_iter().enumerate() {
        let name = unique_asset_name(index, &asset.suggested_name);
        let path = asset_dir.join(&name);
        fs::write(&path, asset.bytes).map_err(|error| {
            ImportFailure::retryable(format!("Could not save embedded asset: {error}"))
        })?;
        let url = asset_url(&path);
        markdown = rewrite_asset_reference(&markdown, &asset.source_reference, &url);
        if asset.source_reference != asset.suggested_name {
            markdown = rewrite_asset_reference(&markdown, &asset.suggested_name, &url);
        }
    }
    Ok(markdown)
}

fn rewrite_asset_reference(markdown: &str, reference: &str, replacement: &str) -> String {
    markdown
        .replace(&format!("]({reference})"), &format!("]({replacement})"))
        .replace(&format!("](./{reference})"), &format!("]({replacement})"))
        .replace(
            &format!("](images/{reference})"),
            &format!("]({replacement})"),
        )
}

fn unique_asset_name(index: usize, suggested: &str) -> String {
    let raw = Path::new(suggested)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("asset.bin");
    let sanitized: String = raw
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    format!("{index:04}-{}", sanitized.trim_matches('.'))
}

fn asset_url(path: &Path) -> String {
    let mut encoded = String::new();
    for byte in path.to_string_lossy().as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(*byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    format!("asset://localhost/{encoded}")
}

fn import_title(document_title: Option<&str>, path: &Path, source_name: &str) -> String {
    let candidate = document_title
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| path.file_stem().and_then(|value| value.to_str()))
        .unwrap_or(source_name)
        .trim();
    candidate.chars().take(240).collect()
}

fn mark_duplicate(
    app: &AppHandle,
    job: &ImportJobClaim,
    source_hash: &str,
    note_id: &str,
) -> Result<(), ImportFailure> {
    let connection = open_database(app).map_err(ImportFailure::retryable)?;
    let now = now_string();
    connection
        .execute(
            "UPDATE import_jobs
             SET status = 'duplicate', source_hash = ?1, note_id = ?2,
                 error = 'This file was already imported. Retry to import another copy.',
                 completed_at = ?3, updated_at = ?3
             WHERE id = ?4",
            params![source_hash, note_id, now, job.id],
        )
        .map_err(|error| ImportFailure::retryable(db_error(error)))?;
    Ok(())
}

fn fail_job(app: &AppHandle, id: &str, failure: ImportFailure) {
    let Ok(connection) = open_database(app) else {
        return;
    };
    let Ok((attempts, max_attempts)) = connection.query_row(
        "SELECT attempts, max_attempts FROM import_jobs WHERE id = ?1",
        params![id],
        |row| Ok((row.get::<_, u32>(0)?, row.get::<_, u32>(1)?)),
    ) else {
        return;
    };
    let retry = failure.retryable && attempts < max_attempts;
    let now_value = now_string().parse::<u128>().unwrap_or_default();
    let available_at = if retry {
        (now_value + 5_000).to_string()
    } else {
        now_value.to_string()
    };
    let status = if retry { "pending" } else { "failed" };
    let completed_at = if retry {
        None
    } else {
        Some(now_value.to_string())
    };
    let _ = connection.execute(
        "UPDATE import_jobs
         SET status = ?1, error = ?2, available_at = ?3, updated_at = ?4,
             started_at = NULL, completed_at = ?5
         WHERE id = ?6",
        params![
            status,
            failure.message,
            available_at,
            now_value.to_string(),
            completed_at,
            id
        ],
    );
}

fn emit_job(app: &AppHandle, id: &str) {
    let Ok(connection) = open_database(app) else {
        return;
    };
    if let Ok(job) = job_by_id(&connection, id) {
        let _ = app.emit(JOB_UPDATED_EVENT, job);
    }
}

fn cleanup_incomplete_note(app: &AppHandle, note_id: &str, asset_dir: &Path) {
    if let Ok(connection) = open_database(app) {
        let _ = connection.execute("DELETE FROM notes WHERE id = ?1", params![note_id]);
    }
    if let Ok(root) = app_data_dir(app) {
        let _ = fs::remove_file(root.join("notes").join(format!("{note_id}.md")));
    }
    let _ = fs::remove_dir_all(asset_dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_names_cannot_escape_the_import_directory() {
        assert_eq!(
            unique_asset_name(2, "../../My image.png"),
            "0002-My_image.png"
        );
    }

    #[test]
    fn asset_urls_match_the_tauri_asset_protocol_shape() {
        assert_eq!(
            asset_url(Path::new("/Users/me/My image.png")),
            "asset://localhost/%2FUsers%2Fme%2FMy%20image.png"
        );
    }

    #[test]
    fn rewrites_common_relative_image_references() {
        let markdown = "![one](image.png) ![two](./image.png)";
        assert_eq!(
            rewrite_asset_reference(markdown, "image.png", "asset://image"),
            "![one](asset://image) ![two](asset://image)"
        );
    }
}
