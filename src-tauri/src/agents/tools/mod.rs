//! Built-in tools and their registration.
//!
//! Each tool lives in a small domain module. Tools use `AgentContext` methods
//! rather than opening SQLite or touching the filesystem directly, which keeps
//! authorization, approvals and audit logging possible at the runtime/context
//! layer later.

use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use super::context::AgentContext;
use super::registry::ToolRegistry;
use super::tool::AgentTool;

pub(crate) mod links;
pub(crate) mod notes;
pub(crate) mod search;

/// Register every built-in tool into the registry.
pub(crate) fn register_builtin_tools(registry: &ToolRegistry) {
    registry.register(Arc::new(PingTool));
    registry.register(Arc::new(notes::ReadNoteTool));
    registry.register(Arc::new(notes::CreateNoteTool));
    registry.register(Arc::new(notes::WriteNoteTool));
    registry.register(Arc::new(search::SearchNotesTool));
    registry.register(Arc::new(links::GetLinkSuggestionsTool));
}

pub(crate) fn parse_input<T>(input: Value) -> anyhow::Result<T>
where
    T: DeserializeOwned,
{
    serde_json::from_value(input).context("Invalid tool input")
}

/// Trivial tool that echoes its input — used to verify runtime wiring.
struct PingTool;

#[async_trait]
impl AgentTool for PingTool {
    fn name(&self) -> &'static str {
        "ping"
    }

    fn description(&self) -> &'static str {
        "Health check: echoes the given input back as { \"pong\": <input> }."
    }

    async fn execute(&self, _ctx: &AgentContext, input: Value) -> anyhow::Result<Value> {
        Ok(json!({ "pong": input }))
    }
}
