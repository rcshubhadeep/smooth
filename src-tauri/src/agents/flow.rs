//! Minimal agent loop for exercising model-driven tool use.
//!
//! This is intentionally not a full agent framework yet: no persistence, no
//! background scheduling and no approval workflow. It validates model-proposed
//! tool names against the registry, executes them through `AgentRuntime`, then
//! feeds the results back to llama.cpp for a final answer. The structure is
//! meant to become the synchronous foreground execution path that a worker,
//! scheduler or MCP adapter can reuse later.

use std::collections::HashSet;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tauri::AppHandle;

use crate::{chat_llama_target, llama_endpoint};

use super::context::AgentContext;
use super::registry::ToolDescriptor;
use super::runtime::AgentRuntime;

const DEFAULT_MAX_AGENT_STEPS: u8 = 3;
const MAX_AGENT_STEPS: u8 = 6;
const AGENT_MAX_TOKENS: u32 = 1024;

#[derive(Debug, Serialize)]
pub(crate) struct AgentRunResult {
    pub(crate) model: String,
    pub(crate) answer: String,
    pub(crate) steps: Vec<AgentRunStep>,
    pub(crate) raw_model_output: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct AgentRunStep {
    pub(crate) tool_name: String,
    pub(crate) input: Value,
    pub(crate) output: Option<Value>,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone)]
struct ToolCall {
    id: String,
    name: String,
    input: Value,
}

#[derive(Debug, Clone)]
struct ToolExecution {
    call: ToolCall,
    response: Value,
}

#[derive(Debug, Clone, Serialize)]
struct KnownNoteRef {
    id: String,
    title: String,
}

#[derive(Deserialize)]
struct ModelsResponse {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
}

#[derive(Deserialize)]
struct CompletionResponse {
    choices: Vec<CompletionChoice>,
}

#[derive(Deserialize)]
struct CompletionChoice {
    message: CompletionMessage,
}

#[derive(Clone, Deserialize)]
struct CompletionMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ResponseToolCall>,
}

#[derive(Clone, Deserialize)]
struct ResponseToolCall {
    #[serde(default)]
    id: Option<String>,
    function: ResponseToolFunction,
}

#[derive(Clone, Deserialize)]
struct ResponseToolFunction {
    name: String,
    #[serde(default)]
    arguments: ToolArguments,
}

#[derive(Clone, Deserialize)]
#[serde(untagged)]
enum ToolArguments {
    String(String),
    Value(Value),
}

impl Default for ToolArguments {
    fn default() -> Self {
        Self::Value(json!({}))
    }
}

