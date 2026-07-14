//! Specialized meeting follow-up agent.
//!
//! This is deliberately not a generic tool loop: meeting transcripts can be
//! larger than the model context. It uses chat's hierarchical note-memory path,
//! records a normal agent run, and returns a proposal only. Gmail draft creation
//! remains a separate, explicit user action.

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::AppHandle;

use crate::chat;

use super::context::AgentContext;
use super::persistence::{AgentEvent, AgentRunRecorder};

const FOLLOW_UP_PROMPT: &str = r#"Write a super short and casual follow-up email that I can send based on the meeting provided.

The email should be action oriented instead of focusing on what was discussed. In general, if there's any information you want to include in the email but you don't have it, put a placeholder in (e.g. "[Insert current ARR]" or "[Insert LINK to DPA]" or "[Insert slide-deck-link]" or "[Attach DPA]").

When I promised to do something in the meeting:
- If it's something that can be done in under 5 minutes (e.g. find a document, look up some information) assume that I've done it already and put in placeholders as needed. For example, let's say I promised to reschedule our next meeting to later, you could write "I rescheduled our next meeting to [Insert DATE]".
- If it takes more than a couple minutes and is important, mention that I'll do it.

When other people promised to do something:
- If it's important, mention the things other people promised to do. It's always good to push people toward action, so instead of saying that Amanda needs to do X, perhaps say "Amanda, when do you think you'll be able to do X by?"

Do not quote the transcript directly within the email, this messes up the formatting.

Return only valid JSON with exactly these string fields:
{"subject":"Short subject","body":"Plain-text email body"}
Do not include recipients, Markdown fences, commentary, or extra fields."#;

#[derive(Debug, Deserialize)]
pub(crate) struct FollowUpEmailRequest {
    note_id: String,
}

