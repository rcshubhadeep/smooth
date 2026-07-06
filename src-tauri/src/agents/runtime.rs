//! `AgentRuntime`: owns the tool registry and is the single entry point for
//! executing tools. It is `.manage()`d by Tauri so command handlers and (later)
//! a background worker share one instance — composition over global state.
//!
//! Every higher-level capability is designed to funnel through `execute_tool`,
//! which keeps future features additive rather than invasive:
//! - agent loops: an LLM plan→act cycle that repeatedly calls `execute_tool`.
//! - background agents / scheduling: `worker` pulls tasks and drives the runtime.
//! - approval workflows: `execute_tool` can consult an approval gate first.
//! - memory / observability: runs & events can be recorded around each call.
//! - MCP: an adapter can expose `list_tools()` + `execute_tool` over MCP, and an
//!   MCP client adapter can register remote tools into the same registry.

use serde_json::Value;

use super::context::AgentContext;
use super::registry::{ToolDescriptor, ToolRegistry};
use super::tools;

pub struct AgentRuntime {
    registry: ToolRegistry,
}

impl AgentRuntime {
    pub fn new() -> Self {
        let registry = ToolRegistry::new();
        tools::register_builtin_tools(&registry);
        Self { registry }
    }

    pub fn list_tools(&self) -> Vec<ToolDescriptor> {
        self.registry.list()
    }

    /// Execute a single tool by name. This is the seam that agent loops, the
    /// worker and any MCP adapter all funnel through, so cross-cutting concerns
    /// (approval, auditing, rate limiting) can be added in one place.
    pub async fn execute_tool(
        &self,
        name: &str,
        input: Value,
        ctx: &AgentContext,
    ) -> anyhow::Result<Value> {
        let tool = self
            .registry
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Unknown tool: {name}"))?;
        tool.execute(ctx, input).await
    }
}

impl Default for AgentRuntime {
    fn default() -> Self {
        Self::new()
    }
}
