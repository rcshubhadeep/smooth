//! Internal agent runtime.
//!
//! A self-contained framework for defining tools and executing them, deliberately
//! independent of MCP. MCP (server + client) is intended to become an *adapter*
//! layered on top of `AgentRuntime` / `ToolRegistry` later — it is NOT the core.
//!
//! Module layout:
//! - `context`  — `AgentContext` handed to tools (wraps `AppHandle`).
//! - `tool`     — the `AgentTool` trait.
//! - `registry` — thread-safe `ToolRegistry`.
//! - `runtime`  — `AgentRuntime`, the single execution entry point.
//! - `tools`    — built-in tool implementations + registration.
//! - `worker`   — background worker scaffold (not spawned yet).

pub mod context;
pub mod flow;
pub mod registry;
pub mod runtime;
pub mod tool;
pub mod tools;
pub mod worker;

pub use context::AgentContext;
pub use registry::ToolDescriptor;
pub use runtime::AgentRuntime;

use serde_json::Value;
use tauri::{AppHandle, State};

/// Tauri bridge: execute one tool by name from the frontend.
///
/// Deliberately defined here (not in `lib.rs`) so `lib.rs` only needs
/// `mod agents;`, one `.manage(...)` and these handler entries.
#[tauri::command]
pub(crate) async fn agent_execute_tool(
    app: AppHandle,
    runtime: State<'_, AgentRuntime>,
    tool: String,
    input: Value,
) -> Result<Value, String> {
    let ctx = AgentContext::new(app);
    runtime
        .execute_tool(&tool, input, &ctx)
        .await
        .map_err(|error| error.to_string())
}

/// Tauri bridge: list registered tools (debugging / future agent UI).
#[tauri::command]
pub(crate) fn agent_list_tools(runtime: State<'_, AgentRuntime>) -> Vec<ToolDescriptor> {
    runtime.list_tools()
}

/// Tauri bridge: run a bounded foreground agent loop using the registered tools.
#[tauri::command]
pub(crate) async fn agent_run(
    app: AppHandle,
    runtime: State<'_, AgentRuntime>,
    prompt: String,
    max_steps: Option<u8>,
) -> Result<flow::AgentRunResult, String> {
    flow::run_agent_once(app, &runtime, prompt, max_steps).await
}
