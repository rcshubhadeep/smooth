//! Thread-safe registry of tools.
//!
//! A `RwLock<HashMap>` keeps registration flexible — tools are added at startup
//! today, but a future MCP *client* adapter could register remote tools at
//! runtime — while keeping the read-heavy lookup path cheap. Tools are held as
//! `Arc<dyn AgentTool>` so they can be shared across the worker, command
//! handlers and (later) agent loops without cloning the tool itself.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use serde_json::Value;

use super::tool::AgentTool;

/// Serializable description of a registered tool, for discovery / debugging and
/// (later) advertising tools to an LLM or over MCP `tools/list`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Default)]
pub struct ToolRegistry {
    tools: RwLock<HashMap<&'static str, Arc<dyn AgentTool>>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, tool: Arc<dyn AgentTool>) {
        self.tools
            .write()
            .expect("tool registry lock poisoned")
            .insert(tool.name(), tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn AgentTool>> {
        self.tools
            .read()
            .expect("tool registry lock poisoned")
            .get(name)
            .cloned()
    }

    pub fn list(&self) -> Vec<ToolDescriptor> {
        self.tools
            .read()
            .expect("tool registry lock poisoned")
            .values()
            .map(|tool| ToolDescriptor {
                name: tool.name().to_string(),
                description: tool.description().to_string(),
                input_schema: tool.input_schema(),
            })
            .collect()
    }
}
