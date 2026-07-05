//! Per-note chat: ask questions about a note (or meeting transcript) using the
//! local llama.cpp server. History is persisted per note; replies stream back
//! to the frontend over a Tauri channel.

use std::time::Duration;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{ipc::Channel, AppHandle};

use crate::{
    chat_llama_target, db_error, llama_endpoint, new_id, note_context, now_string, open_database,
};

/// Cap the note content injected into the prompt so we stay within the model's
/// context window (long transcripts get truncated).
const MAX_CONTEXT_CHARS: usize = 16_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ChatMessage {
    id: String,
    note_id: String,
    role: String,
    content: String,
    created_at: String,
}

/// Streaming events sent to the frontend over the channel.
#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(crate) enum ChatStreamEvent {
    Delta { delta: String },
    Done { message: ChatMessage },
    Error { message: String },
}

pub(crate) fn init_schema(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "
            CREATE TABLE IF NOT EXISTS chat_messages (
                id TEXT PRIMARY KEY,
                note_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS chat_messages_note_idx
                ON chat_messages(note_id, created_at);
            ",
        )
        .map_err(db_error)
}

fn load_messages(connection: &Connection, note_id: &str) -> Result<Vec<ChatMessage>, String> {
    let mut statement = connection
        .prepare(
            "
            SELECT id, note_id, role, content, created_at
            FROM chat_messages
            WHERE note_id = ?1
            ORDER BY created_at ASC, rowid ASC
            ",
        )
        .map_err(db_error)?;
    let rows = statement
        .query_map(params![note_id], |row| {
            Ok(ChatMessage {
                id: row.get(0)?,
                note_id: row.get(1)?,
                role: row.get(2)?,
                content: row.get(3)?,
                created_at: row.get(4)?,
            })
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    Ok(rows)
}

fn insert_message(connection: &Connection, message: &ChatMessage) -> Result<(), String> {
    connection
        .execute(
            "
            INSERT INTO chat_messages (id, note_id, role, content, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ",
            params![
                message.id,
                message.note_id,
                message.role,
                message.content,
                message.created_at
            ],
        )
        .map_err(db_error)?;
    Ok(())
}

#[tauri::command]
pub(crate) fn get_chat_messages(
    app: AppHandle,
    note_id: String,
) -> Result<Vec<ChatMessage>, String> {
    let connection = open_database(&app)?;
    load_messages(&connection, &note_id)
}

#[tauri::command]
pub(crate) fn clear_chat(app: AppHandle, note_id: String) -> Result<(), String> {
    let connection = open_database(&app)?;
    connection
        .execute(
            "DELETE FROM chat_messages WHERE note_id = ?1",
            params![note_id],
        )
        .map_err(db_error)?;
    Ok(())
}

#[tauri::command]
pub(crate) async fn send_chat_message(
    app: AppHandle,
    note_id: String,
    content: String,
    on_event: Channel<ChatStreamEvent>,
) -> Result<(), String> {
    let question = content.trim().to_string();
    if question.is_empty() {
        return Err("Message is empty".to_string());
    }

    // Note context + prior history + persist the new user message (sync, no await).
    let (title, note_content) = note_context(&app, &note_id)?;
    let history = {
        let connection = open_database(&app)?;
        let user_message = ChatMessage {
            id: new_id("msg"),
            note_id: note_id.clone(),
            role: "user".to_string(),
            content: question.clone(),
            created_at: now_string(),
        };
        insert_message(&connection, &user_message)?;
        load_messages(&connection, &note_id)?
    };

    let (base_url, preferred_model) = chat_llama_target(&app)?;

    // Run the streaming completion; report any failure through the channel so the
    // frontend has a single event stream to listen to.
    match stream_completion(&on_event, &base_url, preferred_model, &title, &note_content, &history)
        .await
    {
        Ok(answer) if !answer.trim().is_empty() => {
            let assistant = ChatMessage {
                id: new_id("msg"),
                note_id: note_id.clone(),
                role: "assistant".to_string(),
                content: answer,
                created_at: now_string(),
            };
            let connection = open_database(&app)?;
            insert_message(&connection, &assistant)?;
            let _ = on_event.send(ChatStreamEvent::Done { message: assistant });
            Ok(())
        }
        Ok(_) => {
            let message = "The model returned an empty response.".to_string();
            let _ = on_event.send(ChatStreamEvent::Error {
                message: message.clone(),
            });
            Err(message)
        }
        Err(error) => {
            let _ = on_event.send(ChatStreamEvent::Error {
                message: error.clone(),
            });
            Err(error)
        }
    }
}

// ---- llama.cpp streaming ---------------------------------------------------

#[derive(Deserialize)]
struct ModelsResponse {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
}

#[derive(Deserialize)]
struct StreamChunk {
    choices: Vec<StreamChoice>,
}

#[derive(Deserialize)]
struct StreamChoice {
    #[serde(default)]
    delta: StreamDelta,
}

#[derive(Default, Deserialize)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
}

async fn resolve_model(
    client: &reqwest::Client,
    base_url: &str,
    preferred: Option<String>,
) -> Result<String, String> {
    if let Some(model) = preferred {
        if !model.trim().is_empty() {
            return Ok(model);
        }
    }

    let response = client
        .get(llama_endpoint(base_url, "/v1/models"))
        .send()
        .await
        .map_err(|error| format!("Could not reach llama.cpp: {error}"))?;
    if !response.status().is_success() {
        return Err("llama.cpp has no model available. Check Settings.".to_string());
    }
    let parsed = response
        .json::<ModelsResponse>()
        .await
        .map_err(|error| format!("Invalid model list from llama.cpp: {error}"))?;
    parsed
        .data
        .into_iter()
        .next()
        .map(|entry| entry.id)
        .ok_or_else(|| "No model is loaded in llama.cpp.".to_string())
}

fn build_system_prompt(title: &str, content: &str) -> String {
    let trimmed: String = if content.chars().count() > MAX_CONTEXT_CHARS {
        let mut truncated: String = content.chars().take(MAX_CONTEXT_CHARS).collect();
        truncated.push_str("\n\n…[note truncated]");
        truncated
    } else {
        content.to_string()
    };

    let body = if trimmed.trim().is_empty() {
        "(This note is currently empty.)".to_string()
    } else {
        trimmed
    };

    format!(
        "You are Smooth's assistant. Answer the user's questions about the note below, \
titled \"{title}\". Base your answers on the note's content — if the note is a meeting \
transcript, treat it as the conversation so far. If the answer is not in the note, say so \
plainly instead of guessing. Keep answers concise and use Markdown when helpful.\n\n\
--- NOTE CONTENT ---\n{body}\n--- END NOTE CONTENT ---"
    )
}

async fn stream_completion(
    on_event: &Channel<ChatStreamEvent>,
    base_url: &str,
    preferred_model: Option<String>,
    title: &str,
    note_content: &str,
    history: &[ChatMessage],
) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|error| error.to_string())?;

    let model = resolve_model(&client, base_url, preferred_model).await?;

    let mut messages = vec![json!({
        "role": "system",
        "content": build_system_prompt(title, note_content),
    })];
    for message in history {
        messages.push(json!({ "role": message.role, "content": message.content }));
    }

    let payload = json!({
        "model": model,
        "messages": messages,
        "stream": true,
        "temperature": 0.4,
        "max_tokens": 1024,
    });

    let mut response = client
        .post(llama_endpoint(base_url, "/v1/chat/completions"))
        .json(&payload)
        .send()
        .await
        .map_err(|error| format!("llama.cpp request failed: {error}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "llama.cpp returned HTTP {}: {}",
            status.as_u16(),
            body.trim().chars().take(300).collect::<String>()
        ));
    }

    let mut buffer = String::new();
    let mut full = String::new();

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("Stream error: {error}"))?
    {
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(newline) = buffer.find('\n') {
            let line: String = buffer.drain(..=newline).collect();
            let line = line.trim();
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            let Ok(parsed) = serde_json::from_str::<StreamChunk>(data) else {
                continue;
            };
            if let Some(choice) = parsed.choices.first() {
                if let Some(delta) = &choice.delta.content {
                    if !delta.is_empty() {
                        full.push_str(delta);
                        on_event
                            .send(ChatStreamEvent::Delta {
                                delta: delta.clone(),
                            })
                            .map_err(|error| format!("Channel send failed: {error}"))?;
                    }
                }
            }
        }
    }

    Ok(full)
}
