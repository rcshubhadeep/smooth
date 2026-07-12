mod handler;
mod server;

use std::sync::{Arc, RwLock};

use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::{agents::AgentRuntime, db_error, now_string, open_database};

pub(crate) const MCP_PORT: u16 = 17_843;
const TOKEN_KEY: &str = "mcp_bearer_token";

#[derive(Clone)]
pub(crate) struct McpConfig {
    pub(crate) token: String,
}

/// Shared with the running axum server so a token change made through
/// `set_mcp_bearer_token` takes effect on the next request, without a restart.
#[derive(Clone)]
pub(crate) struct McpAuthState(pub(crate) Arc<RwLock<String>>);

#[derive(Serialize)]
pub(crate) struct McpStatus {
    enabled: bool,
    endpoint: String,
    bearer_token: String,
    read_only: bool,
    tools: Vec<String>,
}

fn random_token() -> String {
    let mut bytes = [0_u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn save_token(connection: &Connection, token: &str) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO app_meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![TOKEN_KEY, token],
        )
        .map_err(db_error)?;
    Ok(())
}

pub(crate) fn init_schema(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "
            CREATE TABLE IF NOT EXISTS mcp_audit_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tool_name TEXT NOT NULL,
                input_json TEXT NOT NULL,
                success INTEGER NOT NULL,
                error TEXT,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS mcp_audit_events_created_idx
                ON mcp_audit_events(created_at);
            ",
        )
        .map_err(db_error)
}

pub(crate) fn load_or_create_config(app: &AppHandle) -> Result<McpConfig, String> {
    let connection = open_database(app)?;
    let saved = connection
        .query_row(
            "SELECT value FROM app_meta WHERE key = ?1",
            params![TOKEN_KEY],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(db_error)?;
    let token = match saved {
        Some(token) if !token.trim().is_empty() => token,
        _ => {
            let token = random_token();
            save_token(&connection, &token)?;
            token
        }
    };
    Ok(McpConfig { token })
}

pub(crate) fn record_call(
    app: &AppHandle,
    tool: &str,
    input: &serde_json::Value,
    error: Option<&str>,
) {
    if let Ok(connection) = open_database(app) {
        let _ = connection.execute(
            "INSERT INTO mcp_audit_events
             (tool_name, input_json, success, error, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                tool,
                input.to_string(),
                error.is_none(),
                error,
                now_string()
            ],
        );
    }
}

pub(crate) fn start(app: AppHandle) -> Result<(), String> {
    let config = load_or_create_config(&app)?;
    let auth_state = McpAuthState(Arc::new(RwLock::new(config.token)));
    app.manage(auth_state.clone());
    let runtime = app.state::<AgentRuntime>().inner().clone();
    tauri::async_runtime::spawn(server::run(app, runtime, auth_state));
    Ok(())
}

fn mcp_status(token: String) -> McpStatus {
    McpStatus {
        enabled: true,
        endpoint: format!("http://127.0.0.1:{MCP_PORT}/mcp"),
        bearer_token: token,
        read_only: true,
        tools: handler::READ_ONLY_TOOLS
            .iter()
            .map(|tool| (*tool).to_string())
            .collect(),
    }
}

#[tauri::command]
pub(crate) fn get_mcp_status(app: AppHandle) -> Result<McpStatus, String> {
    let token = app
        .state::<McpAuthState>()
        .0
        .read()
        .map_err(|_| "MCP auth state is poisoned".to_string())?
        .clone();
    Ok(mcp_status(token))
}

#[tauri::command]
pub(crate) fn set_mcp_bearer_token(
    app: AppHandle,
    token: Option<String>,
) -> Result<McpStatus, String> {
    let token = token
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(random_token);

    let connection = open_database(&app)?;
    save_token(&connection, &token)?;

    let auth_state = app.state::<McpAuthState>();
    *auth_state
        .0
        .write()
        .map_err(|_| "MCP auth state is poisoned".to_string())? = token.clone();

    Ok(mcp_status(token))
}
