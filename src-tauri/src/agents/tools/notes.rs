use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::agents::context::AgentContext;
use crate::agents::tool::AgentTool;
use crate::agents::tools::parse_input;

pub(crate) struct ReadNoteTool;
pub(crate) struct CreateNoteTool;
pub(crate) struct WriteNoteTool;

#[derive(Debug, Deserialize)]
struct ReadNoteInput {
    note_id: String,
}

#[derive(Debug, Deserialize)]
struct CreateNoteInput {
    title: Option<String>,
    folder_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WriteNoteInput {
    note_id: String,
    content: String,
}

#[async_trait]
impl AgentTool for ReadNoteTool {
    fn name(&self) -> &'static str {
        "read_note"
    }

    fn description(&self) -> &'static str {
        "Read a note by ID and return its title and markdown content."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["note_id"],
            "properties": {
                "note_id": { "type": "string", "description": "ID of the note to read." }
            },
            "additionalProperties": false
        })
    }

    async fn execute(&self, ctx: &AgentContext, input: Value) -> anyhow::Result<Value> {
        let input = parse_input::<ReadNoteInput>(input)?;
        let note = ctx.read_note(&input.note_id).map_err(anyhow::Error::msg)?;
        Ok(json!({
            "id": note.id,
            "title": note.title,
            "content": note.content
        }))
    }
}

#[async_trait]
impl AgentTool for CreateNoteTool {
    fn name(&self) -> &'static str {
        "create_note"
    }

    fn description(&self) -> &'static str {
        "Create an empty note with an optional title and folder ID."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": { "type": ["string", "null"], "description": "Initial note title." },
                "folder_id": { "type": ["string", "null"], "description": "Folder ID or null for Inbox." }
            },
            "additionalProperties": false
        })
    }

    async fn execute(&self, ctx: &AgentContext, input: Value) -> anyhow::Result<Value> {
        let input = parse_input::<CreateNoteInput>(input)?;
        let note = ctx
            .create_note(input.title, input.folder_id)
            .map_err(anyhow::Error::msg)?;
        Ok(serde_json::to_value(note)?)
    }
}

#[async_trait]
impl AgentTool for WriteNoteTool {
    fn name(&self) -> &'static str {
        "write_note"
    }

    fn description(&self) -> &'static str {
        "Replace a note's markdown content while preserving its title and folder."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["note_id", "content"],
            "properties": {
                "note_id": { "type": "string", "description": "ID of the note to update." },
                "content": { "type": "string", "description": "New markdown body." }
            },
            "additionalProperties": false
        })
    }

    async fn execute(&self, ctx: &AgentContext, input: Value) -> anyhow::Result<Value> {
        let input = parse_input::<WriteNoteInput>(input)?;
        let note = ctx
            .write_note(&input.note_id, input.content)
            .map_err(anyhow::Error::msg)?;
        Ok(serde_json::to_value(note)?)
    }
}
