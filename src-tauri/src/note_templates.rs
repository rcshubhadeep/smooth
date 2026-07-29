use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::{db_error, new_id, now_string, open_database};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NoteTemplate {
    id: String,
    name: String,
    content: String,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NoteTemplateInput {
    name: String,
    content: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MeetingTemplateAssignment {
    transcript_note_id: String,
    template_id: Option<String>,
    template_name: String,
    template_content: String,
    assigned_at: String,
}

pub(crate) fn init_schema(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "
            CREATE TABLE IF NOT EXISTS note_templates (
                id TEXT PRIMARY KEY NOT NULL,
                name TEXT NOT NULL COLLATE NOCASE UNIQUE,
                content TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS meeting_note_template_assignments (
                transcript_note_id TEXT PRIMARY KEY NOT NULL
                    REFERENCES notes(id) ON DELETE CASCADE,
                template_id TEXT REFERENCES note_templates(id) ON DELETE SET NULL,
                template_name TEXT NOT NULL,
                template_content TEXT NOT NULL,
                assigned_at TEXT NOT NULL,
                discarded_at TEXT
            );
            ",
        )
        .map_err(db_error)
}

#[tauri::command]
pub(crate) fn list_note_templates(app: AppHandle) -> Result<Vec<NoteTemplate>, String> {
    let connection = open_database(&app)?;
    list_templates(&connection)
}

#[tauri::command]
pub(crate) fn create_note_template(
    app: AppHandle,
    input: NoteTemplateInput,
) -> Result<NoteTemplate, String> {
    let connection = open_database(&app)?;
    let (name, content) = validate_input(input)?;
    let id = new_id("note-template");
    let now = now_string();
    connection
        .execute(
            "INSERT INTO note_templates (id, name, content, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4)",
            params![id, name, content, now],
        )
        .map_err(template_write_error)?;
    load_template(&connection, &id)?
        .ok_or_else(|| "The note template could not be loaded after creation".to_string())
}

#[tauri::command]
pub(crate) fn update_note_template(
    app: AppHandle,
    id: String,
    input: NoteTemplateInput,
) -> Result<NoteTemplate, String> {
    let connection = open_database(&app)?;
    let (name, content) = validate_input(input)?;
    let changed = connection
        .execute(
            "UPDATE note_templates
             SET name = ?2, content = ?3, updated_at = ?4
             WHERE id = ?1",
            params![id, name, content, now_string()],
        )
        .map_err(template_write_error)?;
    if changed == 0 {
        return Err("Note template not found".to_string());
    }
    load_template(&connection, &id)?
        .ok_or_else(|| "The updated note template could not be loaded".to_string())
}

#[tauri::command]
pub(crate) fn delete_note_template(app: AppHandle, id: String) -> Result<(), String> {
    let connection = open_database(&app)?;
    let changed = connection
        .execute("DELETE FROM note_templates WHERE id = ?1", params![id])
        .map_err(db_error)?;
    if changed == 0 {
        return Err("Note template not found".to_string());
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn get_meeting_note_template_assignment(
    app: AppHandle,
    transcript_note_id: String,
) -> Result<Option<MeetingTemplateAssignment>, String> {
    let connection = open_database(&app)?;
    load_assignment(&connection, &transcript_note_id)
}

#[tauri::command]
pub(crate) fn assign_meeting_note_template(
    app: AppHandle,
    transcript_note_id: String,
    template_id: String,
) -> Result<MeetingTemplateAssignment, String> {
    let connection = open_database(&app)?;
    ensure_meeting_is_open(&connection, &transcript_note_id)?;
    let template = load_template(&connection, &template_id)?
        .ok_or_else(|| "Note template not found".to_string())?;
    let assigned_at = now_string();
    connection
        .execute(
            "
            INSERT INTO meeting_note_template_assignments (
                transcript_note_id, template_id, template_name,
                template_content, assigned_at, discarded_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, NULL)
            ON CONFLICT(transcript_note_id) DO UPDATE SET
                template_id = excluded.template_id,
                template_name = excluded.template_name,
                template_content = excluded.template_content,
                assigned_at = excluded.assigned_at,
                discarded_at = NULL
            ",
            params![
                transcript_note_id,
                template.id,
                template.name,
                template.content,
                assigned_at
            ],
        )
        .map_err(db_error)?;
    load_assignment(&connection, &transcript_note_id)?
        .ok_or_else(|| "The meeting template assignment could not be loaded".to_string())
}

#[tauri::command]
pub(crate) fn discard_meeting_note_template(
    app: AppHandle,
    transcript_note_id: String,
) -> Result<(), String> {
    let connection = open_database(&app)?;
    ensure_meeting_is_open(&connection, &transcript_note_id)?;
    let changed = connection
        .execute(
            "UPDATE meeting_note_template_assignments
             SET discarded_at = ?2
             WHERE transcript_note_id = ?1 AND discarded_at IS NULL",
            params![transcript_note_id, now_string()],
        )
        .map_err(db_error)?;
    if changed == 0 {
        return Err("This meeting does not have an active note template".to_string());
    }
    Ok(())
}

fn list_templates(connection: &Connection) -> Result<Vec<NoteTemplate>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, name, content, created_at, updated_at
             FROM note_templates
             ORDER BY name COLLATE NOCASE",
        )
        .map_err(db_error)?;
    let templates = statement
        .query_map([], read_template)
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    Ok(templates)
}

fn load_template(connection: &Connection, id: &str) -> Result<Option<NoteTemplate>, String> {
    connection
        .query_row(
            "SELECT id, name, content, created_at, updated_at
             FROM note_templates WHERE id = ?1",
            params![id],
            read_template,
        )
        .optional()
        .map_err(db_error)
}

fn read_template(row: &rusqlite::Row<'_>) -> rusqlite::Result<NoteTemplate> {
    Ok(NoteTemplate {
        id: row.get(0)?,
        name: row.get(1)?,
        content: row.get(2)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
    })
}

fn load_assignment(
    connection: &Connection,
    transcript_note_id: &str,
) -> Result<Option<MeetingTemplateAssignment>, String> {
    connection
        .query_row(
            "
            SELECT transcript_note_id, template_id, template_name,
                   template_content, assigned_at
            FROM meeting_note_template_assignments
            WHERE transcript_note_id = ?1 AND discarded_at IS NULL
            ",
            params![transcript_note_id],
            |row| {
                Ok(MeetingTemplateAssignment {
                    transcript_note_id: row.get(0)?,
                    template_id: row.get(1)?,
                    template_name: row.get(2)?,
                    template_content: row.get(3)?,
                    assigned_at: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(db_error)
}

fn ensure_meeting_is_open(connection: &Connection, note_id: &str) -> Result<(), String> {
    let open = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM meeting_occurrences
                WHERE note_id = ?1 AND completed_at_ms IS NULL
             )",
            params![note_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(db_error)?;
    if open {
        Ok(())
    } else {
        Err("Note templates can only be changed before the meeting ends".to_string())
    }
}

fn validate_input(input: NoteTemplateInput) -> Result<(String, String), String> {
    let name = input.name.trim().to_string();
    let content = input.content.trim().to_string();
    if name.is_empty() {
        return Err("Give the note template a name".to_string());
    }
    if content.is_empty() {
        return Err("Add some content to the note template".to_string());
    }
    Ok((name, content))
}

fn template_write_error(error: rusqlite::Error) -> String {
    let message = error.to_string();
    if message.contains("UNIQUE constraint failed") {
        "A note template with this name already exists".to_string()
    } else {
        message
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection() -> Connection {
        let connection = Connection::open_in_memory().expect("open database");
        connection
            .execute_batch(
                "
                PRAGMA foreign_keys = ON;
                CREATE TABLE notes (id TEXT PRIMARY KEY NOT NULL);
                CREATE TABLE meeting_occurrences (
                    note_id TEXT PRIMARY KEY NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
                    completed_at_ms INTEGER
                );
                ",
            )
            .expect("create base schema");
        init_schema(&connection).expect("create template schema");
        connection
    }

    #[test]
    fn stores_named_templates_and_preserves_assignment_snapshots() {
        let connection = connection();
        let (name, content) = validate_input(NoteTemplateInput {
            name: " Weekly review ".to_string(),
            content: " @summary\n# Decisions ".to_string(),
        })
        .expect("valid input");
        connection
            .execute(
                "INSERT INTO note_templates (id, name, content, created_at, updated_at)
                 VALUES ('template-1', ?1, ?2, 'now', 'now')",
                params![name, content],
            )
            .expect("insert template");
        connection
            .execute("INSERT INTO notes (id) VALUES ('meeting-1')", [])
            .expect("insert note");
        connection
            .execute(
                "INSERT INTO meeting_occurrences (note_id, completed_at_ms)
                 VALUES ('meeting-1', NULL)",
                [],
            )
            .expect("insert meeting");
        connection
            .execute(
                "INSERT INTO meeting_note_template_assignments (
                    transcript_note_id, template_id, template_name,
                    template_content, assigned_at
                 ) VALUES (
                    'meeting-1', 'template-1', 'Weekly review',
                    '@summary\n# Decisions', 'now'
                 )",
                [],
            )
            .expect("assign template");
        connection
            .execute(
                "UPDATE note_templates SET content = '# Changed'
                 WHERE id = 'template-1'",
                [],
            )
            .expect("edit template");

        let assignment = load_assignment(&connection, "meeting-1")
            .expect("load assignment")
            .expect("assignment");
        assert_eq!(assignment.template_name, "Weekly review");
        assert_eq!(assignment.template_content, "@summary\n# Decisions");
    }

    #[test]
    fn rejects_template_changes_after_meeting_completion() {
        let connection = connection();
        connection
            .execute("INSERT INTO notes (id) VALUES ('meeting-1')", [])
            .expect("insert note");
        connection
            .execute(
                "INSERT INTO meeting_occurrences (note_id, completed_at_ms)
                 VALUES ('meeting-1', 123)",
                [],
            )
            .expect("insert meeting");
        assert!(ensure_meeting_is_open(&connection, "meeting-1").is_err());
    }
}
