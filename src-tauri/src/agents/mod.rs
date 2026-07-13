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
pub mod follow_up;
pub mod persistence;
pub mod registry;
pub mod runtime;
pub mod tool;
pub mod tools;
pub mod worker;

pub use context::AgentContext;
pub use registry::ToolDescriptor;
pub use runtime::AgentRuntime;

pub(crate) use persistence::init_schema;

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

/// Tauri bridge: inspect recent persisted agent runs.
#[tauri::command]
pub(crate) fn agent_list_runs(
    app: AppHandle,
    options: Option<persistence::AgentRunListOptions>,
) -> Result<Vec<persistence::AgentRunRecord>, String> {
    persistence::list_runs(app, options)
}

/// Tauri bridge: inspect the ordered event trace for one persisted run.
#[tauri::command]
pub(crate) fn agent_get_run_events(
    app: AppHandle,
    run_id: String,
) -> Result<Vec<persistence::AgentEventRecord>, String> {
    persistence::get_events(app, run_id)
}

// --- User-defined agent definitions (Phase 2) -----------------------------

#[tauri::command]
pub(crate) fn agent_list_definitions(
    app: AppHandle,
) -> Result<Vec<persistence::AgentDefinitionRecord>, String> {
    persistence::list_definitions(app)
}

#[tauri::command]
pub(crate) fn agent_create_definition(
    app: AppHandle,
    definition: persistence::AgentDefinitionInput,
) -> Result<persistence::AgentDefinitionRecord, String> {
    persistence::create_definition(app, definition)
}

#[tauri::command]
pub(crate) fn agent_update_definition(
    app: AppHandle,
    id: String,
    definition: persistence::AgentDefinitionInput,
) -> Result<persistence::AgentDefinitionRecord, String> {
    persistence::update_definition(app, id, definition)
}

#[tauri::command]
pub(crate) fn agent_delete_definition(app: AppHandle, id: String) -> Result<(), String> {
    persistence::delete_definition(app, id)
}
