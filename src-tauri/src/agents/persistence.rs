//! Persistence for foreground/background agent execution.
//!
//! The schema is deliberately append-friendly: `agent_runs` stores the current
//! lifecycle summary, while `agent_events` stores the ordered trace. Future
//! workers, approvals and schedulers can reuse the same event log without
//! changing tool implementations.

use rusqlite::{params, Connection};
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
            ",
        )
        .map_err(db_error)
}

pub(crate) struct AgentRunRecorder {
    app: AppHandle,
    run_id: String,
    sequence: i64,
}

#[derive(Debug, Serialize)]
pub(crate) struct AgentRunRecord {
    pub(crate) id: String,
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
    pub(crate) fn start(app: AppHandle, prompt: &str, max_steps: u8) -> Result<Self, String> {
        let run_id = format!("{}-{}", new_id("agent-run"), std::process::id());
        let started_at = now_string();
        let connection = open_database(&app)?;
        connection
            .execute(
                "
                INSERT INTO agent_runs (
                    id, run_kind, status, prompt, max_steps, started_at, updated_at
                )
                VALUES (?1, 'foreground', 'running', ?2, ?3, ?4, ?4)
                ",
                params![run_id, prompt, max_steps, started_at],
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

pub(crate) fn list_runs(
    app: AppHandle,
    options: Option<AgentRunListOptions>,
) -> Result<Vec<AgentRunRecord>, String> {
    let limit = options
        .and_then(|options| options.limit)
        .unwrap_or(50)
        .clamp(1, 200);
    let connection = open_database(&app)?;
    let mut statement = connection
        .prepare(
            "
            SELECT id, run_kind, status, prompt, model, max_steps, answer, error,
                   started_at, completed_at, updated_at
            FROM agent_runs
            ORDER BY CAST(started_at AS INTEGER) DESC
            LIMIT ?1
            ",
        )
        .map_err(db_error)?;
    let rows = statement
        .query_map(params![limit], |row| {
            Ok(AgentRunRecord {
                id: row.get(0)?,
                run_kind: row.get(1)?,
                status: row.get(2)?,
                prompt: row.get(3)?,
                model: row.get(4)?,
                max_steps: row.get(5)?,
                answer: row.get(6)?,
                error: row.get(7)?,
                started_at: row.get(8)?,
                completed_at: row.get(9)?,
                updated_at: row.get(10)?,
            })
        })
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

#[cfg(test)]
mod tests {
    use super::*;

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
