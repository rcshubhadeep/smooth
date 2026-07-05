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

/// Assumed context window (tokens) when the model doesn't report one.
const DEFAULT_CTX_TOKENS: usize = 4096;
/// Conservative chars-per-token estimate — we deliberately undershoot so prompts
/// never overflow the model's context (an overflow makes llama.cpp return empty).
const CHARS_PER_TOKEN: usize = 3;
/// Safety bound on recursive summarization depth.
const MAX_SUMMARY_PASSES: usize = 5;

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
    /// Transient progress (e.g. while summarizing a long note before answering).
    Status { message: String },
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
    note_content: String,
    on_event: Channel<ChatStreamEvent>,
) -> Result<(), String> {
    let question = content.trim().to_string();
    if question.is_empty() {
        return Err("Message is empty".to_string());
    }

    // Prefer the live content the frontend is showing (avoids any disk/save-timing
    // lag, e.g. right after a meeting); fall back to what's on disk.
    let (title, disk_content) = note_context(&app, &note_id)?;
    let body = if note_content.trim().is_empty() {
        disk_content
    } else {
        note_content
    };
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
    match stream_completion(&on_event, &base_url, preferred_model, &title, &body, &history).await {
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
    #[serde(default)]
    meta: Option<ModelMeta>,
}

#[derive(Default, Deserialize)]
struct ModelMeta {
    #[serde(default)]
    n_ctx: Option<u64>,
    #[serde(default)]
    n_ctx_train: Option<u64>,
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

/// Resolve the model id and its context window (tokens), so we can size prompts.
async fn resolve_model(
    client: &reqwest::Client,
    base_url: &str,
    preferred: Option<String>,
) -> Result<(String, Option<usize>), String> {
    let preferred = preferred.filter(|model| !model.trim().is_empty());

    let models = match client.get(llama_endpoint(base_url, "/v1/models")).send().await {
        Ok(response) if response.status().is_success() => response
            .json::<ModelsResponse>()
            .await
            .map(|parsed| parsed.data)
            .unwrap_or_default(),
        Ok(_) => Vec::new(),
        Err(error) => {
            // If the user pinned a model we can still proceed without context info.
            if let Some(model) = preferred {
                return Ok((model, None));
            }
            return Err(format!("Could not reach llama.cpp: {error}"));
        }
    };

    let entry = if let Some(model) = preferred {
        models
            .into_iter()
            .find(|entry| entry.id == model)
            .unwrap_or(ModelEntry {
                id: model,
                meta: None,
            })
    } else {
        models
            .into_iter()
            .next()
            .ok_or_else(|| "No model is loaded in llama.cpp. Check Settings.".to_string())?
    };

    let context_tokens = entry
        .meta
        .and_then(|meta| meta.n_ctx.or(meta.n_ctx_train))
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value > 0);

    Ok((entry.id, context_tokens))
}

/// Character / token budgets derived from the model's context window.
struct Budget {
    /// Note content up to this many chars is sent directly (no summarization).
    direct_chars: usize,
    /// Raw content consumed per summarization request.
    chunk_chars: usize,
    summary_max_tokens: u32,
    answer_max_tokens: u32,
}

fn budget_for(context_tokens: Option<usize>) -> Budget {
    let ctx = context_tokens.unwrap_or(DEFAULT_CTX_TOKENS).max(1024);
    let answer = (ctx / 3).clamp(256, 1024);
    let summary = (ctx / 4).clamp(224, 640);
    // Reserve room for the output + the fixed prompt/instructions/history.
    let direct_tokens = ctx.saturating_sub(answer + 350).max(400);
    let chunk_tokens = ctx.saturating_sub(summary as usize + 250).max(400);
    Budget {
        direct_chars: direct_tokens * CHARS_PER_TOKEN,
        chunk_chars: chunk_tokens * CHARS_PER_TOKEN,
        summary_max_tokens: summary as u32,
        answer_max_tokens: answer as u32,
    }
}

#[derive(Deserialize)]
struct CompletionResponse {
    choices: Vec<CompletionChoice>,
}

#[derive(Deserialize)]
struct CompletionChoice {
    message: CompletionMessage,
}

#[derive(Deserialize)]
struct CompletionMessage {
    #[serde(default)]
    content: Option<String>,
}

fn build_system_prompt(title: &str, content: &str, summarized: bool) -> String {
    let body = if content.trim().is_empty() {
        "(This note is currently empty.)".to_string()
    } else {
        content.to_string()
    };

    let summary_note = if summarized {
        " The content below is a condensed summary of a longer note, produced to fit the \
model's context — key facts, decisions, and action items are preserved."
    } else {
        ""
    };

    format!(
        "You are Smooth's assistant. Answer the user's questions about the note below, \
titled \"{title}\". Base your answers on the note's content — if the note is a meeting \
transcript, treat it as the conversation so far. If the answer is not in the note, say so \
plainly instead of guessing. Keep answers concise and format them with Markdown when \
helpful.{summary_note}\n\n--- NOTE CONTENT ---\n{body}\n--- END NOTE CONTENT ---"
    )
}

/// Split text into chunks of roughly `size` characters, preferring line breaks.
fn split_chunks(text: &str, size: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for line in text.split_inclusive('\n') {
        if !current.is_empty() && current.chars().count() + line.chars().count() > size {
            chunks.push(std::mem::take(&mut current));
        }
        if line.chars().count() > size {
            for ch in line.chars() {
                current.push(ch);
                if current.chars().count() >= size {
                    chunks.push(std::mem::take(&mut current));
                }
            }
        } else {
            current.push_str(line);
        }
    }
    if !current.trim().is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Non-streaming completion, used for summarization passes.
async fn complete_once(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    system: &str,
    user: &str,
    max_tokens: u32,
) -> Result<String, String> {
    // Some chat templates (e.g. Gemma) have no system role — only include one
    // when provided, otherwise everything goes in the user turn.
    let mut messages = Vec::new();
    if !system.trim().is_empty() {
        messages.push(json!({ "role": "system", "content": system }));
    }
    messages.push(json!({ "role": "user", "content": user }));

    let payload = json!({
        "model": model,
        "messages": messages,
        "stream": false,
        "temperature": 0.2,
        "max_tokens": max_tokens,
    });

    let response = client
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
            body.trim().chars().take(200).collect::<String>()
        ));
    }
    let parsed = response
        .json::<CompletionResponse>()
        .await
        .map_err(|error| format!("Invalid llama.cpp response: {error}"))?;
    parsed
        .choices
        .into_iter()
        .next()
        .and_then(|choice| choice.message.content)
        .ok_or_else(|| "llama.cpp returned no summary content".to_string())
}

