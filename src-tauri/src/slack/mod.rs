mod api;
mod notes;
mod socket;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Mutex,
};
use tauri::{AppHandle, State};

use crate::{db_error, open_database};

const APP_TOKEN_KEY: &str = "slack_app_token";
const BOT_TOKEN_KEY: &str = "slack_bot_token";

#[derive(Default)]
pub(crate) struct SlackState {
    connected: AtomicBool,
    revision: AtomicU64,
    last_error: Mutex<Option<String>>,
}

impl SlackState {
    fn set_connected(&self, connected: bool) {
        self.connected.store(connected, Ordering::Relaxed);
    }

    fn set_error(&self, error: Option<String>) {
        *self.last_error.lock().expect("Slack state lock poisoned") = error;
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct SlackConfigInput {
    app_token: String,
    bot_token: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct SlackConfigView {
    has_app_token: bool,
    has_bot_token: bool,
    connected: bool,
    last_error: Option<String>,
    folder_name: &'static str,
    trigger: &'static str,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SlackPostInput {
    destination: String,
    text: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct SlackPostResult {
    channel: String,
    ts: String,
    thread_ts: Option<String>,
}

#[derive(Clone)]
struct SlackCredentials {
    app_token: String,
    bot_token: String,
}

pub(crate) fn init_schema(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "
        CREATE TABLE IF NOT EXISTS slack_events (
            event_id TEXT PRIMARY KEY NOT NULL,
            channel_id TEXT NOT NULL,
            thread_ts TEXT NOT NULL,
            status TEXT NOT NULL CHECK(status IN ('processing', 'completed', 'failed')),
            attempts INTEGER NOT NULL DEFAULT 1,
            note_id TEXT REFERENCES notes(id) ON DELETE SET NULL,
            error TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS slack_events_status_idx
            ON slack_events(status, updated_at);
        ",
        )
        .map_err(db_error)
}

#[tauri::command]
pub(crate) fn get_slack_config(
    app: AppHandle,
    state: State<'_, SlackState>,
) -> Result<SlackConfigView, String> {
    config_view(&app, &state)
}

#[tauri::command]
pub(crate) fn save_slack_config(
    app: AppHandle,
    state: State<'_, SlackState>,
    config: SlackConfigInput,
) -> Result<SlackConfigView, String> {
    let connection = open_database(&app)?;
    let app_token = if config.app_token.trim().is_empty() {
        meta_value(&connection, APP_TOKEN_KEY)?.unwrap_or_default()
    } else {
        config.app_token.trim().to_string()
    };
    let bot_token = if config.bot_token.trim().is_empty() {
        meta_value(&connection, BOT_TOKEN_KEY)?.unwrap_or_default()
    } else {
        config.bot_token.trim().to_string()
    };
    if !app_token.starts_with("xapp-") {
        return Err("Enter a Slack Socket Mode app token beginning with xapp-".to_string());
    }
    if !bot_token.starts_with("xoxb-") {
        return Err("Enter a Slack bot token beginning with xoxb-".to_string());
    }
    drop(connection);
    let mut connection = open_database(&app)?;
    let transaction = connection.transaction().map_err(db_error)?;
    set_meta(&transaction, APP_TOKEN_KEY, &app_token)?;
    set_meta(&transaction, BOT_TOKEN_KEY, &bot_token)?;
    transaction.commit().map_err(db_error)?;
    state.set_error(None);
    state.revision.fetch_add(1, Ordering::Relaxed);
    config_view(&app, &state)
}

#[tauri::command]
pub(crate) fn clear_slack_config(
    app: AppHandle,
    state: State<'_, SlackState>,
) -> Result<SlackConfigView, String> {
    let connection = open_database(&app)?;
    connection
        .execute(
            "DELETE FROM app_meta WHERE key IN (?1, ?2)",
            params![APP_TOKEN_KEY, BOT_TOKEN_KEY],
        )
        .map_err(db_error)?;
    state.set_connected(false);
    state.set_error(None);
    state.revision.fetch_add(1, Ordering::Relaxed);
    config_view(&app, &state)
}

#[tauri::command]
pub(crate) async fn post_note_to_slack(
    app: AppHandle,
    request: SlackPostInput,
) -> Result<SlackPostResult, String> {
    post_message(app, request.destination, request.text).await
}

pub(crate) async fn post_message(
    app: AppHandle,
    destination: String,
    text: String,
) -> Result<SlackPostResult, String> {
    let destination = parse_destination(&destination)?;
    let text = text.trim();
    if text.is_empty() {
        return Err("Slack message is empty".to_string());
    }
    let credentials = load_credentials(&app)?
        .ok_or_else(|| "Configure Slack in Settings before posting".to_string())?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(25))
        .build()
        .map_err(|error| error.to_string())?;
    let posted = api::post_message(
        &client,
        &credentials.bot_token,
        &destination.channel,
        destination.thread_ts.as_deref(),
        text,
    )
    .await?;
    Ok(SlackPostResult {
        channel: posted.channel,
        ts: posted.ts,
        thread_ts: destination.thread_ts,
    })
}

struct SlackDestination {
    channel: String,
    thread_ts: Option<String>,
}

fn parse_destination(value: &str) -> Result<SlackDestination, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("Enter a Slack channel ID or message URL".to_string());
    }
    if !value.contains("://") {
        return Ok(SlackDestination {
            channel: value.to_string(),
            thread_ts: None,
        });
    }
    let url = reqwest::Url::parse(value).map_err(|_| "Invalid Slack URL".to_string())?;
    let parts = url
        .path_segments()
        .map(|segments| segments.collect::<Vec<_>>())
        .unwrap_or_default();
    let archives = parts.iter().position(|part| *part == "archives");
    let channel = archives
        .and_then(|index| parts.get(index + 1))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Slack URL does not contain a channel ID".to_string())?
        .to_string();
    let query_thread = url
        .query_pairs()
        .find(|(key, _)| key == "thread_ts")
        .map(|(_, value)| value.into_owned());
    let path_thread = archives
        .and_then(|index| parts.get(index + 2))
        .and_then(|part| part.strip_prefix('p'))
        .and_then(slack_path_timestamp);
    Ok(SlackDestination {
        channel,
        thread_ts: query_thread.or(path_thread),
    })
}

fn slack_path_timestamp(value: &str) -> Option<String> {
    if value.len() <= 6 || !value.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    let split = value.len() - 6;
    Some(format!("{}.{}", &value[..split], &value[split..]))
}

fn config_view(app: &AppHandle, state: &SlackState) -> Result<SlackConfigView, String> {
    let connection = open_database(app)?;
    Ok(SlackConfigView {
        has_app_token: meta_value(&connection, APP_TOKEN_KEY)?.is_some_and(|v| !v.is_empty()),
        has_bot_token: meta_value(&connection, BOT_TOKEN_KEY)?.is_some_and(|v| !v.is_empty()),
        connected: state.connected.load(Ordering::Relaxed),
        last_error: state
            .last_error
            .lock()
            .expect("Slack state lock poisoned")
            .clone(),
        folder_name: notes::SLACK_FOLDER_NAME,
        trigger: "@smooth create note (inside a thread)",
    })
}

fn load_credentials(app: &AppHandle) -> Result<Option<SlackCredentials>, String> {
    let connection = open_database(app)?;
    let app_token = meta_value(&connection, APP_TOKEN_KEY)?.filter(|v| !v.trim().is_empty());
    let bot_token = meta_value(&connection, BOT_TOKEN_KEY)?.filter(|v| !v.trim().is_empty());
    Ok(match (app_token, bot_token) {
        (Some(app_token), Some(bot_token)) => Some(SlackCredentials {
            app_token,
            bot_token,
        }),
        _ => None,
    })
}

fn meta_value(connection: &Connection, key: &str) -> Result<Option<String>, String> {
    connection
        .query_row(
            "SELECT value FROM app_meta WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error)
}

fn set_meta(transaction: &rusqlite::Transaction<'_>, key: &str, value: &str) -> Result<(), String> {
    transaction
        .execute(
            "INSERT INTO app_meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )
        .map_err(db_error)?;
    Ok(())
}

pub(crate) async fn worker(app: AppHandle) {
    socket::run(app).await;
}

#[cfg(test)]
mod tests {
    use super::parse_destination;

    #[test]
    fn parses_channel_id_and_message_url() {
        let channel = parse_destination("C123ABC").unwrap();
        assert_eq!(channel.channel, "C123ABC");
        assert!(channel.thread_ts.is_none());

        let message =
            parse_destination("https://workspace.slack.com/archives/C123ABC/p1712345678901234")
                .unwrap();
        assert_eq!(message.channel, "C123ABC");
        assert_eq!(message.thread_ts.as_deref(), Some("1712345678.901234"));
    }
}