pub(crate) async fn run_agent_once(
    app: AppHandle,
    runtime: &AgentRuntime,
    prompt: String,
    max_steps: Option<u8>,
) -> Result<AgentRunResult, String> {
    let prompt = prompt.trim().to_string();
    if prompt.is_empty() {
        return Err("Agent prompt is empty".to_string());
    }

    let max_steps = max_steps
        .unwrap_or(DEFAULT_MAX_AGENT_STEPS)
        .clamp(1, MAX_AGENT_STEPS);
    let ctx = AgentContext::new(app.clone());
    let tools = runtime.list_tools();
    let tool_names = tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<HashSet<_>>();
    let openai_tools = tools
        .iter()
        .filter(|tool| tool.name != "ping")
        .map(openai_tool_descriptor)
        .collect::<Vec<_>>();

    let (base_url, preferred_model) = chat_llama_target(&app)?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|error| error.to_string())?;
    let model = resolve_model(&client, &base_url, preferred_model).await?;

    let mut messages = vec![
        json!({
            "role": "system",
            "content": agent_system_prompt(&tools),
        }),
        json!({
            "role": "user",
            "content": prompt,
        }),
    ];
    let mut steps = Vec::new();
    let mut known_notes = Vec::<KnownNoteRef>::new();

    for _ in 0..max_steps {
        let response =
            complete_agent_turn(&client, &base_url, &model, &messages, Some(&openai_tools)).await?;
        let raw_model_output = response.content.clone().unwrap_or_default();
        let tool_calls = extract_tool_calls(&response)?;

        if tool_calls.is_empty() {
            return Ok(AgentRunResult {
                model,
                answer: clean_model_text(raw_model_output.clone()),
                steps,
                raw_model_output,
            });
        }

        let mut executions = Vec::new();
        for call in tool_calls {
            let (executable_call, repair) = repair_note_id_arg(call, &known_notes);
            let note_id_validation_error = validate_note_id_arg(&executable_call, &known_notes);
            let (output, error) = if let Some(error) = note_id_validation_error {
                (None, Some(error))
            } else if tool_names.contains(executable_call.name.as_str()) {
                match runtime
                    .execute_tool(&executable_call.name, executable_call.input.clone(), &ctx)
                    .await
                {
                    Ok(value) => (Some(value), None),
                    Err(error) => (None, Some(error.to_string())),
                }
            } else {
                (
                    None,
                    Some(format!("Tool '{}' is not registered", executable_call.name)),
                )
            };

            let tool_response = match (&output, &error) {
                (Some(value), None) => value.clone(),
                (_, Some(message)) => json!({ "error": message }),
                _ => json!({ "error": "Tool returned no output" }),
            };
            if executable_call.name == "search_notes" {
                if let Some(value) = &output {
                    merge_known_notes(&mut known_notes, extract_search_results(value));
                }
            }

            executions.push(ToolExecution {
                call: executable_call.clone(),
                response: enrich_tool_response(
                    tool_response,
                    &executable_call,
                    &known_notes,
                    repair.as_deref(),
                ),
            });
            steps.push(AgentRunStep {
                tool_name: executable_call.name,
                input: executable_call.input,
                output,
                error,
            });
        }
        messages.push(assistant_tool_response_message(&executions));
        messages.push(json!({
            "role": "user",
            "content": tool_result_continuation_prompt(&executions),
        }));
    }

    messages.push(json!({
        "role": "user",
        "content": "Give the final answer now. Do not call more tools. Use only the tool result blocks above as source material. If a detail is not present there, do not include it.",
    }));
    let response = complete_agent_turn(&client, &base_url, &model, &messages, None).await?;
    let raw_model_output = response.content.clone().unwrap_or_default();
    Ok(AgentRunResult {
        model,
        answer: clean_model_text(raw_model_output.clone()),
        steps,
        raw_model_output,
    })
}

fn agent_system_prompt(tools: &[ToolDescriptor]) -> String {
    let tool_names = tools
        .iter()
        .filter(|tool| tool.name != "ping")
        .map(|tool| format!("- {}: {}", tool.name, tool.description))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "You are Smooth's internal notes agent. Use tools when the request needs \
note data or note changes. Only use the tools listed below; never invent tool \
names or database access. Prefer `search_notes` when you need to find a note ID. \
Create or overwrite notes only when the user explicitly asks. For prompts like \
\"search for notes about X and summarize what you find\", first call \
`search_notes`, then call `read_note` for the most relevant result or results \
before writing the summary. Copy note IDs exactly from tool output; never invent \
placeholder IDs such as `note_123`. Do not summarize from search excerpts alone \
when a full note can be read. After tool results arrive, either call another \
useful tool or answer concisely in Markdown. Final answers must be grounded only \
in tool results. Do not infer biographical details, occupations, relationships, \
or facts that are not explicitly present in the tool output.\n\nAvailable tools:\n{tool_names}"
    )
}

fn openai_tool_descriptor(tool: &ToolDescriptor) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.input_schema,
        }
    })
}

