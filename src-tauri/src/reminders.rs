use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::{agents::reminder_workflows, db_error, new_id, now_string, open_database};

const DELIVERY_POLL_SECONDS: u64 = 10;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReminderRecord {
    pub id: String,
    pub note_id: String,
    pub note_title: String,
    pub scheduled_at: i64,
    pub comment: Option<String>,
    pub selected_text: String,
    pub start_offset: i64,
    pub end_offset: i64,
    pub context_before: String,
    pub context_after: String,
    pub status: String,
    pub last_notified_at: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateReminderInput {
    note_id: String,
    scheduled_at: i64,
    comment: Option<String>,
    selected_text: String,
    start_offset: i64,
    end_offset: i64,
    context_before: String,
    context_after: String,
    #[serde(default)]
    workflow_steps: Vec<reminder_workflows::WorkflowStepInput>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SnoozeReminderInput {
    id: String,
    scheduled_at: i64,
}

pub(crate) fn init_schema(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "
            CREATE TABLE IF NOT EXISTS reminders (
                id TEXT PRIMARY KEY NOT NULL,
                note_id TEXT NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
                scheduled_at INTEGER NOT NULL,
                comment TEXT,
                selected_text TEXT NOT NULL,
                start_offset INTEGER NOT NULL,
                end_offset INTEGER NOT NULL,
                context_before TEXT NOT NULL DEFAULT '',
                context_after TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL DEFAULT 'pending'
                    CHECK (status IN ('pending', 'completed', 'dismissed')),
                last_notified_at INTEGER,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS reminders_due_idx
                ON reminders(status, scheduled_at, last_notified_at);
            CREATE INDEX IF NOT EXISTS reminders_note_idx
                ON reminders(note_id, status, scheduled_at);
            ",
        )
        .map_err(db_error)
}

fn validate_input(input: &CreateReminderInput) -> Result<(), String> {
    if input.note_id.trim().is_empty() {
        return Err("A note is required".to_string());
    }
    if input.selected_text.trim().is_empty() {
        return Err("Select some note text first".to_string());
    }
    if input.selected_text.chars().count() > 10_000 {
        return Err("The selected text is too long for a reminder".to_string());
    }
    if input.comment.as_deref().unwrap_or_default().chars().count() > 2_000 {
        return Err("The reminder comment is too long".to_string());
    }
    if input.start_offset < 0 || input.end_offset <= input.start_offset {
        return Err("The selected note range is invalid".to_string());
    }
    Ok(())
}

fn reminder_query(where_clause: &str) -> String {
    format!(
        "
        SELECT r.id, r.note_id, n.title, r.scheduled_at, r.comment,
               r.selected_text, r.start_offset, r.end_offset,
               r.context_before, r.context_after, r.status,
               r.last_notified_at, r.created_at, r.updated_at
        FROM reminders r
        JOIN notes n ON n.id = r.note_id
        {where_clause}
        "
    )
}

fn read_reminder(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReminderRecord> {
    Ok(ReminderRecord {
        id: row.get(0)?,
        note_id: row.get(1)?,
        note_title: row.get(2)?,
        scheduled_at: row.get(3)?,
        comment: row.get(4)?,
        selected_text: row.get(5)?,
        start_offset: row.get(6)?,
        end_offset: row.get(7)?,
        context_before: row.get(8)?,
        context_after: row.get(9)?,
        status: row.get(10)?,
        last_notified_at: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

fn load_one(connection: &Connection, id: &str) -> Result<ReminderRecord, String> {
    connection
        .query_row(
            &reminder_query("WHERE r.id = ?1"),
            params![id],
            read_reminder,
        )
        .map_err(db_error)
}

pub(crate) fn load_by_id(connection: &Connection, id: &str) -> Result<ReminderRecord, String> {
    load_one(connection, id)
}

#[tauri::command]
pub(crate) fn create_reminder(
    app: AppHandle,
    input: CreateReminderInput,
) -> Result<ReminderRecord, String> {
    validate_input(&input)?;
    let id = new_id("reminder");
    let now = now_string();
    let comment = input
        .comment
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let mut connection = open_database(&app)?;
    let transaction = connection.transaction().map_err(db_error)?;
    transaction
        .execute(
            "
            INSERT INTO reminders (
                id, note_id, scheduled_at, comment, selected_text,
                start_offset, end_offset, context_before, context_after,
                status, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'pending', ?10, ?10)
            ",
            params![
                id,
                input.note_id,
                input.scheduled_at,
                comment,
                input.selected_text,
                input.start_offset,
                input.end_offset,
                input.context_before,
                input.context_after,
                now,
            ],
        )
        .map_err(db_error)?;
    reminder_workflows::insert_workflow(&transaction, &id, &input.workflow_steps)?;
    transaction.commit().map_err(db_error)?;
    load_one(&connection, &id)
}

#[tauri::command]
pub(crate) fn list_reminders(app: AppHandle) -> Result<Vec<ReminderRecord>, String> {
    let connection = open_database(&app)?;
    let query = reminder_query(
        "ORDER BY CASE r.status WHEN 'pending' THEN 0 ELSE 1 END,
                  r.scheduled_at ASC, r.updated_at DESC",
    );
    let mut statement = connection.prepare(&query).map_err(db_error)?;
    let reminders = statement
        .query_map([], read_reminder)
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    Ok(reminders)
}

#[tauri::command]
pub(crate) fn list_due_reminders(app: AppHandle) -> Result<Vec<ReminderRecord>, String> {
    let connection = open_database(&app)?;
    due_reminders(&connection, current_time_ms(), false)
}

#[tauri::command]
pub(crate) fn complete_reminder(app: AppHandle, id: String) -> Result<(), String> {
    set_status(&app, &id, "completed")
}

#[tauri::command]
pub(crate) fn dismiss_reminder(app: AppHandle, id: String) -> Result<(), String> {
    set_status(&app, &id, "dismissed")
}

#[tauri::command]
pub(crate) fn snooze_reminder(
    app: AppHandle,
    input: SnoozeReminderInput,
) -> Result<ReminderRecord, String> {
    let connection = open_database(&app)?;
    let changed = connection
        .execute(
            "
            UPDATE reminders
            SET scheduled_at = ?2, status = 'pending', last_notified_at = NULL,
                updated_at = ?3
            WHERE id = ?1
            ",
            params![input.id, input.scheduled_at, now_string()],
        )
        .map_err(db_error)?;
    if changed == 0 {
        return Err("Reminder not found".to_string());
    }
    load_one(&connection, &input.id)
}

#[tauri::command]
pub(crate) fn delete_reminder(app: AppHandle, id: String) -> Result<(), String> {
    let connection = open_database(&app)?;
    connection
        .execute("DELETE FROM reminders WHERE id = ?1", params![id])
        .map_err(db_error)?;
    Ok(())
}

fn set_status(app: &AppHandle, id: &str, status: &str) -> Result<(), String> {
    let connection = open_database(app)?;
    let changed = connection
        .execute(
            "UPDATE reminders SET status = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, status, now_string()],
        )
        .map_err(db_error)?;
    if changed == 0 {
        return Err("Reminder not found".to_string());
    }
    connection
        .execute(
            "UPDATE reminder_workflows
             SET status = 'cancelled', updated_at = ?2
             WHERE reminder_id = ?1
               AND status IN ('scheduled', 'running', 'awaiting_approval')",
            params![id, now_string()],
        )
        .map_err(db_error)?;
    connection
        .execute(
            "UPDATE reminder_workflow_steps
             SET status = 'cancelled', updated_at = ?2
             WHERE workflow_id IN (
                 SELECT id FROM reminder_workflows WHERE reminder_id = ?1
             ) AND status IN ('pending', 'running', 'awaiting_approval')",
            params![id, now_string()],
        )
        .map_err(db_error)?;
    Ok(())
}

fn current_time_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn due_reminders(
    connection: &Connection,
    now: i64,
    only_undelivered: bool,
) -> Result<Vec<ReminderRecord>, String> {
    let delivery_filter = if only_undelivered {
        "AND r.last_notified_at IS NULL"
    } else {
        ""
    };
    let query = reminder_query(&format!(
        "WHERE r.status = 'pending' AND r.scheduled_at <= ?1 {delivery_filter}
         ORDER BY r.scheduled_at ASC"
    ));
    let mut statement = connection.prepare(&query).map_err(db_error)?;
    let reminders = statement
        .query_map(params![now], read_reminder)
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    Ok(reminders)
}

fn deliver_due(app: &AppHandle) -> Result<(), String> {
    let mut connection = open_database(app)?;
    let now = current_time_ms();
    let due = due_reminders(&connection, now, true)?;
    if due.is_empty() {
        return Ok(());
    }

    let transaction = connection.transaction().map_err(db_error)?;
    for reminder in &due {
        transaction
            .execute(
                "UPDATE reminders SET last_notified_at = ?2 WHERE id = ?1",
                params![reminder.id, now],
            )
            .map_err(db_error)?;
    }
    transaction.commit().map_err(db_error)?;

    for mut reminder in due {
        reminder.last_notified_at = Some(now);
        app.emit("reminder-due", reminder)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub(crate) async fn worker(app: AppHandle) {
    // Give the webview time to register its event listener before the first
    // overdue delivery is emitted during application startup.
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    loop {
        if let Err(error) = deliver_due(&app) {
            eprintln!("[smooth:reminder-worker] {error}");
        }
        tokio::time::sleep(std::time::Duration::from_secs(DELIVERY_POLL_SECONDS)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Connection {
        let connection = Connection::open_in_memory().expect("open database");
        connection
            .execute_batch(
                "
                PRAGMA foreign_keys = ON;
                CREATE TABLE notes (id TEXT PRIMARY KEY, title TEXT NOT NULL);
                INSERT INTO notes (id, title) VALUES ('note-1', 'A note');
                ",
            )
            .expect("notes schema");
        init_schema(&connection).expect("reminder schema");
        connection
    }

    #[test]
    fn due_query_excludes_completed_and_future_reminders() {
        let connection = database();
        connection
            .execute_batch(
                "
                INSERT INTO reminders VALUES
                  ('due', 'note-1', 100, NULL, 'selected', 1, 9, '', '', 'pending', NULL, '1', '1'),
                  ('future', 'note-1', 300, NULL, 'future', 1, 7, '', '', 'pending', NULL, '1', '1'),
                  ('done', 'note-1', 100, NULL, 'done', 1, 5, '', '', 'completed', NULL, '1', '1');
                ",
            )
            .expect("seed reminders");

        let reminders = due_reminders(&connection, 200, false).expect("query due");
        assert_eq!(reminders.len(), 1);
        assert_eq!(reminders[0].id, "due");
    }

    #[test]
    fn delivered_filter_keeps_overdue_reminders_available_to_the_ui() {
        let connection = database();
        connection
            .execute_batch(
                "
                INSERT INTO reminders VALUES
                  ('shown', 'note-1', 100, NULL, 'selected', 1, 9, '', '', 'pending', 150, '1', '1');
                ",
            )
            .expect("seed reminder");

        assert_eq!(
            due_reminders(&connection, 200, false)
                .expect("all due")
                .len(),
            1
        );
        assert!(due_reminders(&connection, 200, true)
            .expect("undelivered due")
            .is_empty());
    }
}
