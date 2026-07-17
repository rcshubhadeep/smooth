//! Per-note chat: ask questions about a note (or meeting transcript) using the
//! local llama.cpp server. History is persisted per note; replies stream back
//! to the frontend over a Tauri channel.

use std::{collections::VecDeque, time::Duration};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tauri::{ipc::Channel, AppHandle};

use crate::{
    db_error, is_bonsai_model, llama_endpoint,
    llm::{is_mercury_model, resolve_target, LlmSelection, LlmTarget},
    new_id, note_context, now_string, open_database,
};

/// Assumed context window (tokens) when the model doesn't report one.
const DEFAULT_CTX_TOKENS: usize = 4096;
/// Conservative fallback when llama.cpp's token counter is unavailable.
const FALLBACK_CHARS_PER_TOKEN: usize = 3;
/// Safety bound on hierarchical summary reduction.
const MAX_SUMMARY_LEVELS: usize = 8;
const MAX_ANSWER_CONTINUATIONS: usize = 4;
const MIN_SPLIT_CHARS: usize = 600;
const INPUT_RESERVE_TOKENS: usize = 300;
const DEFAULT_MAX_CHAT_ANSWER_TOKENS: usize = 1024;
const BONSAI_MAX_CHAT_ANSWER_TOKENS: usize = 2048;
const MERCURY_MAX_CHAT_ANSWER_TOKENS: usize = 4096;

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
    Status {
        message: String,
    },
    Delta {
        delta: String,
    },
    Done {
        message: ChatMessage,
    },
    Error {
        message: String,
    },
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

            CREATE TABLE IF NOT EXISTS note_summaries (
                note_id TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                model TEXT NOT NULL,
                level INTEGER NOT NULL,
                chunk_index INTEGER NOT NULL,
                source_hash TEXT NOT NULL,
                summary TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (
                    note_id, content_hash, model, level, chunk_index, source_hash
                )
            );

            CREATE INDEX IF NOT EXISTS note_summaries_lookup_idx
                ON note_summaries(note_id, content_hash, model, level, chunk_index);
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
    selection: Option<LlmSelection>,
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

    let target = resolve_target(&app, selection.as_ref())?;

    // Run the streaming completion; report any failure through the channel so the
    // frontend has a single event stream to listen to.
    match stream_completion(&app, &on_event, &note_id, &target, &title, &body, &history).await {
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
    #[serde(default)]
    finish_reason: Option<String>,
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

    let models = match client
        .get(llama_endpoint(base_url, "/v1/models"))
        .send()
        .await
    {
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
    context_tokens: usize,
    note_input_tokens: usize,
    chunk_input_tokens: usize,
    summary_max_tokens: u32,
    answer_max_tokens: u32,
    initial_chunk_chars: usize,
}

fn budget_for(context_tokens: Option<usize>, model: &str) -> Budget {
    let ctx = context_tokens.unwrap_or(DEFAULT_CTX_TOKENS).max(1024);
    let max_answer_tokens = if is_mercury_model(model) {
        MERCURY_MAX_CHAT_ANSWER_TOKENS
    } else if is_bonsai_model(model) {
        BONSAI_MAX_CHAT_ANSWER_TOKENS
    } else {
        DEFAULT_MAX_CHAT_ANSWER_TOKENS
    };
    let answer = (ctx / 4).clamp(256, max_answer_tokens);
    let summary = (ctx / 5).clamp(256, 900);
    let note_input_tokens = ctx.saturating_sub(answer + INPUT_RESERVE_TOKENS).max(400);
    let chunk_input_tokens = ctx
        .saturating_sub(summary as usize + INPUT_RESERVE_TOKENS)
        .max(400);
    Budget {
        context_tokens: ctx,
        note_input_tokens,
        chunk_input_tokens,
        summary_max_tokens: summary as u32,
        answer_max_tokens: answer as u32,
        initial_chunk_chars: chunk_input_tokens * FALLBACK_CHARS_PER_TOKEN,
    }
}

#[derive(Deserialize)]
struct InputTokensResponse {
    input_tokens: usize,
}

#[derive(Deserialize)]
struct CompletionResponse {
    choices: Vec<CompletionChoice>,
}

#[derive(Deserialize)]
struct CompletionChoice {
    message: CompletionMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct CompletionMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
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
helpful. Do not use Markdown tables; this answer is shown in a narrow panel, so use short \
headings and bullet lists instead.{summary_note}\n\n--- NOTE CONTENT ---\n{body}\n--- END NOTE CONTENT ---"
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

fn hash_text(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn summary_prompt(chunk: &str) -> String {
    format!(
        "Summarize the section of a note or meeting transcript below, preserving key facts, \
names, numbers, decisions, open questions, action items, and chronological order. Be \
comprehensive but concise. Do not add anything that is not present.\n\n---\n{chunk}"
    )
}

fn reduce_prompt(chunk: &str) -> String {
    format!(
        "Combine the partial summaries below into one faithful summary. Preserve concrete \
details, decisions, action items, unresolved questions, names, numbers, dates, and chronology. \
Remove duplication, but do not drop unique facts.\n\n---\n{chunk}"
    )
}

fn summary_cache_lookup(
    app: &AppHandle,
    note_id: &str,
    content_hash: &str,
    model: &str,
    level: usize,
    chunk_index: usize,
    source_hash: &str,
) -> Result<Option<String>, String> {
    let connection = open_database(app)?;
    connection
        .query_row(
            "
            SELECT summary
            FROM note_summaries
            WHERE note_id = ?1
              AND content_hash = ?2
              AND model = ?3
              AND level = ?4
              AND chunk_index = ?5
              AND source_hash = ?6
            ",
            params![
                note_id,
                content_hash,
                model,
                level as i64,
                chunk_index as i64,
                source_hash
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(db_error)
}

fn summary_cache_store(
    app: &AppHandle,
    note_id: &str,
    content_hash: &str,
    model: &str,
    level: usize,
    chunk_index: usize,
    source_hash: &str,
    summary: &str,
) -> Result<(), String> {
    let connection = open_database(app)?;
    connection
        .execute(
            "
            INSERT INTO note_summaries (
                note_id, content_hash, model, level, chunk_index, source_hash,
                summary, created_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(note_id, content_hash, model, level, chunk_index, source_hash)
            DO UPDATE SET summary = excluded.summary, created_at = excluded.created_at
            ",
            params![
                note_id,
                content_hash,
                model,
                level as i64,
                chunk_index as i64,
                source_hash,
                summary,
                now_string()
            ],
        )
        .map_err(db_error)?;
    Ok(())
}

async fn count_input_tokens(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    messages: &[serde_json::Value],
) -> Option<usize> {
    let response = client
        .post(llama_endpoint(
            base_url,
            "/v1/chat/completions/input_tokens",
        ))
        .json(&json!({
            "model": model,
            "messages": messages
        }))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    response
        .json::<InputTokensResponse>()
        .await
        .ok()
        .map(|result| result.input_tokens)
}

fn estimate_tokens(messages: &[serde_json::Value]) -> usize {
    let content_chars = messages
        .iter()
        .filter_map(|message| message.get("content").and_then(|value| value.as_str()))
        .map(|content| content.chars().count())
        .sum::<usize>();
    (content_chars / FALLBACK_CHARS_PER_TOKEN) + (messages.len() * 12) + 32
}

async fn messages_fit(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    messages: &[serde_json::Value],
    input_budget: usize,
) -> bool {
    count_input_tokens(client, base_url, model, messages)
        .await
        .unwrap_or_else(|| estimate_tokens(messages))
        <= input_budget
}

async fn text_fits_as_note_context(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    title: &str,
    text: &str,
    summarized: bool,
    budget: &Budget,
) -> bool {
    let messages = [json!({
        "role": "system",
        "content": build_system_prompt(title, text, summarized),
    })];
    messages_fit(client, base_url, model, &messages, budget.note_input_tokens).await
}

fn split_text_near_middle(text: &str) -> Option<(String, String)> {
    let total_chars = text.chars().count();
    if total_chars < 2 {
        return None;
    }
    let midpoint = total_chars / 2;
    let char_indices = text.char_indices().collect::<Vec<_>>();
    let mid_byte = char_indices
        .get(midpoint)
        .map(|(index, _)| *index)
        .unwrap_or(text.len());

    let search_radius = text.len() / 4;
    let start = mid_byte.saturating_sub(search_radius);
    let end = (mid_byte + search_radius).min(text.len());
    let window = &text[start..end];
    let split_byte = window
        .match_indices("\n\n")
        .map(|(index, _)| start + index + 2)
        .min_by_key(|index| index.abs_diff(mid_byte))
        .or_else(|| {
            window
                .match_indices('\n')
                .map(|(index, _)| start + index + 1)
                .min_by_key(|index| index.abs_diff(mid_byte))
        })
        .unwrap_or(mid_byte);

    let (first, second) = text.split_at(split_byte);
    let first = first.trim().to_string();
    let second = second.trim().to_string();
    if first.is_empty() || second.is_empty() {
        None
    } else {
        Some((first, second))
    }
}

async fn fit_summary_chunks(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    text: &str,
    budget: &Budget,
    reduce: bool,
) -> Result<Vec<String>, String> {
    let mut pending = VecDeque::from(split_chunks(text, budget.initial_chunk_chars));
    let mut chunks = Vec::new();

    while let Some(chunk) = pending.pop_front() {
        let prompt = if reduce {
            reduce_prompt(&chunk)
        } else {
            summary_prompt(&chunk)
        };
        let messages = [json!({ "role": "user", "content": prompt })];
        let fits = messages_fit(
            client,
            base_url,
            model,
            &messages,
            budget.chunk_input_tokens,
        )
        .await;
        if fits {
            chunks.push(chunk);
            continue;
        }

        if chunk.chars().count() <= MIN_SPLIT_CHARS {
            return Err(format!(
                "A note segment cannot fit within the {} token summarization budget",
                budget.chunk_input_tokens
            ));
        }
        let Some((first, second)) = split_text_near_middle(&chunk) else {
            return Err("Unable to split an oversized note segment".to_string());
        };
        pending.push_front(second);
        pending.push_front(first);
    }

    Ok(chunks)
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
    // These calls are reduction/extraction tasks where hidden reasoning only
    // consumes the bounded output budget. Gemma 4 in particular can otherwise
    // finish with empty content after placing every token in reasoning_content.
    complete_once_with_format(
        client, base_url, model, system, user, max_tokens, None, true,
    )
    .await
}

async fn complete_once_with_format(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    system: &str,
    user: &str,
    max_tokens: u32,
    response_format: Option<serde_json::Value>,
    disable_thinking: bool,
) -> Result<String, String> {
    // Some chat templates (e.g. Gemma) have no system role — only include one
    // when provided, otherwise everything goes in the user turn.
    let mut messages = Vec::new();
    if !system.trim().is_empty() {
        messages.push(json!({ "role": "system", "content": system }));
    }
    messages.push(json!({ "role": "user", "content": user }));

    let mut payload = json!({
        "model": model,
        "messages": messages,
        "stream": false,
        "temperature": 0.2,
        "max_tokens": max_tokens,
    });
    if let Some(response_format) = response_format {
        payload["response_format"] = response_format;
    }
    if disable_thinking {
        if is_mercury_model(model) {
            payload["reasoning_effort"] = json!("low");
        } else if is_bonsai_model(model) || model.to_ascii_lowercase().contains("gemma") {
            payload["chat_template_kwargs"] = json!({ "enable_thinking": false });
        }
    }

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
    let choice = parsed
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| "llama.cpp returned no completion choice".to_string())?;
    if let Some(content) = choice
        .message
        .content
        .filter(|content| !content.trim().is_empty())
    {
        return Ok(content);
    }
    if choice.finish_reason.as_deref() == Some("length") {
        return Err(
            "The model used its output budget for reasoning before producing an answer".to_string(),
        );
    }
    choice
        .message
        .reasoning_content
        .filter(|content| !content.trim().is_empty())
        .ok_or_else(|| "llama.cpp returned no completion content".to_string())
}

async fn summarize_chunk(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    chunk: &str,
    max_tokens: u32,
) -> Result<String, String> {
    let prompt = summary_prompt(chunk);
    complete_once(client, base_url, model, "", &prompt, max_tokens).await
}

async fn reduce_summary_chunk(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    chunk: &str,
    max_tokens: u32,
) -> Result<String, String> {
    let prompt = reduce_prompt(chunk);
    complete_once(client, base_url, model, "", &prompt, max_tokens).await
}

/// Build a model-fit note memory. Long notes are summarized in cached levels:
/// raw chunks -> chunk summaries -> reduced summaries, until the final memory fits.
async fn prepare_note_memory(
    app: &AppHandle,
    note_id: &str,
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    title: &str,
    content: &str,
    budget: &Budget,
    on_event: &Channel<ChatStreamEvent>,
) -> Result<(String, bool), String> {
    if text_fits_as_note_context(client, base_url, model, title, content, false, budget).await {
        return Ok((content.to_string(), false));
    }

    let content_hash = hash_text(content);
    let _ = on_event.send(ChatStreamEvent::Status {
        message: "Reading the full note…".to_string(),
    });

    let mut current = content.to_string();
    for level in 0..MAX_SUMMARY_LEVELS {
        let reduce = level > 0;
        let chunks = fit_summary_chunks(client, base_url, model, &current, budget, reduce).await?;
        let total = chunks.len();
        let mut summaries = Vec::with_capacity(total);

        for (index, chunk) in chunks.into_iter().enumerate() {
            let source_hash = hash_text(&chunk);
            if let Some(summary) = summary_cache_lookup(
                app,
                note_id,
                &content_hash,
                model,
                level,
                index,
                &source_hash,
            )? {
                summaries.push(summary);
                continue;
            }

            let _ = on_event.send(ChatStreamEvent::Status {
                message: if reduce {
                    format!(
                        "Reducing note summary… pass {} ({}/{})",
                        level + 1,
                        index + 1,
                        total
                    )
                } else {
                    format!("Summarizing long note… ({}/{})", index + 1, total)
                },
            });

            let summary = if reduce {
                reduce_summary_chunk(client, base_url, model, &chunk, budget.summary_max_tokens)
                    .await?
            } else {
                summarize_chunk(client, base_url, model, &chunk, budget.summary_max_tokens).await?
            };
            let summary = summary.trim().to_string();
            if summary.is_empty() {
                return Err(format!(
                    "The model returned an empty summary for chunk {} of {}",
                    index + 1,
                    total
                ));
            }

            summary_cache_store(
                app,
                note_id,
                &content_hash,
                model,
                level,
                index,
                &source_hash,
                &summary,
            )?;
            summaries.push(summary);
        }

        current = summaries
            .into_iter()
            .enumerate()
            .map(|(index, summary)| format!("## Summary part {}\n{}", index + 1, summary))
            .collect::<Vec<_>>()
            .join("\n\n");

        if text_fits_as_note_context(client, base_url, model, title, &current, true, budget).await {
            return Ok((current, true));
        }
    }

    Err(format!(
        "Could not reduce this note enough to fit the {} token context window after {} passes",
        budget.context_tokens, MAX_SUMMARY_LEVELS
    ))
}

async fn prepare_note_memory_silent(
    app: &AppHandle,
    note_id: &str,
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    title: &str,
    content: &str,
    budget: &Budget,
) -> Result<(String, bool), String> {
    if text_fits_as_note_context(client, base_url, model, title, content, false, budget).await {
        return Ok((content.to_string(), false));
    }

    let content_hash = hash_text(content);
    let mut current = content.to_string();
    for level in 0..MAX_SUMMARY_LEVELS {
        let reduce = level > 0;
        let chunks = fit_summary_chunks(client, base_url, model, &current, budget, reduce).await?;
        let mut summaries = Vec::with_capacity(chunks.len());

        for (index, chunk) in chunks.into_iter().enumerate() {
            let source_hash = hash_text(&chunk);
            if let Some(summary) = summary_cache_lookup(
                app,
                note_id,
                &content_hash,
                model,
                level,
                index,
                &source_hash,
            )? {
                summaries.push(summary);
                continue;
            }

            let summary = if reduce {
                reduce_summary_chunk(client, base_url, model, &chunk, budget.summary_max_tokens)
                    .await?
            } else {
                summarize_chunk(client, base_url, model, &chunk, budget.summary_max_tokens).await?
            };
            let summary = summary.trim().to_string();
            if summary.is_empty() {
                return Err("The model returned an empty meeting summary".to_string());
            }
            summary_cache_store(
                app,
                note_id,
                &content_hash,
                model,
                level,
                index,
                &source_hash,
                &summary,
            )?;
            summaries.push(summary);
        }

        current = summaries
            .into_iter()
            .enumerate()
            .map(|(index, summary)| format!("## Summary part {}\n{}", index + 1, summary))
            .collect::<Vec<_>>()
            .join("\n\n");
        if text_fits_as_note_context(client, base_url, model, title, &current, true, budget).await {
            return Ok((current, true));
        }
    }

    Err("Could not reduce the meeting transcript enough to fill note sections".to_string())
}

pub(crate) async fn generate_meeting_note_sections(
    app: &AppHandle,
    transcript_note_id: &str,
    transcript_title: &str,
    transcript_content: &str,
    user_note_content: &str,
    empty_headings: &[String],
) -> Result<String, String> {
    let target = resolve_target(app, None)?;
    let client = target
        .client_builder()?
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|error| error.to_string())?;
    let base_url = target.base_url.clone();
    let (model, discovered_context) =
        resolve_model(&client, &base_url, target.model.clone()).await?;
    let context_tokens = target.context_tokens.or(discovered_context);
    let budget = budget_for(context_tokens, &model);
    let (memory, _) = prepare_note_memory_silent(
        app,
        transcript_note_id,
        &client,
        &base_url,
        &model,
        transcript_title,
        transcript_content,
        &budget,
    )
    .await?;
    let headings = empty_headings
        .iter()
        .map(|heading| format!("- {heading}"))
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = format!(
        "Fill only the empty markdown sections requested below using evidence from the meeting transcript. Do not invent facts. Return only JSON in this exact shape: {{\"sections\":[{{\"heading\":\"Heading text without #\",\"content\":\"Markdown body\"}}]}}. Keep each body concise and actionable.\n\nEmpty headings:\n{headings}\n\nUser's meeting note (preserve existing text):\n{user_note_content}\n\nMeeting transcript or reduced transcript:\n{memory}"
    );
    complete_once(
        &client,
        &base_url,
        &model,
        "",
        &prompt,
        budget.answer_max_tokens,
    )
    .await
}

/// Result of a one-off task grounded in a complete note. Oversized notes are
/// hierarchically reduced before the final task, using the same context-aware
/// path as note chat and meeting-note completion.
pub(crate) struct GroundedNoteCompletion {
    pub(crate) model: String,
    pub(crate) base_url: String,
    pub(crate) answer: String,
    pub(crate) used_summary: bool,
}

pub(crate) async fn complete_grounded_note_task(
    app: &AppHandle,
    note_id: &str,
    title: &str,
    content: &str,
    instructions: &str,
    response_format: Option<serde_json::Value>,
    selection: Option<&LlmSelection>,
) -> Result<GroundedNoteCompletion, String> {
    let target = resolve_target(app, selection)?;
    let client = target
        .client_builder()?
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|error| error.to_string())?;
    let base_url = target.base_url.clone();
    let (model, discovered_context) =
        resolve_model(&client, &base_url, target.model.clone()).await?;
    let context_tokens = target.context_tokens.or(discovered_context);
    let mut budget = budget_for(context_tokens, &model);
    // The task itself occupies context in addition to note memory. Reduce the
    // note against a smaller budget so long instructions cannot make the final
    // request overflow after the memory has already been prepared.
    let task_overhead_tokens = (instructions.chars().count() / FALLBACK_CHARS_PER_TOKEN) + 200;
    budget.note_input_tokens = budget
        .note_input_tokens
        .saturating_sub(task_overhead_tokens)
        .max(400);
    let (memory, used_summary) = prepare_note_memory_silent(
        app, note_id, &client, &base_url, &model, title, content, &budget,
    )
    .await?;
    let source_kind = if used_summary {
        "a faithful hierarchical reduction of the complete note"
    } else {
        "the complete note"
    };
    let prompt = format!(
        "Perform the task below using only {source_kind}. Do not invent facts.\n\n\
TASK:\n{instructions}\n\n\
NOTE TITLE:\n{title}\n\n\
NOTE CONTENT:\n{memory}\n\n--- END NOTE ---"
    );
    let final_messages = [json!({ "role": "user", "content": prompt })];
    let final_input_budget = budget
        .context_tokens
        .saturating_sub(budget.answer_max_tokens as usize + INPUT_RESERVE_TOKENS)
        .max(400);
    if !messages_fit(
        &client,
        &base_url,
        &model,
        &final_messages,
        final_input_budget,
    )
    .await
    {
        return Err("Could not reduce this note enough for the requested task".to_string());
    }
    let formatted = complete_once_with_format(
        &client,
        &base_url,
        &model,
        "",
        &prompt,
        budget.answer_max_tokens,
        response_format.clone(),
        true,
    )
    .await;
    let answer = match formatted {
        Ok(answer) => answer,
        Err(error) if response_format.is_some() && error.contains("HTTP 400") => {
            complete_once_with_format(
                &client,
                &base_url,
                &model,
                "",
                &prompt,
                budget.answer_max_tokens,
                None,
                true,
            )
            .await?
        }
        Err(error) => return Err(error),
    };

    Ok(GroundedNoteCompletion {
        model,
        base_url,
        answer,
        used_summary,
    })
}

async fn select_history_messages(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    system_message: serde_json::Value,
    history: &[ChatMessage],
    budget: &Budget,
) -> Result<Vec<serde_json::Value>, String> {
    let max_input_tokens = budget
        .context_tokens
        .saturating_sub(budget.answer_max_tokens as usize + INPUT_RESERVE_TOKENS)
        .max(400);
    let Some(latest) = history.last() else {
        return Ok(vec![system_message]);
    };

    let latest_message = json!({ "role": latest.role, "content": latest.content });
    let required = vec![system_message.clone(), latest_message.clone()];
    if !messages_fit(client, base_url, model, &required, max_input_tokens).await {
        return Err(
            "The note memory plus your latest question still exceeds the model context. Try a shorter question or a larger-context model."
                .to_string(),
        );
    }

    let mut selected = vec![latest_message];
    for message in history[..history.len().saturating_sub(1)].iter().rev() {
        let candidate_message = json!({ "role": message.role, "content": message.content });
        let mut candidate = Vec::with_capacity(selected.len() + 2);
        candidate.push(system_message.clone());
        candidate.push(candidate_message.clone());
        candidate.extend(selected.iter().cloned().rev());

        if messages_fit(client, base_url, model, &candidate, max_input_tokens).await {
            selected.push(candidate_message);
        } else {
            break;
        }
    }

    let mut messages = Vec::with_capacity(selected.len() + 1);
    messages.push(system_message);
    messages.extend(selected.into_iter().rev());
    Ok(messages)
}

async fn stream_chat_once(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    messages: &[serde_json::Value],
    max_tokens: u32,
    on_event: &Channel<ChatStreamEvent>,
) -> Result<(String, Option<String>), String> {
    let payload = chat_stream_payload(model, messages, max_tokens);

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

    // Buffer raw bytes and only decode complete lines. A network chunk can split
    // a multi-byte character or an SSE line, which would corrupt/drop deltas.
    let mut byte_buffer: Vec<u8> = Vec::new();
    let mut full = String::new();
    let mut finish_reason = None;

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
                if let Some(reason) = &choice.finish_reason {
                    finish_reason = Some(reason.clone());
                }
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

    Ok((full, finish_reason))
}

fn chat_stream_payload(
    model: &str,
    messages: &[serde_json::Value],
    max_tokens: u32,
) -> serde_json::Value {
    let mut payload = json!({
        "model": model,
        "messages": messages,
        "stream": true,
        "temperature": 0.4,
        "max_tokens": max_tokens
    });
    if is_bonsai_model(model) {
        payload["reasoning_format"] = json!("none");
        payload["chat_template_kwargs"] = json!({ "enable_thinking": false });
    } else if is_mercury_model(model) {
        payload["reasoning_effort"] = json!("low");
    }
    payload
}

async fn stream_completion(
    app: &AppHandle,
    on_event: &Channel<ChatStreamEvent>,
    note_id: &str,
    target: &LlmTarget,
    title: &str,
    note_content: &str,
    history: &[ChatMessage],
) -> Result<String, String> {
    let client = target
        .client_builder()?
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|error| error.to_string())?;

    let base_url = target.base_url.as_str();
    let (model, discovered_context) =
        resolve_model(&client, base_url, target.model.clone()).await?;
    let context_tokens = target.context_tokens.or(discovered_context);
    let budget = budget_for(context_tokens, &model);
    let (context, summarized) = prepare_note_memory(
        app,
        note_id,
        &client,
        base_url,
        &model,
        title,
        note_content,
        &budget,
        on_event,
    )
    .await?;

    let system_message = json!({
        "role": "system",
        "content": build_system_prompt(title, &context, summarized),
    });
    let mut messages =
        select_history_messages(&client, base_url, &model, system_message, history, &budget)
            .await?;
    let mut full = String::new();
    for continuation in 0..=MAX_ANSWER_CONTINUATIONS {
        let (part, finish_reason) = stream_chat_once(
            &client,
            base_url,
            &model,
            &messages,
            budget.answer_max_tokens,
            on_event,
        )
        .await?;
        full.push_str(&part);

        if finish_reason.as_deref() != Some("length") {
            return Ok(full);
        }

        if continuation == MAX_ANSWER_CONTINUATIONS {
            if !is_bonsai_model(&model) && !is_mercury_model(&model) {
                return Ok(full);
            }
            return Err(format!(
                "The chat answer exceeded {} output tokens across {} completion parts",
                budget.answer_max_tokens,
                MAX_ANSWER_CONTINUATIONS + 1
            ));
        }

        let _ = on_event.send(ChatStreamEvent::Status {
            message: "Continuing answer…".to_string(),
        });
        messages.push(json!({ "role": "assistant", "content": part }));
        messages.push(json!({
            "role": "user",
            "content": "Continue exactly where you stopped. Do not restart or repeat previous sections."
        }));
    }

    Ok(full)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bonsai_chat_budget_and_payload_preserve_visible_output() {
        let budget = budget_for(Some(65_536), "prism-ml/Bonsai-27B-gguf:Q1_0");
        let messages = [json!({ "role": "user", "content": "What are the action items?" })];
        let payload = chat_stream_payload(
            "prism-ml/Bonsai-27B-gguf:Q1_0",
            &messages,
            budget.answer_max_tokens,
        );

        assert_eq!(budget.answer_max_tokens, 2048);
        assert_eq!(payload["max_tokens"], 2048);
        assert_eq!(payload["reasoning_format"], "none");
        assert_eq!(payload["chat_template_kwargs"]["enable_thinking"], false);
    }

    #[test]
    fn gemma_chat_preserves_legacy_budget_and_payload() {
        let model = "unsloth/gemma-4-12B-it-qat-GGUF:UD-Q4_K_XL";
        let budget = budget_for(Some(65_536), model);
        let messages = [json!({ "role": "user", "content": "What are the action items?" })];
        let payload = chat_stream_payload(model, &messages, budget.answer_max_tokens);

        assert_eq!(budget.answer_max_tokens, 1024);
        assert_eq!(payload["max_tokens"], 1024);
        assert!(payload.get("reasoning_format").is_none());
        assert!(payload.get("chat_template_kwargs").is_none());
    }

    #[test]
    fn mercury_chat_uses_remote_profile_without_llama_fields() {
        let model = "mercury-2";
        let budget = budget_for(Some(128_000), model);
        let messages = [json!({ "role": "user", "content": "What are the action items?" })];
        let payload = chat_stream_payload(model, &messages, budget.answer_max_tokens);

        assert_eq!(budget.answer_max_tokens, 4096);
        assert_eq!(payload["reasoning_effort"], "low");
        assert!(payload.get("reasoning_format").is_none());
        assert!(payload.get("chat_template_kwargs").is_none());
    }
}
