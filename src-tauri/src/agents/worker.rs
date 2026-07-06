//! Background agent worker — SCAFFOLD ONLY (intentionally not wired up yet).
//!
//! This module marks where long-running / background agent execution will live.
//! Per the plan we add a worker (and its `tauri::async_runtime::spawn`) only
//! once there is real background work to run, to avoid a spinning no-op task.
//!
//! When implemented, `lib.rs` will do (mirroring the existing `extraction_worker`):
//!
//! ```ignore
//! tauri::async_runtime::spawn(agents::worker::agent_worker(app.handle().clone()));
//! ```
//!
//! and the worker will:
//! 1. Poll an `agent_tasks` table (Chunk 3) for queued work.
//! 2. Drive `AgentRuntime::execute_tool` in a plan→act loop, recording
//!    `agent_runs` / `agent_events` for observability and approval gating.
//! 3. Emit Tauri events so the frontend can render live progress.