async fn resolve_model(
    client: &reqwest::Client,
    base_url: &str,
    preferred: Option<String>,
) -> Result<String, String> {
    if let Some(model) = preferred.filter(|model| !model.trim().is_empty()) {
        return Ok(model);
    }

    let response = client
        .get(llama_endpoint(base_url, "/v1/models"))
        .send()
        .await
        .map_err(|error| format!("Could not reach llama.cpp: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "llama.cpp returned HTTP {} while listing models",
            response.status().as_u16()
        ));
    }
    response
        .json::<ModelsResponse>()
        .await
        .map_err(|error| format!("Invalid llama.cpp models response: {error}"))?
        .data
        .into_iter()
        .next()
        .map(|model| model.id)
        .ok_or_else(|| "No model is loaded in llama.cpp. Check Settings.".to_string())
}

async fn complete_agent_turn(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    messages: &[Value],
    tools: Option<&[Value]>,
) -> Result<CompletionMessage, String> {
    let mut payload = json!({
        "model": model,
        "messages": messages,
        "stream": false,
        "temperature": 0.2,
        "max_tokens": AGENT_MAX_TOKENS,
    });
    if let Some(tools) = tools.filter(|tools| !tools.is_empty()) {
        payload["tools"] = Value::Array(tools.to_vec());
        payload["tool_choice"] = json!("auto");
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
            body.trim().chars().take(500).collect::<String>()
        ));
    }

    response
        .json::<CompletionResponse>()
        .await
        .map_err(|error| format!("Invalid llama.cpp response: {error}"))?
        .choices
        .into_iter()
        .next()
        .map(|choice| choice.message)
        .ok_or_else(|| "llama.cpp returned no choices".to_string())
}

fn extract_tool_calls(message: &CompletionMessage) -> Result<Vec<ToolCall>, String> {
    if !message.tool_calls.is_empty() {
        return message
            .tool_calls
            .iter()
            .enumerate()
            .map(|(index, call)| {
                let input = match &call.function.arguments {
                    ToolArguments::Value(value) => value.clone(),
                    ToolArguments::String(value) => serde_json::from_str::<Value>(value)
                        .unwrap_or_else(|_| Value::Object(parse_gemma_arguments(value))),
                };
                Ok(ToolCall {
                    id: call.id.clone().unwrap_or_else(|| format!("call_{index}")),
                    name: call.function.name.clone(),
                    input,
                })
            })
            .collect();
    }

    Ok(message
        .content
        .as_deref()
        .map(extract_gemma_tool_calls)
        .unwrap_or_default())
}

fn validate_note_id_arg(call: &ToolCall, known_notes: &[KnownNoteRef]) -> Option<String> {
    if !matches!(
        call.name.as_str(),
        "read_note" | "write_note" | "get_link_suggestions"
    ) {
        return None;
    }

    let Some(note_id) = call.input.get("note_id").and_then(Value::as_str) else {
        return None;
    };

    // Keep explicit note IDs usable even when they did not come from a search.
    // The common hallucination shape is a placeholder such as `note_123` or an
    // ID that is absent from the immediately returned search results.
    let looks_like_placeholder = note_id.contains('_') || note_id.ends_with("123");
    let conflicts_with_search =
        !known_notes.is_empty() && !known_notes.iter().any(|note| note.id == note_id);
    if looks_like_placeholder || conflicts_with_search {
        Some(format!(
            "Invalid note_id '{note_id}'. Retry using one of the exact note IDs from the latest search results."
        ))
    } else {
        None
    }
}

fn repair_note_id_arg(call: ToolCall, known_notes: &[KnownNoteRef]) -> (ToolCall, Option<String>) {
    if !matches!(call.name.as_str(), "read_note" | "get_link_suggestions") || known_notes.is_empty()
    {
        return (call, None);
    }

    let Some(note_id) = call
        .input
        .get("note_id")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return (call, None);
    };
    if known_notes.iter().any(|note| note.id == note_id) {
        return (call, None);
    }

    let Some(replacement) = known_notes.first() else {
        return (call, None);
    };
    let mut repaired = call;
    let original = note_id;
    repaired.input["note_id"] = json!(replacement.id);
    (
        repaired,
        Some(format!(
            "Repaired hallucinated note_id '{original}' to '{}' ({}) from the latest search results.",
            replacement.id, replacement.title
        )),
    )
}

