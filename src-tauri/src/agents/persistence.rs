//! Persistence for foreground/background agent execution.
//!
//! The schema is deliberately append-friendly: `agent_runs` stores the current
//! lifecycle summary, while `agent_events` stores the ordered trace. Future
//! workers, approvals and schedulers can reuse the same event log without
//! changing tool implementations.

use rusqlite::{named_params, params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::AppHandle;

use crate::{db_error, new_id, now_string, open_database};

pub(crate) fn init_schema(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "
            CREATE TABLE IF NOT EXISTS agent_runs (
                id TEXT PRIMARY KEY,
                run_kind TEXT NOT NULL DEFAULT 'foreground',
                status TEXT NOT NULL
                    CHECK (status IN ('running', 'succeeded', 'failed', 'cancelled')),
                prompt TEXT NOT NULL,
                model TEXT,
                base_url TEXT,
                max_steps INTEGER NOT NULL,
                answer TEXT,
                raw_model_output TEXT,
                error TEXT,
                started_at TEXT NOT NULL,
                completed_at TEXT,
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS agent_runs_status_idx
                ON agent_runs(status, updated_at);

            CREATE INDEX IF NOT EXISTS agent_runs_started_at_idx
                ON agent_runs(started_at);

            CREATE TABLE IF NOT EXISTS agent_events (
                id TEXT PRIMARY KEY,
                run_id TEXT NOT NULL REFERENCES agent_runs(id) ON DELETE CASCADE,
                sequence INTEGER NOT NULL,
                event_type TEXT NOT NULL,
                role TEXT,
                tool_name TEXT,
                content TEXT,
                input_json TEXT,
                output_json TEXT,
                error TEXT,
                created_at TEXT NOT NULL,
                UNIQUE (run_id, sequence)
            );

            CREATE INDEX IF NOT EXISTS agent_events_run_sequence_idx
                ON agent_events(run_id, sequence);

            CREATE INDEX IF NOT EXISTS agent_events_type_idx
                ON agent_events(event_type, created_at);

            -- User-defined agents (Phase 2). These are prompt presets, not a
            -- new execution path: `agent_run` still runs them. `scope`/`icon`
            -- are stored with sensible defaults so the schema can grow into
            -- per-agent scope/tool selection without a migration.
            CREATE TABLE IF NOT EXISTS agent_definitions (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                instructions TEXT NOT NULL,
                scope TEXT NOT NULL DEFAULT 'global',
                icon TEXT NOT NULL DEFAULT 'overview',
                max_steps INTEGER,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS agent_definitions_updated_idx
                ON agent_definitions(updated_at);
            ",
        )
        .map_err(db_error)?;
    migrate_agent_runs_schema(connection)
}

fn has_column(connection: &Connection, table: &str, column: &str) -> Result<bool, String> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(db_error)?;
    let mut names = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(db_error)?;
    names.try_fold(false, |found, name| {
        Ok(found || name.map_err(db_error)? == column)
    })
}

/// Tag runs with the agent that produced them. Added after the fact, so an
/// `ALTER TABLE` is needed for existing databases; old rows keep `agent_id`
/// NULL and simply do not appear under any agent in the inspect view.
fn migrate_agent_runs_schema(connection: &Connection) -> Result<(), String> {
    if !has_column(connection, "agent_runs", "agent_id")? {
        connection
            .execute("ALTER TABLE agent_runs ADD COLUMN agent_id TEXT", [])
            .map_err(db_error)?;
    }
    connection
        .execute(
            "CREATE INDEX IF NOT EXISTS agent_runs_agent_idx ON agent_runs(agent_id)",
            [],
        )
        .map_err(db_error)?;
    Ok(())
}

pub(crate) struct AgentRunRecorder {
    app: AppHandle,
    run_id: String,
    sequence: i64,
}

#[derive(Debug, Serialize)]
pub(crate) struct AgentRunRecord {
    pub(crate) id: String,
    pub(crate) agent_id: Option<String>,
    pub(crate) run_kind: String,
    pub(crate) status: String,
    pub(crate) prompt: String,
    pub(crate) model: Option<String>,
    pub(crate) max_steps: i64,
    pub(crate) answer: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) started_at: String,
    pub(crate) completed_at: Option<String>,
    pub(crate) updated_at: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct AgentEventRecord {
    pub(crate) id: String,
    pub(crate) run_id: String,
    pub(crate) sequence: i64,
    pub(crate) event_type: String,
    pub(crate) role: Option<String>,
    pub(crate) tool_name: Option<String>,
    pub(crate) content: Option<String>,
    pub(crate) input_json: Option<Value>,
    pub(crate) output_json: Option<Value>,
    pub(crate) error: Option<String>,
    pub(crate) created_at: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AgentRunListOptions {
    pub(crate) limit: Option<u32>,
    /// When set, only runs produced by this agent are returned.
    pub(crate) agent_id: Option<String>,
}

pub(crate) struct AgentEvent<'a> {
    pub(crate) event_type: &'a str,
    pub(crate) role: Option<&'a str>,
    pub(crate) tool_name: Option<&'a str>,
    pub(crate) content: Option<&'a str>,
    pub(crate) input: Option<&'a Value>,
    pub(crate) output: Option<&'a Value>,
    pub(crate) error: Option<&'a str>,
}

impl AgentRunRecorder {
    pub(crate) fn start(
        app: AppHandle,
        agent_id: Option<&str>,
        run_kind: &str,
        prompt: &str,
        max_steps: u8,
    ) -> Result<Self, String> {
        let run_id = format!("{}-{}", new_id("agent-run"), std::process::id());
        let started_at = now_string();
        let connection = open_database(&app)?;
        connection
            .execute(
                "
                INSERT INTO agent_runs (
                    id, agent_id, run_kind, status, prompt, max_steps,
                    started_at, updated_at
                )
                VALUES (?1, ?2, ?3, 'running', ?4, ?5, ?6, ?6)
                ",
                params![run_id, agent_id, run_kind, prompt, max_steps, started_at],
            )
            .map_err(db_error)?;

        Ok(Self {
            app,
            run_id,
            sequence: 0,
        })
    }

    pub(crate) fn run_id(&self) -> &str {
        &self.run_id
    }

    pub(crate) fn set_model(&self, model: &str, base_url: &str) -> Result<(), String> {
        let now = now_string();
        let connection = open_database(&self.app)?;
        connection
            .execute(
                "
                UPDATE agent_runs
                SET model = ?1, base_url = ?2, updated_at = ?3
                WHERE id = ?4
                ",
                params![model, base_url, now, self.run_id],
            )
            .map_err(db_error)?;
        Ok(())
    }

    pub(crate) fn record(&mut self, event: AgentEvent<'_>) -> Result<(), String> {
        self.sequence += 1;
        let event_id = format!("{}-{}", new_id("agent-event"), self.sequence);
        let now = now_string();
        let input_json = json_to_string(event.input)?;
        let output_json = json_to_string(event.output)?;
        let connection = open_database(&self.app)?;
        connection
            .execute(
                "
                INSERT INTO agent_events (
                    id, run_id, sequence, event_type, role, tool_name, content,
                    input_json, output_json, error, created_at
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                ",
                params![
                    event_id,
                    self.run_id,
                    self.sequence,
                    event.event_type,
                    event.role,
                    event.tool_name,
                    event.content,
                    input_json,
                    output_json,
                    event.error,
                    now
                ],
            )
            .map_err(db_error)?;
        Ok(())
    }

    pub(crate) fn complete_success(
        &self,
        answer: &str,
        raw_model_output: &str,
    ) -> Result<(), String> {
        let now = now_string();
        let connection = open_database(&self.app)?;
        connection
            .execute(
                "
                UPDATE agent_runs
                SET status = 'succeeded',
                    answer = ?1,
                    raw_model_output = ?2,
                    error = NULL,
                    completed_at = ?3,
                    updated_at = ?3
                WHERE id = ?4
                ",
                params![answer, raw_model_output, now, self.run_id],
            )
            .map_err(db_error)?;
        Ok(())
    }

    pub(crate) fn complete_failure(&self, error: &str) -> Result<(), String> {
        let now = now_string();
        let connection = open_database(&self.app)?;
        connection
            .execute(
                "
                UPDATE agent_runs
                SET status = 'failed',
                    error = ?1,
                    completed_at = ?2,
                    updated_at = ?2
                WHERE id = ?3
                ",
                params![error, now, self.run_id],
            )
            .map_err(db_error)?;
        Ok(())
    }
}

fn json_to_string(value: Option<&Value>) -> Result<Option<String>, String> {
    value
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| error.to_string())
}

fn row_to_run_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentRunRecord> {
    Ok(AgentRunRecord {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        run_kind: row.get(2)?,
        status: row.get(3)?,
        prompt: row.get(4)?,
        model: row.get(5)?,
        max_steps: row.get(6)?,
        answer: row.get(7)?,
        error: row.get(8)?,
        started_at: row.get(9)?,
        completed_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

const RUN_COLUMNS: &str = "id, agent_id, run_kind, status, prompt, model, \
    max_steps, answer, error, started_at, completed_at, updated_at";

pub(crate) fn list_runs(
    app: AppHandle,
    options: Option<AgentRunListOptions>,
) -> Result<Vec<AgentRunRecord>, String> {
    let (limit, agent_id) = match options {
        Some(options) => (options.limit.unwrap_or(50).clamp(1, 200), options.agent_id),
        None => (50, None),
    };
    let connection = open_database(&app)?;
    let mut sql = format!("SELECT {RUN_COLUMNS} FROM agent_runs");
    if agent_id.is_some() {
        sql.push_str(" WHERE agent_id = :agent_id");
    }
    sql.push_str(" ORDER BY CAST(started_at AS INTEGER) DESC LIMIT :limit");

    let mut statement = connection.prepare(&sql).map_err(db_error)?;
    let mapped = match agent_id.as_deref() {
        Some(agent_id) => statement.query_map(
            named_params![":agent_id": agent_id, ":limit": limit],
            row_to_run_record,
        ),
        None => statement.query_map(named_params![":limit": limit], row_to_run_record),
    };
    let rows = mapped
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    Ok(rows)
}

pub(crate) fn get_events(app: AppHandle, run_id: String) -> Result<Vec<AgentEventRecord>, String> {
    let connection = open_database(&app)?;
    let mut statement = connection
        .prepare(
            "
            SELECT id, run_id, sequence, event_type, role, tool_name, content,
                   input_json, output_json, error, created_at
            FROM agent_events
            WHERE run_id = ?1
            ORDER BY sequence ASC
            ",
        )
        .map_err(db_error)?;
    let rows = statement
        .query_map(params![run_id], |row| {
            Ok(AgentEventRecord {
                id: row.get(0)?,
                run_id: row.get(1)?,
                sequence: row.get(2)?,
                event_type: row.get(3)?,
                role: row.get(4)?,
                tool_name: row.get(5)?,
                content: row.get(6)?,
                input_json: parse_json_column(row.get::<_, Option<String>>(7)?),
                output_json: parse_json_column(row.get::<_, Option<String>>(8)?),
                error: row.get(9)?,
                created_at: row.get(10)?,
            })
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    Ok(rows)
}

fn parse_json_column(value: Option<String>) -> Option<Value> {
    value.and_then(|value| serde_json::from_str(&value).ok())
}

// ---------------------------------------------------------------------------
// User-defined agent definitions (Phase 2)
// ---------------------------------------------------------------------------

/// A persisted user-defined agent. `scope`/`icon` are carried through with
/// defaults so the frontend renders them like built-ins and we can expose
/// editing them later without a schema change.
#[derive(Debug, Serialize)]
pub(crate) struct AgentDefinitionRecord {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) instructions: String,
    pub(crate) scope: String,
    pub(crate) icon: String,
    pub(crate) max_steps: Option<i64>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

/// Fields accepted from the frontend when creating or updating an agent.
/// Only name/description/instructions/max_steps are user-editable for now.
#[derive(Debug, Deserialize)]
pub(crate) struct AgentDefinitionInput {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) description: String,
    pub(crate) instructions: String,
    #[serde(default)]
    pub(crate) max_steps: Option<i64>,
    #[serde(default)]
    pub(crate) scope: Option<String>,
    #[serde(default)]
    pub(crate) icon: Option<String>,
}

/// Validate + normalize user input. Keeping this in one place means both create
/// and update enforce the same rules.
fn clean_definition(
    input: &AgentDefinitionInput,
) -> Result<
    (
        String,
        String,
        String,
        Option<i64>,
        Option<String>,
        Option<String>,
    ),
    String,
> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err("Agent name is required".to_string());
    }
    let instructions = input.instructions.trim();
    if instructions.is_empty() {
        return Err("Agent instructions are required".to_string());
    }
    let max_steps = input.max_steps.map(|value| value.clamp(1, 6));
    let scope = match input.scope.as_deref().map(str::trim) {
        None | Some("") => None,
        Some("note") => Some("note".to_string()),
        Some("global") => Some("global".to_string()),
        Some(_) => return Err("Agent scope must be 'note' or 'global'".to_string()),
    };
    let icon = input
        .icon
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    Ok((
        name.to_string(),
        input.description.trim().to_string(),
        instructions.to_string(),
        max_steps,
        scope,
        icon,
    ))
}

fn map_definition_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentDefinitionRecord> {
    Ok(AgentDefinitionRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        instructions: row.get(3)?,
        scope: row.get(4)?,
        icon: row.get(5)?,
        max_steps: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

const DEFINITION_COLUMNS: &str =
    "id, name, description, instructions, scope, icon, max_steps, created_at, updated_at";

pub(crate) fn list_definitions(app: AppHandle) -> Result<Vec<AgentDefinitionRecord>, String> {
    let connection = open_database(&app)?;
    let sql = format!(
        "SELECT {DEFINITION_COLUMNS} FROM agent_definitions ORDER BY CAST(created_at AS INTEGER) ASC"
    );
    let mut statement = connection.prepare(&sql).map_err(db_error)?;
    let rows = statement
        .query_map([], map_definition_row)
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    Ok(rows)
}

pub(crate) fn create_definition(
    app: AppHandle,
    input: AgentDefinitionInput,
) -> Result<AgentDefinitionRecord, String> {
    let (name, description, instructions, max_steps, scope, icon) = clean_definition(&input)?;
    let id = new_id("agent");
    let now = now_string();

    let connection = open_database(&app)?;
    connection
        .execute(
            "INSERT INTO agent_definitions
                (id, name, description, instructions, scope, icon, max_steps, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
            params![
                id,
                name,
                description,
                instructions,
                scope.as_deref().unwrap_or("global"),
                icon.as_deref().unwrap_or("overview"),
                max_steps,
                now
            ],
        )
        .map_err(db_error)?;

    read_definition(&connection, &id)
}

pub(crate) fn update_definition(
    app: AppHandle,
    id: String,
    input: AgentDefinitionInput,
) -> Result<AgentDefinitionRecord, String> {
    let (name, description, instructions, max_steps, scope, icon) = clean_definition(&input)?;
    let now = now_string();

    let connection = open_database(&app)?;
    let changed = connection
        .execute(
            "UPDATE agent_definitions
             SET name = ?2, description = ?3, instructions = ?4, max_steps = ?5,
                 scope = COALESCE(?6, scope), icon = COALESCE(?7, icon), updated_at = ?8
             WHERE id = ?1",
            params![
                id,
                name,
                description,
                instructions,
                max_steps,
                scope,
                icon,
                now
            ],
        )
        .map_err(db_error)?;
    if changed == 0 {
        return Err("Agent not found".to_string());
    }

    read_definition(&connection, &id)
}

pub(crate) fn delete_definition(app: AppHandle, id: String) -> Result<(), String> {
    let connection = open_database(&app)?;
    let changed = connection
        .execute("DELETE FROM agent_definitions WHERE id = ?1", params![id])
        .map_err(db_error)?;
    if changed == 0 {
        return Err("Agent not found".to_string());
    }
    Ok(())
}

fn read_definition(connection: &Connection, id: &str) -> Result<AgentDefinitionRecord, String> {
    let sql = format!("SELECT {DEFINITION_COLUMNS} FROM agent_definitions WHERE id = ?1");
    connection
        .query_row(&sql, params![id], map_definition_row)
        .map_err(db_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_note_scoped_task_definitions() {
        let input = AgentDefinitionInput {
            name: "Find objections".to_string(),
            description: "Created from chat".to_string(),
            instructions: "Were any objections raised?".to_string(),
            max_steps: Some(3),
            scope: Some("note".to_string()),
            icon: Some("overview".to_string()),
        };

        let (_, _, _, max_steps, scope, icon) =
            clean_definition(&input).expect("valid task definition");
        assert_eq!(max_steps, Some(3));
        assert_eq!(scope.as_deref(), Some("note"));
        assert_eq!(icon.as_deref(), Some("overview"));
    }

    #[test]
    fn init_schema_creates_agent_tables() {
        let connection = Connection::open_in_memory().expect("open test database");

        init_schema(&connection).expect("initialize schema");

        let run_table: String = connection
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'agent_runs'",
                [],
                |row| row.get(0),
            )
            .expect("agent_runs table");
        let event_table: String = connection
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'agent_events'",
                [],
                |row| row.get(0),
            )
            .expect("agent_events table");

        assert_eq!(run_table, "agent_runs");
        assert_eq!(event_table, "agent_events");
    }
}