#[derive(Debug, Deserialize)]
struct ModelEmailDraft {
    subject: String,
    body: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct FollowUpEmailDraft {
    run_id: String,
    model: String,
    subject: String,
    body: String,
    used_summary: bool,
}

#[tauri::command]
pub(crate) async fn prepare_follow_up_email(
    app: AppHandle,
    request: FollowUpEmailRequest,
) -> Result<FollowUpEmailDraft, String> {
    let note_id = request.note_id.trim();
    if note_id.is_empty() {
        return Err("note_id is required".to_string());
    }

    let context = AgentContext::new(app.clone());
    let note = context.read_note(note_id)?;
    if note.content.trim().is_empty() {
        return Err("This note is empty, so there is no meeting to follow up on.".to_string());
    }

    let run_prompt = format!(
        "Write a follow-up email for note '{}' ({})",
        note.title, note.id
    );
    let mut recorder = AgentRunRecorder::start(
        app.clone(),
        Some("meeting-follow-up-email"),
        "foreground",
        &run_prompt,
        1,
    )?;
    let note_event = json!({
        "note_id": note.id,
        "title": note.title,
        "content_chars": note.content.chars().count(),
    });
    recorder.record(AgentEvent {
        event_type: "note_context",
        role: Some("tool"),
        tool_name: Some("read_note"),
        content: None,
        input: None,
        output: Some(&note_event),
        error: None,
    })?;

    let outcome = chat::complete_grounded_note_task(
        &app,
        &note.id,
        &note.title,
        &note.content,
        FOLLOW_UP_PROMPT,
        Some(email_response_format()),
    )
    .await;

    match outcome {
        Ok(completion) => {
            recorder.set_model(&completion.model, &completion.base_url)?;
            let parsed = match parse_email_draft(&completion.answer, &note.title) {
                Ok(parsed) => parsed,
                Err(error) => {
                    let _ = recorder.record(AgentEvent {
                        event_type: "model_response",
                        role: Some("assistant"),
                        tool_name: None,
                        content: Some(&completion.answer),
                        input: None,
                        output: None,
                        error: Some(&error),
                    });
                    let _ = recorder.complete_failure(&error);
                    return Err(error);
                }
            };
            let result_json = json!({
                "subject": parsed.subject,
                "body": parsed.body,
                "used_summary": completion.used_summary,
            });
            recorder.record(AgentEvent {
                event_type: "final_answer",
                role: Some("assistant"),
                tool_name: None,
                content: Some(&completion.answer),
                input: None,
                output: Some(&result_json),
                error: None,
            })?;
            recorder.complete_success(&parsed.body, &completion.answer)?;

            Ok(FollowUpEmailDraft {
                run_id: recorder.run_id().to_string(),
                model: completion.model,
                subject: parsed.subject,
                body: parsed.body,
                used_summary: completion.used_summary,
            })
        }
        Err(error) => {
            let _ = recorder.complete_failure(&error);
            Err(error)
        }
    }
}

fn email_response_format() -> serde_json::Value {
    json!({
        "type": "json_schema",
        "json_schema": {
            "name": "follow_up_email",
            "strict": true,
            "schema": {
                "type": "object",
                "properties": {
                    "subject": { "type": "string" },
                    "body": { "type": "string" }
                },
                "required": ["subject", "body"],
                "additionalProperties": false
            }
        }
    })
}

fn parse_email_draft(raw: &str, note_title: &str) -> Result<ModelEmailDraft, String> {
    let cleaned = clean_model_response(raw);
    let trimmed = cleaned.trim();
    let json_text = serde_json::from_str::<ModelEmailDraft>(trimmed)
        .ok()
        .or_else(|| parse_first_email_object(trimmed))
        .or_else(|| parse_labeled_email(trimmed))
        .or_else(|| {
            (!trimmed.is_empty()).then(|| ModelEmailDraft {
                subject: fallback_subject(note_title),
                body: trimmed.to_string(),
            })
        })
        .ok_or_else(|| {
            "The model returned an empty email draft. Run the agent again.".to_string()
        })?;

    let subject = json_text.subject.trim().to_string();
    let body = json_text.body.trim().to_string();
    if subject.is_empty() || body.is_empty() {
        return Err(
            "The model returned an incomplete email draft. Run the agent again.".to_string(),
        );
    }
    Ok(ModelEmailDraft { subject, body })
}

fn clean_model_response(raw: &str) -> String {
    let final_markers = ["<|channel>final", "<|channel|>final"];
    let selected = final_markers
        .iter()
        .find_map(|marker| raw.rsplit_once(marker).map(|(_, final_text)| final_text))
        .unwrap_or(raw);

    selected
        .lines()
        .filter(|line| {
            let line = line.trim();
            !(line.starts_with("<|channel>")
                || line.starts_with("<|channel|>")
                || line == "```json"
                || line == "```")
        })
        .collect::<Vec<_>>()
        .join("\n")
        .replace("<|end|>", "")
        .replace("<turn|>", "")
        .trim()
        .to_string()
}

fn parse_labeled_email(text: &str) -> Option<ModelEmailDraft> {
    let subject_start = text.to_ascii_lowercase().find("subject:")?;
    let after_subject = &text[subject_start + "subject:".len()..];
    let body_offset = after_subject.to_ascii_lowercase().find("body:")?;
    let subject = after_subject[..body_offset].trim().to_string();
    let body = after_subject[body_offset + "body:".len()..]
        .trim()
        .to_string();
    (!subject.is_empty() && !body.is_empty()).then_some(ModelEmailDraft { subject, body })
}

fn fallback_subject(note_title: &str) -> String {
    let title = note_title.trim();
    if title.is_empty() {
        "Following up".to_string()
    } else {
        format!("Following up: {title}")
    }
}

fn parse_first_email_object(text: &str) -> Option<ModelEmailDraft> {
    text.match_indices('{').find_map(|(start, _)| {
        serde_json::Deserializer::from_str(&text[start..])
            .into_iter::<ModelEmailDraft>()
            .next()
            .and_then(Result::ok)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_fenced_json() {
        let plain = parse_email_draft(
            r#"{"subject":"Next steps","body":"Hi team,\\n\\nThanks!"}"#,
            "Meeting",
        )
        .expect("plain JSON");
        assert_eq!(plain.subject, "Next steps");

        let fenced = parse_email_draft(
            "<|channel>thought\n{not valid}\n<|channel>final\n```json\n{\"subject\":\"Follow up\",\"body\":\"Hi all\"}\n```",
            "Meeting",
        )
        .expect("embedded JSON");
        assert_eq!(fenced.body, "Hi all");
    }

    #[test]
    fn accepts_labeled_and_plain_text_fallbacks() {
        let labeled = parse_email_draft(
            "Subject: Next steps\nBody:\nHi team,\n\nI'll send [Insert LINK].",
            "Meeting",
        )
        .expect("labeled email");
        assert_eq!(labeled.subject, "Next steps");

        let plain = parse_email_draft("Hi team,\n\nThanks for the time.", "Customer call")
            .expect("plain email");
        assert_eq!(plain.subject, "Following up: Customer call");
    }
}