fn enrich_tool_response(
    response: Value,
    call: &ToolCall,
    known_notes: &[KnownNoteRef],
    repair: Option<&str>,
) -> Value {
    if call.name == "search_notes" && !known_notes.is_empty() {
        return json!({
            "result": response,
            "available_note_ids": known_notes,
            "instruction": "Use available_note_ids[].id exactly when calling read_note.",
        });
    }

    if let Some(repair) = repair {
        return json!({
            "result": response,
            "agent_repair": repair,
            "instruction": "Continue using exact note IDs from available_note_ids or tool results.",
        });
    }

    let is_invalid_note_id = response
        .get("error")
        .and_then(Value::as_str)
        .is_some_and(|error| error.contains("Invalid note_id") || error.contains("Note not found"));
    if !is_invalid_note_id || known_notes.is_empty() {
        return response;
    }

    json!({
        "error": response.get("error").cloned().unwrap_or_else(|| json!("Invalid note_id")),
        "invalid_input": call.input,
        "available_note_ids": known_notes,
        "instruction": "Retry the tool call using one of available_note_ids[].id exactly.",
    })
}

fn extract_search_results(output: &Value) -> Vec<KnownNoteRef> {
    output
        .get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            Some(KnownNoteRef {
                id: item.get("id")?.as_str()?.to_string(),
                title: item
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("Untitled")
                    .to_string(),
            })
        })
        .collect()
}

fn merge_known_notes(known_notes: &mut Vec<KnownNoteRef>, new_notes: Vec<KnownNoteRef>) {
    for note in new_notes {
        if !known_notes.iter().any(|existing| existing.id == note.id) {
            known_notes.push(note);
        }
    }
}

