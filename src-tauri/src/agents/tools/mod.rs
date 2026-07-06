//! Built-in tools and their registration.
//!
//! Chunk 1 ships a single `PingTool` to exercise the trait → registry → runtime
//! → Tauri-command path end-to-end. Chunk 2 adds the real note/search/link tools
//! in submodules (`notes`, `search`, `links`) and registers them here — adding a
//! tool then stays a one-liner, and the worker / future MCP adapter get it free.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::context::AgentContext;
use super::registry::ToolRegistry;
use super::tool::AgentTool;

/// Register every built-in tool into the registry.
pub(crate) fn register_builtin_tools(registry: &ToolRegistry) {
    registry.register(Arc::new(PingTool));
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
