//! The `AgentTool` trait: one capability the runtime can invoke.
//!
//! Design decisions:
//! - `#[async_trait]`: `execute` is async, and the registry stores tools as
//!   `Arc<dyn AgentTool>`. Native async-fn-in-trait is not yet `dyn`-compatible
//!   on stable Rust, so the (small, standard) `async-trait` crate is warranted.
//! - JSON in / JSON out: keeping the boundary as `serde_json::Value` means the
//!   same tool can back a local LLM function-calling loop today and an MCP
//!   adapter later, with no change to tools.
//! - `input_schema` is provided up front (default: open object) so we can
//!   advertise JSON-Schema to an LLM or an MCP `tools/list` without a breaking
//!   trait change down the line.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::context::AgentContext;

#[async_trait]
pub trait AgentTool: Send + Sync {
    /// Stable identifier used for lookup and LLM/MCP tool advertising.
    fn name(&self) -> &'static str;

    /// Human/LLM-readable description of what the tool does.
    fn description(&self) -> &'static str;

    /// JSON Schema for the tool's input. Tools should override this once their
    /// input shape matters (LLM function-calling / MCP); the default accepts
    /// any object.
    fn input_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    /// Run the tool. Errors are `anyhow::Error` so implementations can use `?`
    /// freely; the runtime converts them to strings at the Tauri boundary.
    async fn execute(&self, ctx: &AgentContext, input: Value) -> anyhow::Result<Value>;
}
