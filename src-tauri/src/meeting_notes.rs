use std::{collections::HashMap, time::Duration};

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};

use crate::{
    chat, create_note_with_extraction_status, db_error, disabled_extraction_status, load_note_meta,
    now_string, open_database, read_note_content, save_note_internal, NoteWithContent,
};

const POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug)]
struct MeetingNoteJob {
    id: i64,
    transcript_note_id: String,
    user_note_id: String,
    content_hash: String,
    attempts: u32,
    max_attempts: u32,
}

#[derive(Debug, Serialize)]
pub(crate) struct MeetingNoteCompletionStatus {
    pub(crate) status: String,
    pub(crate) user_note_id: String,
    pub(crate) empty_headings: usize,
}

#[derive(Clone, Debug, Serialize)]
struct MeetingNoteCompletedEvent {
    note_id: String,
    status: String,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GeneratedSections {
    sections: Vec<GeneratedSection>,
}

#[derive(Debug, Deserialize)]
struct GeneratedSection {
    heading: String,
    content: String,
}

#[derive(Clone, Debug)]
struct EmptyHeading {
    title: String,
    line_index: usize,
    next_heading_index: usize,
}

pub(crate) fn init_schema(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "
            CREATE TABLE IF NOT EXISTS meeting_note_jobs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                transcript_note_id TEXT NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
                user_note_id TEXT NOT NULL UNIQUE REFERENCES notes(id) ON DELETE CASCADE,
                content_hash TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending'
                    CHECK (status IN ('pending', 'processing', 'done', 'failed')),
                attempts INTEGER NOT NULL DEFAULT 0,
                max_attempts INTEGER NOT NULL DEFAULT 3,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                started_at TEXT,
                completed_at TEXT,
                last_error TEXT
            );

            CREATE INDEX IF NOT EXISTS meeting_note_jobs_claim_idx
                ON meeting_note_jobs(status, created_at, id);
            ",
        )
        .map_err(db_error)
}

pub(crate) fn recover_interrupted_jobs(connection: &Connection) -> Result<(), String> {
    connection
        .execute(
            "
            UPDATE meeting_note_jobs
            SET status = 'pending', started_at = NULL, updated_at = ?1
            WHERE status = 'processing'
            ",
            params![now_string()],
        )
        .map_err(db_error)?;
    Ok(())
}

#[tauri::command]
pub(crate) fn create_meeting_quick_note(
    app: AppHandle,
    transcript_note_id: String,
    transcript_title: String,
    content: String,
) -> Result<NoteWithContent, String> {
    if content.trim().is_empty() {
        return Err("Meeting note content is empty".to_string());
    }
    let transcript_meta = {
        let connection = open_database(&app)?;
        load_note_meta(&connection, &transcript_note_id)?
    };
    if transcript_meta.deleted_at.is_some() {
        return Err("Meeting transcript is in the trash".to_string());
    }

    let title = format!(
        "{} Notes",
        transcript_title.trim().trim_end_matches(" Notes")
    );
    let note = create_note_with_extraction_status(
        app.clone(),
        Some(title.clone()),
        transcript_meta.folder_id.clone(),
        disabled_extraction_status(),
    )?;
    let note = save_note_internal(
        app.clone(),
        note.id,
        title,
        content,
        transcript_meta.folder_id,
        true,
    )?;

    let connection = open_database(&app)?;
    let created_at = now_string();
    connection
        .execute(
            "
            INSERT INTO note_links (source_id, target_id, created_at, label, link_kind)
            VALUES (?1, ?2, ?3, 'Meeting Note', 'manual')
            ON CONFLICT(source_id, target_id) DO UPDATE SET label = excluded.label
            ",
            params![transcript_note_id, note.id, created_at],
        )
        .map_err(db_error)?;
    connection
        .execute(
            "
            INSERT INTO note_links (source_id, target_id, created_at, label, link_kind)
            VALUES (?1, ?2, ?3, 'Parent Meeting', 'manual')
            ON CONFLICT(source_id, target_id) DO UPDATE SET label = excluded.label
            ",
            params![note.id, transcript_note_id, created_at],
        )
        .map_err(db_error)?;
    Ok(note)
}

#[tauri::command]
pub(crate) fn enqueue_meeting_note_completion(
    app: AppHandle,
    transcript_note_id: String,
    user_note_id: String,
) -> Result<MeetingNoteCompletionStatus, String> {
    let content = read_note_content(&app, &user_note_id)?;
    let headings = empty_headings(&content);
    if headings.is_empty() {
        enable_note_extraction(&app, &user_note_id, &content)?;
        return Ok(MeetingNoteCompletionStatus {
            status: "not_needed".to_string(),
            user_note_id,
            empty_headings: 0,
        });
    }

    let now = now_string();
    let content_hash = hash_text(&content);
    let connection = open_database(&app)?;
    connection
        .execute(
            "
            INSERT INTO meeting_note_jobs (
                transcript_note_id, user_note_id, content_hash, status,
                attempts, max_attempts, created_at, updated_at
            )
            VALUES (?1, ?2, ?3, 'pending', 0, 3, ?4, ?4)
            ON CONFLICT(user_note_id) DO UPDATE SET
                transcript_note_id = excluded.transcript_note_id,
                content_hash = excluded.content_hash,
                status = 'pending',
                attempts = 0,
                updated_at = excluded.updated_at,
                started_at = NULL,
                completed_at = NULL,
                last_error = NULL
            ",
            params![transcript_note_id, user_note_id, content_hash, now],
        )
        .map_err(db_error)?;
    Ok(MeetingNoteCompletionStatus {
        status: "queued".to_string(),
        user_note_id,
        empty_headings: headings.len(),
    })
}

