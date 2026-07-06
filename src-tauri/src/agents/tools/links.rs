use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::agents::context::AgentContext;
use crate::agents::tool::AgentTool;
use crate::agents::tools::parse_input;

pub(crate) struct GetLinkSuggestionsTool;

#[derive(Debug, Deserialize)]
struct GetLinkSuggestionsInput {
    note_id: String,
    limit: Option<u32>,
}

#[async_trait]
impl AgentTool for GetLinkSuggestionsTool {
    fn name(&self) -> &'static str {
        "get_link_suggestions"
    }

    fn description(&self) -> &'static str {
        "Suggest related notes using shared extracted entities."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["note_id"],
            "properties": {
                "note_id": { "type": "string", "description": "Source note ID." },
                "limit": {
                    "type": ["integer", "null"],
                    "minimum": 1,
                    "maximum": 20,
                    "description": "Maximum suggestion count."
                }
            },
            "additionalProperties": false
        })
    }

    async fn execute(&self, ctx: &AgentContext, input: Value) -> anyhow::Result<Value> {
        let input = parse_input::<GetLinkSuggestionsInput>(input)?;
        let suggestions = ctx
            .link_suggestions(input.note_id, input.limit)
            .map_err(anyhow::Error::msg)?;
        Ok(json!({ "suggestions": suggestions }))
    }
}
