use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::agents::context::AgentContext;
use crate::agents::tool::AgentTool;
use crate::agents::tools::parse_input;

pub(crate) struct SearchNotesTool;

#[derive(Debug, Deserialize)]
struct SearchNotesInput {
    query: String,
    limit: Option<u32>,
}

#[async_trait]
impl AgentTool for SearchNotesTool {
    fn name(&self) -> &'static str {
        "search_notes"
    }

    fn description(&self) -> &'static str {
        "Search active notes by title or markdown content."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": { "type": "string", "description": "Text to search for." },
                "limit": {
                    "type": ["integer", "null"],
                    "minimum": 1,
                    "maximum": 100,
                    "description": "Maximum result count."
                }
            },
            "additionalProperties": false
        })
    }

    async fn execute(&self, ctx: &AgentContext, input: Value) -> anyhow::Result<Value> {
        let input = parse_input::<SearchNotesInput>(input)?;
        let results = ctx
            .search_notes(&input.query, input.limit)
            .map_err(anyhow::Error::msg)?;
        Ok(json!({ "results": results }))
    }
}
