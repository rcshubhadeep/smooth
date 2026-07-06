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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use serde_json::{json, Value};

    use crate::agents::context::AgentContext;

    use super::*;

    struct TestTool {
        name: &'static str,
        description: &'static str,
    }

    #[async_trait]
    impl AgentTool for TestTool {
        fn name(&self) -> &'static str {
            self.name
        }

        fn description(&self) -> &'static str {
            self.description
        }

        fn input_schema(&self) -> Value {
            json!({
                "type": "object",
                "properties": {
                    "value": { "type": "string" }
                }
            })
        }

        async fn execute(&self, _ctx: &AgentContext, input: Value) -> anyhow::Result<Value> {
            Ok(json!({ "tool": self.name, "input": input }))
        }
    }

    #[test]
    fn lists_registered_tools_with_descriptors() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(TestTool {
            name: "test_tool",
            description: "A test tool",
        }));

        let tools = registry.list();

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "test_tool");
        assert_eq!(tools[0].description, "A test tool");
        assert_eq!(tools[0].input_schema["type"], "object");
    }

    #[test]
    fn registering_the_same_name_replaces_the_previous_tool() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(TestTool {
            name: "duplicate",
            description: "First",
        }));
        registry.register(Arc::new(TestTool {
            name: "duplicate",
            description: "Second",
        }));

        let tool = registry.get("duplicate").expect("registered tool");

        assert_eq!(tool.description(), "Second");
        assert_eq!(registry.list().len(), 1);
    }

    #[test]
    fn missing_tools_return_none() {
        let registry = ToolRegistry::new();

        assert!(registry.get("missing").is_none());
    }
}
