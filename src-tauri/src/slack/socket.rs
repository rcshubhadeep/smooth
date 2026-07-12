use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use super::{api, load_credentials, notes, SlackState};

#[derive(Debug, Deserialize)]
struct Envelope {
    envelope_id: Option<String>,
    #[serde(rename = "type")]
    kind: String,
    payload: Option<EventPayload>,
}

#[derive(Debug, Deserialize)]
struct EventPayload {
    event_id: Option<String>,
    event: Option<AppMentionEvent>,
}

#[derive(Debug, Deserialize)]
struct AppMentionEvent {
    #[serde(rename = "type")]
    kind: String,
    channel: String,
    text: String,
    ts: String,
    thread_ts: Option<String>,
}

pub(crate) async fn run(app: AppHandle) {
    let client = Client::builder()
        .timeout(Duration::from_secs(25))
        .build()
        .expect("create Slack HTTP client");
    loop {
        let state = app.state::<SlackState>();
        let credentials = match load_credentials(&app) {
            Ok(Some(credentials)) => credentials,
            Ok(None) => {
                state.set_connected(false);
                tokio::time::sleep(Duration::from_secs(3)).await;
                continue;
            }
            Err(error) => {
                state.set_error(Some(error));
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        let revision = state.revision.load(std::sync::atomic::Ordering::Relaxed);
        match run_connection(
            &app,
            &client,
            &credentials.app_token,
            &credentials.bot_token,
            revision,
        )
        .await
        {
            Ok(()) => state.set_connected(false),
            Err(error) => {
                state.set_connected(false);
                state.set_error(Some(error));
                tokio::time::sleep(Duration::from_secs(4)).await;
            }
        }
    }
}

async fn run_connection(
    app: &AppHandle,
    client: &Client,
    app_token: &str,
    bot_token: &str,
    revision: u64,
) -> Result<(), String> {
    let socket_url = api::open_socket(client, app_token).await?;
    let (stream, _) = connect_async(&socket_url)
        .await
        .map_err(|error| format!("Could not connect to Slack Socket Mode: {error}"))?;
    let (mut writer, mut reader) = stream.split();
    let state = app.state::<SlackState>();
    state.set_connected(true);
    state.set_error(None);

    loop {
        let message = tokio::select! {
            message = reader.next() => message,
            _ = tokio::time::sleep(Duration::from_secs(1)) => {
                if state.revision.load(std::sync::atomic::Ordering::Relaxed) != revision {
                    return Ok(());
                }
                continue;
            }
        };
        let Some(message) = message else { break };
        let message =
            message.map_err(|error| format!("Slack Socket Mode disconnected: {error}"))?;
        let Message::Text(text) = message else {
            continue;
        };
        let envelope = serde_json::from_str::<Envelope>(&text)
            .map_err(|error| format!("Slack sent an invalid Socket Mode payload: {error}"))?;
        if let Some(envelope_id) = envelope.envelope_id.as_deref() {
            writer
                .send(Message::Text(
                    serde_json::json!({ "envelope_id": envelope_id })
                        .to_string()
                        .into(),
                ))
                .await
                .map_err(|error| format!("Could not acknowledge Slack event: {error}"))?;
        }
        if envelope.kind == "disconnect" {
            break;
        }
        if envelope.kind != "events_api" {
            continue;
        }
        if let Some(payload) = envelope.payload {
            process_event(app, client, bot_token, payload).await;
        }
    }
    Err("Slack Socket Mode connection closed; reconnecting".to_string())
}

async fn process_event(app: &AppHandle, client: &Client, bot_token: &str, payload: EventPayload) {
    let Some(event_id) = payload.event_id else {
        return;
    };
    let Some(event) = payload.event else { return };
    if event.kind != "app_mention" || !is_create_note_command(&event.text) {
        return;
    }
    let Some(thread_ts) = event.thread_ts.as_deref() else {
        return;
    };
    let claimed = notes::claim_event(app, &event_id, &event.channel, thread_ts).unwrap_or(false);
    if !claimed {
        return;
    }

    let result = async {
        let messages = api::fetch_thread(client, bot_token, &event.channel, thread_ts).await?;
        let (note_id, title) = notes::create_thread_note(
            app.clone(),
            &event.channel,
            thread_ts,
            &event.ts,
            &messages,
        )?;
        // Confirmation is intentionally best-effort; note creation is the durable operation.
        let _ = api::post_confirmation(client, bot_token, &event.channel, thread_ts, &title).await;
        Ok::<String, String>(note_id)
    }
    .await;
    match result {
        Ok(note_id) => notes::complete_event(app, &event_id, &note_id),
        Err(error) => notes::fail_event(app, &event_id, &error),
    }
}

fn is_create_note_command(text: &str) -> bool {
    let without_mention = text.rsplit_once('>').map(|(_, rest)| rest).unwrap_or(text);
    without_mention
        .trim()
        .to_lowercase()
        .starts_with("create note")
}

#[cfg(test)]
mod tests {
    use super::is_create_note_command;

    #[test]
    fn recognizes_create_note_after_mention() {
        assert!(is_create_note_command("<@U123> create note"));
        assert!(is_create_note_command("<@U123> Create Note please"));
        assert!(!is_create_note_command("<@U123> summarize this"));
    }
}
