//! Durable local document import pipeline.
//!
//! Tauri commands only validate and enqueue paths selected by the native file
//! dialog. A single background worker converts files, creates ordinary notes,
//! and therefore reuses the existing extraction and semantic-index queues.

mod converters;
mod types;
mod worker;

use std::{collections::HashSet, path::PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use tauri::AppHandle;

use crate::{db_error, new_id, now_string, open_database};

pub(crate) use types::{EnqueueImportsRequest, ImportJobRecord};
pub(crate) use worker::worker;

use types::{ImportJobClaim, ImportLimits};

pub(crate) const IMPORTED_FOLDER_NAME: &str = "Imported";
pub(crate) const IMPORTED_FOLDER_KEY: &str = "imported";
const IMPORTED_FOLDER_ID: &str = "system-folder-imported";

pub(crate) fn init_schema(connection: &Connection) -> Result<(), String> {
    let folder_columns = table_columns(connection, "folders")?;
    if !folder_columns.contains("system_key") {
        connection
            .execute("ALTER TABLE folders ADD COLUMN system_key TEXT", [])
            .map_err(db_error)?;
    }
    connection
        .execute_batch(
            "
            CREATE UNIQUE INDEX IF NOT EXISTS folders_system_key_idx
                ON folders(system_key) WHERE system_key IS NOT NULL;

            CREATE TABLE IF NOT EXISTS import_jobs (
                id TEXT PRIMARY KEY,
                source_path TEXT,
                source_name TEXT NOT NULL,
                source_size INTEGER NOT NULL,
                source_hash TEXT,
                format TEXT NOT NULL,
                status TEXT NOT NULL
                    CHECK (status IN ('pending', 'processing', 'succeeded', 'failed', 'duplicate')),
                attempts INTEGER NOT NULL DEFAULT 0,
                max_attempts INTEGER NOT NULL DEFAULT 2,
                allow_duplicate INTEGER NOT NULL DEFAULT 0,
                available_at TEXT NOT NULL,
                note_id TEXT REFERENCES notes(id) ON DELETE SET NULL,
                warnings_json TEXT NOT NULL DEFAULT '[]',
                error TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                started_at TEXT,
                completed_at TEXT
            );

            CREATE INDEX IF NOT EXISTS import_jobs_claim_idx
                ON import_jobs(status, available_at, created_at);

            CREATE TABLE IF NOT EXISTS note_imports (
                note_id TEXT PRIMARY KEY REFERENCES notes(id) ON DELETE CASCADE,
                source_name TEXT NOT NULL,
                source_format TEXT NOT NULL,
                source_hash TEXT NOT NULL,
                imported_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS note_imports_hash_idx
                ON note_imports(source_hash);
            ",
        )
        .map_err(db_error)?;

    let has_imported = connection
        .query_row(
            "SELECT 1 FROM folders WHERE system_key = ?1 LIMIT 1",
            params![IMPORTED_FOLDER_KEY],
            |_| Ok(()),
        )
        .optional()
        .map_err(db_error)?
        .is_some();
    if !has_imported {
        connection
            .execute(
                "
                UPDATE folders
                SET system_key = ?1, name = ?2
                WHERE id = (
                    SELECT id FROM folders
                    WHERE lower(name) = lower(?2) AND system_key IS NULL
                    ORDER BY created_at ASC LIMIT 1
                )
                ",
                params![IMPORTED_FOLDER_KEY, IMPORTED_FOLDER_NAME],
            )
            .map_err(db_error)?;
    }
    connection
        .execute(
            "
            INSERT INTO folders (id, name, created_at, system_key)
            SELECT ?1, ?2, ?3, ?4
            WHERE NOT EXISTS (SELECT 1 FROM folders WHERE system_key = ?4)
            ",
            params![
                IMPORTED_FOLDER_ID,
                IMPORTED_FOLDER_NAME,
                now_string(),
                IMPORTED_FOLDER_KEY
            ],
        )
        .map_err(db_error)?;
    connection
        .execute(
            "UPDATE folders SET name = ?1 WHERE system_key = ?2",
            params![IMPORTED_FOLDER_NAME, IMPORTED_FOLDER_KEY],
        )
        .map_err(db_error)?;
    Ok(())
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

pub(crate) fn is_system_folder(connection: &Connection, id: &str) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT system_key IS NOT NULL FROM folders WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .optional()
        .map(|value| value.unwrap_or(false))
        .map_err(db_error)
}

pub(crate) fn imported_folder_id(connection: &Connection) -> Result<String, String> {
    connection
        .query_row(
            "SELECT id FROM folders WHERE system_key = ?1",
            params![IMPORTED_FOLDER_KEY],
            |row| row.get(0),
        )
        .map_err(db_error)
}

#[tauri::command]
pub(crate) fn enqueue_imports(
    app: AppHandle,
    request: EnqueueImportsRequest,
) -> Result<Vec<ImportJobRecord>, String> {
    let limits = ImportLimits::default();
    if request.paths.is_empty() {
        return Err("Choose at least one file".to_string());
    }
    if request.paths.len() > limits.max_files_per_batch {
        return Err(format!(
            "A batch can contain at most {} files",
            limits.max_files_per_batch
        ));
    }

    let mut sources = Vec::new();
    let mut seen = HashSet::new();
    let mut batch_bytes = 0_u64;
    for raw_path in request.paths {
        let path = PathBuf::from(raw_path)
            .canonicalize()
            .map_err(|error| format!("Could not access selected file: {error}"))?;
        if !seen.insert(path.clone()) {
            continue;
        }
        let metadata = path
            .metadata()
            .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
        if !metadata.is_file() {
            return Err(format!("{} is not a regular file", path.display()));
        }
        let format = detect_format(&path)?;
        validate_source_size(&format, metadata.len(), limits)?;
        batch_bytes = batch_bytes.saturating_add(metadata.len());
        if batch_bytes > limits.max_batch_bytes {
            return Err(format!(
                "Selected files exceed the {} MiB batch limit",
                limits.max_batch_bytes / 1024 / 1024
            ));
        }
        let source_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("Imported document")
            .to_string();
        sources.push((path, source_name, metadata.len(), format));
    }

    let connection = open_database(&app)?;
    let now = now_string();
    let mut ids = Vec::new();
    for (path, source_name, source_size, format) in sources {
        let id = format!("{}-{:016x}", new_id("import-job"), rand::random::<u64>());
        connection
            .execute(
                "
                INSERT INTO import_jobs (
                    id, source_path, source_name, source_size, format, status,
                    attempts, max_attempts, allow_duplicate, available_at,
                    created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, 'pending', 0, 2, 0, ?6, ?6, ?6)
                ",
                params![
                    id,
                    path.to_string_lossy(),
                    source_name,
                    source_size as i64,
                    format,
                    now
                ],
            )
            .map_err(db_error)?;
        ids.push(id);
    }
    jobs_by_ids(&connection, &ids)
}

#[tauri::command]
pub(crate) fn list_import_jobs(
    app: AppHandle,
    limit: Option<u32>,
) -> Result<Vec<ImportJobRecord>, String> {
    let connection = open_database(&app)?;
    let limit = limit.unwrap_or(50).clamp(1, 200);
    let mut statement = connection
        .prepare(
            "
            SELECT id, source_name, source_size, format, status, attempts,
                   max_attempts, note_id, warnings_json, error, created_at, updated_at
            FROM import_jobs ORDER BY created_at DESC LIMIT ?1
            ",
        )
        .map_err(db_error)?;
    let jobs = statement
        .query_map(params![limit], map_job)
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    Ok(jobs)
}

#[tauri::command]
pub(crate) fn retry_import_job(app: AppHandle, id: String) -> Result<ImportJobRecord, String> {
    let connection = open_database(&app)?;
    let now = now_string();
    let changed = connection
        .execute(
            "
            UPDATE import_jobs
            SET status = 'pending', attempts = 0, allow_duplicate = 1,
                available_at = ?1, error = NULL, completed_at = NULL,
                started_at = NULL, updated_at = ?1
            WHERE id = ?2 AND status IN ('failed', 'duplicate') AND source_path IS NOT NULL
            ",
            params![now, id],
        )
        .map_err(db_error)?;
    if changed == 0 {
        return Err("This import cannot be retried".to_string());
    }
    job_by_id(&connection, &id)
}

pub(crate) fn recover_interrupted_jobs(connection: &Connection) -> Result<(), String> {
    connection
        .execute(
            "
            UPDATE import_jobs
            SET status = 'pending', started_at = NULL, available_at = ?1,
                updated_at = ?1, error = 'Interrupted while importing; retrying'
            WHERE status = 'processing' AND source_path IS NOT NULL
            ",
            params![now_string()],
        )
        .map_err(db_error)?;
    Ok(())
}

pub(crate) fn claim_job(connection: &mut Connection) -> Result<Option<ImportJobClaim>, String> {
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(db_error)?;
    let now = now_string();
    let claim = transaction
        .query_row(
            "
            SELECT id, source_path, source_name, source_size, format, attempts,
                   allow_duplicate
            FROM import_jobs
            WHERE status = 'pending' AND source_path IS NOT NULL
              AND CAST(available_at AS INTEGER) <= CAST(?1 AS INTEGER)
            ORDER BY created_at ASC LIMIT 1
            ",
            params![now],
            |row| {
                Ok(ImportJobClaim {
                    id: row.get(0)?,
                    source_path: row.get(1)?,
                    source_name: row.get(2)?,
                    source_size: row.get::<_, i64>(3)? as u64,
                    format: row.get(4)?,
                    attempts: row.get::<_, i64>(5)? as u32 + 1,
                    allow_duplicate: row.get::<_, i64>(6)? != 0,
                })
            },
        )
        .optional()
        .map_err(db_error)?;
    if let Some(job) = &claim {
        transaction
            .execute(
                "
                UPDATE import_jobs SET status = 'processing', attempts = ?1,
                    started_at = ?2, updated_at = ?2, error = NULL
                WHERE id = ?3
                ",
                params![job.attempts, now, job.id],
            )
            .map_err(db_error)?;
    }
    transaction.commit().map_err(db_error)?;
    Ok(claim)
}

pub(crate) fn job_by_id(connection: &Connection, id: &str) -> Result<ImportJobRecord, String> {
    connection
        .query_row(
            "
            SELECT id, source_name, source_size, format, status, attempts,
                   max_attempts, note_id, warnings_json, error, created_at, updated_at
            FROM import_jobs WHERE id = ?1
            ",
            params![id],
            map_job,
        )
        .map_err(db_error)
}

fn jobs_by_ids(connection: &Connection, ids: &[String]) -> Result<Vec<ImportJobRecord>, String> {
    ids.iter().map(|id| job_by_id(connection, id)).collect()
}

fn map_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<ImportJobRecord> {
    let warnings_json: String = row.get(8)?;
    Ok(ImportJobRecord {
        id: row.get(0)?,
        source_name: row.get(1)?,
        source_size: row.get::<_, i64>(2)? as u64,
        format: row.get(3)?,
        status: row.get(4)?,
        attempts: row.get::<_, i64>(5)? as u32,
        max_attempts: row.get::<_, i64>(6)? as u32,
        note_id: row.get(7)?,
        warnings: serde_json::from_str(&warnings_json).unwrap_or_default(),
        error: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

fn detect_format(path: &std::path::Path) -> Result<String, String> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| "The selected file has no supported extension".to_string())?;
    const SUPPORTED: &[&str] = &[
        "pdf", "docx", "pptx", "xlsx", "xls", "html", "htm", "csv", "ipynb", "json", "xml", "txt",
        "md", "rst", "log", "toml", "yaml", "yml", "ini", "cfg", "py", "rs", "js", "jsx", "ts",
        "tsx", "c", "h", "cpp", "hpp", "go", "java", "rb", "swift", "sh", "sql", "css",
    ];
    if SUPPORTED.contains(&extension.as_str()) {
        Ok(extension)
    } else {
        Err(format!("Unsupported file type: .{extension}"))
    }
}

fn validate_source_size(format: &str, size: u64, limits: ImportLimits) -> Result<(), String> {
    let limit = match format {
        "pdf" => limits.max_pdf_bytes,
        "docx" | "pptx" | "xlsx" | "xls" => limits.max_office_bytes,
        "html" | "htm" | "csv" | "ipynb" | "json" | "xml" => limits.max_structured_bytes,
        _ => limits.max_text_bytes,
    };
    if size > limit {
        Err(format!(
            "File is {} MiB; the .{} limit is {} MiB",
            size / 1024 / 1024,
            format,
            limit / 1024 / 1024
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_database() -> Connection {
        let connection = Connection::open_in_memory().expect("in-memory database");
        connection
            .execute_batch(
                "CREATE TABLE folders (
                    id TEXT PRIMARY KEY NOT NULL,
                    name TEXT NOT NULL,
                    created_at TEXT NOT NULL
                );
                CREATE TABLE notes (
                    id TEXT PRIMARY KEY NOT NULL
                );",
            )
            .expect("base schema");
        connection
    }

    #[test]
    fn migration_adopts_an_existing_imported_folder_and_is_idempotent() {
        let connection = base_database();
        connection
            .execute(
                "INSERT INTO folders (id, name, created_at) VALUES ('existing', 'Imported', '1')",
                [],
            )
            .expect("existing folder");

        init_schema(&connection).expect("first migration");
        init_schema(&connection).expect("second migration");

        assert_eq!(imported_folder_id(&connection).unwrap(), "existing");
        assert!(is_system_folder(&connection, "existing").unwrap());
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM folders WHERE system_key = 'imported'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
    }
}
