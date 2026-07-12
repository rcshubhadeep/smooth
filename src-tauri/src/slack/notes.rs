use rusqlite::{params, OptionalExtension};
use tauri::{AppHandle, Emitter};

use crate::{
    create_standard_note, db_error, new_id, now_string, open_database, save_note_internal,
};

use super::api::SlackMessage;

pub(crate) const SLACK_FOLDER_NAME: &str = "Notes From Slack";

pub(crate) fn claim_event(
    app: &AppHandle,
    event_id: &str,
    channel: &str,
    thread_ts: &str,
) -> Result<bool, String> {
    let connection = open_database(app)?;
    let now = now_string();
    let changed = connection
        .execute(
            "INSERT INTO slack_events
         (event_id, channel_id, thread_ts, status, attempts, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'processing', 1, ?4, ?4)
         ON CONFLICT(event_id) DO UPDATE SET
           status='processing', attempts=slack_events.attempts+1,
           error=NULL, updated_at=excluded.updated_at
         WHERE slack_events.status='failed' AND slack_events.attempts < 3",
            params![event_id, channel, thread_ts, now],
        )
        .map_err(db_error)?;
    Ok(changed > 0)
}

pub(crate) fn complete_event(app: &AppHandle, event_id: &str, note_id: &str) {
    if let Ok(connection) = open_database(app) {
        let _ = connection.execute(
            "UPDATE slack_events SET status='completed', note_id=?1, error=NULL, updated_at=?2 WHERE event_id=?3",
            params![note_id, now_string(), event_id],
        );
    }
}

pub(crate) fn fail_event(app: &AppHandle, event_id: &str, error: &str) {
    if let Ok(connection) = open_database(app) {
        let _ = connection.execute(
            "UPDATE slack_events SET status='failed', error=?1, updated_at=?2 WHERE event_id=?3",
            params![
                error.chars().take(500).collect::<String>(),
                now_string(),
                event_id
            ],
        );
    }
}

pub(crate) fn create_thread_note(
    app: AppHandle,
    channel: &str,
    thread_ts: &str,
    trigger_ts: &str,
    messages: &[SlackMessage],
) -> Result<(String, String), String> {
    let folder_id = exact_slack_folder(&app)?;
    let included: Vec<_> = messages
        .iter()
        .filter(|message| message.ts.as_deref() != Some(trigger_ts))
        .collect();
    if included.is_empty() {
        return Err("The Slack thread did not contain any messages to save".to_string());
    }
    let title = infer_slack_title(included[0].text.as_deref().unwrap_or("Slack thread"));
    let mut content = format!(
        "# Slack thread\n\n- Channel: `{channel}`\n- Thread: `{thread_ts}`\n- Imported: {}\n\n## Conversation\n",
        now_string()
    );
    for message in included {
        let author = message
            .user
            .as_deref()
            .or(message.bot_id.as_deref())
            .unwrap_or("Unknown");
        let timestamp = message.ts.as_deref().unwrap_or("unknown time");
        let text = message.text.as_deref().unwrap_or("").trim();
        if !text.is_empty() {
            content.push_str(&format!("\n### {author} · {timestamp}\n\n{text}\n"));
        }
    }
    let note = create_standard_note(app.clone(), Some(title.clone()), Some(folder_id))?;
    let saved = save_note_internal(
        app.clone(),
        note.id,
        title.clone(),
        content,
        note.folder_id,
        false,
    )?;
    let _ = app.emit("slack-note-created", &saved.id);
    Ok((saved.id, title))
}

fn exact_slack_folder(app: &AppHandle) -> Result<String, String> {
    let connection = open_database(app)?;
    if let Some(id) = connection
        .query_row(
            "SELECT id FROM folders WHERE name=?1 ORDER BY created_at LIMIT 1",
            params![SLACK_FOLDER_NAME],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(db_error)?
    {
        return Ok(id);
    }
    let id = new_id("folder");
    connection
        .execute(
            "INSERT INTO folders (id, name, created_at) VALUES (?1, ?2, ?3)",
            params![id, SLACK_FOLDER_NAME, now_string()],
        )
        .map_err(db_error)?;
    Ok(id)
}

fn infer_slack_title(text: &str) -> String {
    let clean = text
        .replace(['\n', '\r'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let clean = clean
        .trim()
        .trim_start_matches(|character| character == '#' || character == '*' || character == '>');
    let title: String = clean.chars().take(72).collect();
    if title.is_empty() {
        "Slack thread".to_string()
    } else {
        title
    }
}
