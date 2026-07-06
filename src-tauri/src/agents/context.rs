//! Execution context handed to every tool.
//!
//! A tool receives exactly two things: this context and its JSON input. The
//! context wraps the Tauri `AppHandle`; tools reach application data only
//! through curated helpers (added alongside the note/link tools in Chunk 2) —
//! never through raw SQLite. Keeping a single, vetted choke point here is what
//! lets us later bolt on per-tool auth, approval gating and auditing without
//! touching individual tools.

use tauri::AppHandle;

pub struct AgentContext {
    pub app: AppHandle,
}

impl AgentContext {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}