fn tool_result_continuation_prompt(executions: &[ToolExecution]) -> String {
    let results = executions
        .iter()
        .enumerate()
        .map(|(index, execution)| {
            format!(
                "## Tool result {}\nTool: {}\nInput JSON:\n{}\nResult JSON:\n{}",
                index + 1,
                execution.call.name,
                serde_json::to_string_pretty(&execution.call.input)
                    .unwrap_or_else(|_| "{}".to_string()),
                serde_json::to_string_pretty(&execution.response)
                    .unwrap_or_else(|_| "{}".to_string()),
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    format!(
        "Continue from these exact tool results.\n\n{results}\n\nIf more information is needed, call another available tool. Otherwise give the final answer using only the facts explicitly present in these tool results. Do not invent occupations, background, relationships, dates, or motivations."
    )
}

fn assistant_tool_response_message(executions: &[ToolExecution]) -> Value {
    json!({
        "role": "assistant",
        "content": "",
        "tool_calls": executions
            .iter()
            .map(|execution| {
                json!({
                    "id": execution.call.id,
                    "type": "function",
                    "function": {
                        "name": execution.call.name,
                        "arguments": serde_json::to_string(&execution.call.input).unwrap_or_else(|_| "{}".to_string()),
                    }
                })
            })
            .collect::<Vec<_>>(),
        "tool_responses": executions
            .iter()
            .map(|execution| {
                json!({
                    "name": execution.call.name,
                    "response": execution.response,
                })
            })
            .collect::<Vec<_>>(),
    })
}

fn extract_gemma_tool_calls(text: &str) -> Vec<ToolCall> {
    let marker = "<|tool_call>call:";
    let end_marker = "}<tool_call|>";
    let mut calls = Vec::new();
    let mut offset = 0;

    while let Some(start) = text[offset..].find(marker) {
        let call_start = offset + start + marker.len();
        let Some(name_end_rel) = text[call_start..].find('{') else {
            break;
        };
        let name_end = call_start + name_end_rel;
        let name = text[call_start..name_end].trim();
        let args_start = name_end + 1;
        let Some(args_end_rel) = text[args_start..].find(end_marker) else {
            break;
        };
        let args_end = args_start + args_end_rel;
        if !name.is_empty() {
            calls.push(ToolCall {
                id: format!("call_{}", calls.len()),
                name: name.to_string(),
                input: Value::Object(parse_gemma_arguments(&text[args_start..args_end])),
            });
        }
        offset = args_end + end_marker.len();
    }

    calls
}

fn parse_gemma_arguments(args: &str) -> Map<String, Value> {
    let quote_marker = "<|\"|>";
    let mut parsed = Map::new();
    let mut rest = args.trim();

    while !rest.is_empty() {
        rest = rest.trim_start_matches(|ch: char| ch.is_whitespace() || ch == ',');
        if rest.is_empty() {
            break;
        }

        let Some(colon) = rest.find(':') else {
            break;
        };
        let key = rest[..colon]
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();
        let value_part = rest[colon + 1..].trim_start();

        let (value, consumed) = if let Some(value_part) = value_part.strip_prefix(quote_marker) {
            if let Some(end) = value_part.find(quote_marker) {
                (
                    Value::String(value_part[..end].to_string()),
                    quote_marker.len() + end + quote_marker.len(),
                )
            } else {
                (Value::String(value_part.to_string()), value_part.len())
            }
        } else {
            let end = value_part.find(',').unwrap_or(value_part.len());
            (cast_gemma_value(&value_part[..end]), end)
        };

        if !key.is_empty() {
            parsed.insert(key, value);
        }
        rest = &value_part[consumed.min(value_part.len())..];
    }

    parsed
}

fn cast_gemma_value(value: &str) -> Value {
    let value = value.trim().trim_matches('"').trim_matches('\'');
    if value.eq_ignore_ascii_case("true") {
        return Value::Bool(true);
    }
    if value.eq_ignore_ascii_case("false") {
        return Value::Bool(false);
    }
    if value.eq_ignore_ascii_case("null") {
        return Value::Null;
    }
    if let Ok(number) = value.parse::<i64>() {
        return json!(number);
    }
    if let Ok(number) = value.parse::<f64>() {
        return json!(number);
    }
    Value::String(value.to_string())
}

fn clean_model_text(text: String) -> String {
    text.replace("<|tool_response>", "")
        .replace("<tool_response|>", "")
        .replace("<|tool_call>", "")
        .replace("<tool_call|>", "")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gemma_tool_call_arguments() {
        let text = r#"<|tool_call>call:search_notes{query:<|"|>Big note<|"|>,limit:5}<tool_call|><|tool_response>"#;

        let calls = extract_gemma_tool_calls(text);

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "search_notes");
        assert_eq!(calls[0].input["query"], "Big note");
        assert_eq!(calls[0].input["limit"], 5);
    }

    #[test]
    fn repairs_hallucinated_read_note_ids_after_search_results() {
        let call = ToolCall {
            id: "call_0".to_string(),
            name: "read_note".to_string(),
            input: json!({ "note_id": "note_123" }),
        };
        let known_notes = vec![KnownNoteRef {
            id: "note-1781777544292".to_string(),
            title: "Ramu".to_string(),
        }];

        let (repaired, message) = repair_note_id_arg(call, &known_notes);

        assert_eq!(repaired.input["note_id"], "note-1781777544292");
        assert!(message.expect("repair message").contains("note_123"));
    }

    #[test]
    fn continuation_prompt_contains_read_note_content_as_plain_text() {
        let prompt = tool_result_continuation_prompt(&[ToolExecution {
            call: ToolCall {
                id: "call_0".to_string(),
                name: "read_note".to_string(),
                input: json!({ "note_id": "note-1" }),
            },
            response: json!({
                "id": "note-1",
                "title": "Ramu",
                "content": "I went to meet Ramu and it was a good interaction."
            }),
        }]);

        assert!(prompt.contains("I went to meet Ramu"));
        assert!(prompt.contains("Do not invent occupations"));
    }
}