pub(crate) async fn worker(app: AppHandle) {
    loop {
        match claim_job(&app) {
            Ok(Some(job)) => process_job(&app, &job).await,
            Ok(None) => tokio::time::sleep(POLL_INTERVAL).await,
            Err(error) => {
                eprintln!("[smooth:meeting-note-worker] {error}");
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    }
}

fn claim_job(app: &AppHandle) -> Result<Option<MeetingNoteJob>, String> {
    let mut connection = open_database(app)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error)?;
    let job = transaction
        .query_row(
            "
            SELECT id, transcript_note_id, user_note_id, content_hash, attempts, max_attempts
            FROM meeting_note_jobs
            WHERE status = 'pending'
            ORDER BY created_at, id
            LIMIT 1
            ",
            [],
            |row| {
                Ok(MeetingNoteJob {
                    id: row.get(0)?,
                    transcript_note_id: row.get(1)?,
                    user_note_id: row.get(2)?,
                    content_hash: row.get(3)?,
                    attempts: row.get::<_, u32>(4)? + 1,
                    max_attempts: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(db_error)?;
    let Some(job) = job else {
        transaction.commit().map_err(db_error)?;
        return Ok(None);
    };
    let changed = transaction
        .execute(
            "
            UPDATE meeting_note_jobs
            SET status = 'processing', attempts = attempts + 1,
                started_at = ?1, updated_at = ?1, last_error = NULL
            WHERE id = ?2 AND status = 'pending'
            ",
            params![now_string(), job.id],
        )
        .map_err(db_error)?;
    transaction.commit().map_err(db_error)?;
    Ok((changed == 1).then_some(job))
}

async fn process_job(app: &AppHandle, job: &MeetingNoteJob) {
    if let Err(error) = process_job_inner(app, job).await {
        let _ = fail_job(app, job, &error);
    }
}

async fn process_job_inner(app: &AppHandle, job: &MeetingNoteJob) -> Result<(), String> {
    let transcript_meta = {
        let connection = open_database(app)?;
        load_note_meta(&connection, &job.transcript_note_id)?
    };
    let transcript = read_note_content(app, &job.transcript_note_id)?;
    let user_note = read_note_content(app, &job.user_note_id)?;
    if hash_text(&user_note) != job.content_hash {
        return requeue_changed_note(app, job, &user_note);
    }
    let headings = empty_headings(&user_note);
    if headings.is_empty() {
        enable_note_extraction(app, &job.user_note_id, &user_note)?;
        return complete_job(app, job, "done");
    }
    let heading_titles = headings
        .iter()
        .map(|heading| heading.title.clone())
        .collect::<Vec<_>>();
    let raw = chat::generate_meeting_note_sections(
        app,
        &job.transcript_note_id,
        &transcript_meta.title,
        &transcript,
        &user_note,
        &heading_titles,
    )
    .await?;
    let generated = parse_generated_sections(&raw)?;

    let latest = read_note_content(app, &job.user_note_id)?;
    if hash_text(&latest) != job.content_hash {
        return requeue_changed_note(app, job, &latest);
    }
    let filled = fill_empty_sections(&latest, &generated);
    enable_note_extraction(app, &job.user_note_id, &filled)?;
    complete_job(app, job, "done")
}

fn enable_note_extraction(app: &AppHandle, note_id: &str, content: &str) -> Result<(), String> {
    let meta = {
        let connection = open_database(app)?;
        load_note_meta(&connection, note_id)?
    };
    save_note_internal(
        app.clone(),
        note_id.to_string(),
        meta.title,
        content.to_string(),
        meta.folder_id,
        false,
    )?;
    Ok(())
}

fn complete_job(app: &AppHandle, job: &MeetingNoteJob, status: &str) -> Result<(), String> {
    let connection = open_database(app)?;
    connection
        .execute(
            "
            UPDATE meeting_note_jobs
            SET status = ?1, updated_at = ?2, completed_at = ?2, last_error = NULL
            WHERE id = ?3
            ",
            params![status, now_string(), job.id],
        )
        .map_err(db_error)?;
    let _ = app.emit(
        "meeting-note-completed",
        MeetingNoteCompletedEvent {
            note_id: job.user_note_id.clone(),
            status: status.to_string(),
            error: None,
        },
    );
    Ok(())
}

fn fail_job(app: &AppHandle, job: &MeetingNoteJob, error: &str) -> Result<(), String> {
    let final_failure = job.attempts >= job.max_attempts;
    let status = if final_failure { "failed" } else { "pending" };
    let connection = open_database(app)?;
    connection
        .execute(
            "
            UPDATE meeting_note_jobs
            SET status = ?1, updated_at = ?2, last_error = ?3,
                completed_at = CASE WHEN ?1 = 'failed' THEN ?2 ELSE completed_at END
            WHERE id = ?4
            ",
            params![status, now_string(), error, job.id],
        )
        .map_err(db_error)?;
    if final_failure {
        let _ = app.emit(
            "meeting-note-completed",
            MeetingNoteCompletedEvent {
                note_id: job.user_note_id.clone(),
                status: status.to_string(),
                error: Some(error.to_string()),
            },
        );
    }
    Ok(())
}

fn requeue_changed_note(
    app: &AppHandle,
    job: &MeetingNoteJob,
    content: &str,
) -> Result<(), String> {
    let connection = open_database(app)?;
    connection
        .execute(
            "
            UPDATE meeting_note_jobs
            SET status = 'pending', content_hash = ?1, attempts = 0,
                updated_at = ?2, started_at = NULL, last_error = NULL
            WHERE id = ?3
            ",
            params![hash_text(content), now_string(), job.id],
        )
        .map_err(db_error)?;
    Ok(())
}

fn heading_title(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let hashes = trimmed
        .chars()
        .take_while(|character| *character == '#')
        .count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let remainder = trimmed.get(hashes..)?;
    if !remainder.starts_with(char::is_whitespace) {
        return None;
    }
    let title = remainder.trim().trim_end_matches('#').trim();
    (!title.is_empty()).then(|| title.to_string())
}

fn empty_headings(content: &str) -> Vec<EmptyHeading> {
    let lines = content.lines().collect::<Vec<_>>();
    let headings = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| heading_title(line).map(|title| (index, title)))
        .collect::<Vec<_>>();
    headings
        .iter()
        .enumerate()
        .filter_map(|(position, (line_index, title))| {
            let next = headings
                .get(position + 1)
                .map(|(index, _)| *index)
                .unwrap_or(lines.len());
            lines[*line_index + 1..next]
                .iter()
                .all(|line| line.trim().is_empty())
                .then(|| EmptyHeading {
                    title: title.clone(),
                    line_index: *line_index,
                    next_heading_index: next,
                })
        })
        .collect()
}

fn parse_generated_sections(raw: &str) -> Result<HashMap<String, String>, String> {
    let start = raw
        .find('{')
        .ok_or_else(|| "Model returned no JSON".to_string())?;
    let end = raw
        .rfind('}')
        .ok_or_else(|| "Model returned incomplete JSON".to_string())?;
    let parsed = serde_json::from_str::<GeneratedSections>(&raw[start..=end])
        .map_err(|error| format!("Invalid meeting note JSON: {error}"))?;
    Ok(parsed
        .sections
        .into_iter()
        .filter_map(|section| {
            let content = section.content.trim().to_string();
            (!content.is_empty()).then(|| (normalize_heading(&section.heading), content))
        })
        .collect())
}

fn fill_empty_sections(content: &str, generated: &HashMap<String, String>) -> String {
    let mut lines = content.lines().map(str::to_string).collect::<Vec<_>>();
    let headings = empty_headings(content);
    for heading in headings.into_iter().rev() {
        let Some(body) = generated.get(&normalize_heading(&heading.title)) else {
            continue;
        };
        let replacement = body.lines().map(str::to_string).collect::<Vec<_>>();
        lines.splice(
            heading.line_index + 1..heading.next_heading_index,
            replacement,
        );
    }
    let mut result = lines.join("\n");
    if content.ends_with('\n') {
        result.push('\n');
    }
    result
}

fn normalize_heading(value: &str) -> String {
    value.trim().trim_start_matches('#').trim().to_lowercase()
}

fn hash_text(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_and_fills_only_empty_heading_sections() {
        let content = "Opening note\n\n# To do\n\n# Decisions\nAlready agreed\n\n# Next steps\n";
        let headings = empty_headings(content);
        assert_eq!(
            headings
                .iter()
                .map(|heading| heading.title.as_str())
                .collect::<Vec<_>>(),
            vec!["To do", "Next steps"]
        );

        let generated = HashMap::from([
            ("to do".to_string(), "- Send proposal".to_string()),
            ("next steps".to_string(), "Meet on Friday".to_string()),
        ]);
        let filled = fill_empty_sections(content, &generated);
        assert!(filled.contains("# To do\n- Send proposal"));
        assert!(filled.contains("# Decisions\nAlready agreed"));
        assert!(filled.contains("# Next steps\nMeet on Friday"));
    }
}