async fn summarize_chunk(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    chunk: &str,
    max_tokens: u32,
) -> Result<String, String> {
    // Instruction lives in the user turn (no system role) for cross-template safety.
    let prompt = format!(
        "Summarize the section of a note or meeting transcript below, preserving key facts, \
names, numbers, decisions, open questions, and action items. Be comprehensive but concise, \
and do not add anything that isn't present.\n\n---\n{chunk}"
    );
    complete_once(client, base_url, model, "", &prompt, max_tokens).await
}

/// Fit the whole note into the model's context: send it directly if small enough,
/// otherwise recursively summarize chunk-by-chunk until it fits. Never returns
/// empty when the note has content — falls back to raw slices if summaries fail.
async fn prepare_context(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    content: &str,
    budget: &Budget,
    on_event: &Channel<ChatStreamEvent>,
) -> Result<(String, bool), String> {
    if content.chars().count() <= budget.direct_chars {
        return Ok((content.to_string(), false));
    }

    let _ = on_event.send(ChatStreamEvent::Status {
        message: "Reading the full note…".to_string(),
    });

    let mut current = content.to_string();
    let mut pass = 0;
    while current.chars().count() > budget.direct_chars && pass < MAX_SUMMARY_PASSES {
        let chunks = split_chunks(&current, budget.chunk_chars);
        let total = chunks.len();
        let mut summaries = Vec::with_capacity(total);
        for (index, chunk) in chunks.into_iter().enumerate() {
            let _ = on_event.send(ChatStreamEvent::Status {
                message: format!("Summarizing long note… ({}/{})", index + 1, total),
            });
            // If a summary fails or comes back empty (e.g. the chunk still didn't
            // leave the model room to respond), keep a raw slice so the section
            // isn't lost — this guarantees a non-empty final context.
            let summary = summarize_chunk(client, base_url, model, &chunk, budget.summary_max_tokens)
                .await
                .unwrap_or_default();
            let summary = if summary.trim().is_empty() {
                chunk.chars().take(budget.chunk_chars / 3).collect()
            } else {
                summary
            };
            summaries.push(summary);
        }

        let joined = summaries.join("\n\n");
        // Stop if summarization isn't shrinking the content (avoids spinning).
        if joined.chars().count() >= current.chars().count() {
            current = joined;
            break;
        }
        current = joined;
        pass += 1;
    }

    if current.trim().is_empty() {
        // Absolute fallback: use the head of the original note.
        current = content.chars().take(budget.direct_chars).collect();
    } else if current.chars().count() > budget.direct_chars {
        current = current.chars().take(budget.direct_chars).collect();
    }
    Ok((current, true))
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

    let (model, context_tokens) = resolve_model(&client, base_url, preferred_model).await?;
    let budget = budget_for(context_tokens);
    let (context, summarized) =
        prepare_context(&client, base_url, &model, note_content, &budget, on_event).await?;

    let mut messages = vec![json!({
        "role": "system",
        "content": build_system_prompt(title, &context, summarized),
    })];
    for message in history {
        messages.push(json!({ "role": message.role, "content": message.content }));
    }

    let payload = json!({
        "model": model,
        "messages": messages,
        "stream": true,
        "temperature": 0.4,
        "max_tokens": budget.answer_max_tokens,
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

    // Buffer raw bytes and only decode complete lines — a network chunk can split
    // a multi-byte character or an SSE line, which would corrupt/drop deltas.
    let mut byte_buffer: Vec<u8> = Vec::new();
    let mut full = String::new();

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("Stream error: {error}"))?
    {
        byte_buffer.extend_from_slice(&chunk);

        while let Some(newline) = byte_buffer.iter().position(|&byte| byte == b'\n') {
            let line_bytes: Vec<u8> = byte_buffer.drain(..=newline).collect();
            let line = String::from_utf8_lossy(&line_bytes);
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
