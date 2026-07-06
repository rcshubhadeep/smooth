//! Execution context handed to every tool.
//!
//! A tool receives exactly two things: this context and its JSON input. The
//! context wraps the Tauri `AppHandle`; tools reach application data only
//! through curated helpers (added alongside the note/link tools in Chunk 2) —
//! never through raw SQLite. Keeping a single, vetted choke point here is what
//! lets us later bolt on per-tool auth, approval gating and auditing without
//! touching individual tools.

use serde::Serialize;
use tauri::AppHandle;

use crate::{
    create_standard_note, get_link_suggestions_internal, load_note_meta, open_database,
    read_note_content, save_note_internal, snapshot, LinkSuggestion, NoteWithContent,
};

pub struct AgentContext {
    pub app: AppHandle,
}

impl AgentContext {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }

    /// Read a note through the same metadata/content helpers used by Tauri
    /// commands. Tools never receive a database connection.
    pub(crate) fn read_note(&self, note_id: &str) -> Result<NoteWithContent, String> {
        let connection = open_database(&self.app)?;
        let note = load_note_meta(&connection, note_id)?;
        let content = read_note_content(&self.app, &note.id)?;

        Ok(NoteWithContent {
            id: note.id,
            title: note.title,
            folder_id: note.folder_id,
            created_at: note.created_at,
            updated_at: note.updated_at,
            deleted_at: note.deleted_at,
            content,
            extraction_status: note.extraction_status,
        })
    }

    pub(crate) fn create_note(
        &self,
        title: Option<String>,
        folder_id: Option<String>,
    ) -> Result<NoteWithContent, String> {
        create_standard_note(self.app.clone(), title, folder_id)
    }

    /// Replace a note body while preserving the current title/folder and the
    /// normal extraction behavior. Higher-level tools can choose to add title
    /// changes later without widening this minimal primitive now.
    pub(crate) fn write_note(
        &self,
        note_id: &str,
        content: String,
    ) -> Result<NoteWithContent, String> {
        let connection = open_database(&self.app)?;
        let note = load_note_meta(&connection, note_id)?;
        drop(connection);

        save_note_internal(
            self.app.clone(),
            note.id,
            note.title,
            content,
            note.folder_id,
            false,
        )
    }

    pub(crate) fn search_notes(
        &self,
        query: &str,
        limit: Option<u32>,
    ) -> Result<Vec<AgentSearchResult>, String> {
        let clean_query = query.trim().to_lowercase();
        if clean_query.is_empty() {
            return Ok(Vec::new());
        }

        let connection = open_database(&self.app)?;
        let bank = snapshot(&self.app, &connection)?;
        let limit = limit.unwrap_or(20).clamp(1, 100) as usize;
        let mut results = Vec::new();

        for note in bank.notes {
            if note.deleted_at.is_some() {
                continue;
            }

            let content = read_note_content(&self.app, &note.id)?;
            if note.title.to_lowercase().contains(&clean_query)
                || content.to_lowercase().contains(&clean_query)
            {
                results.push(AgentSearchResult {
                    id: note.id,
                    title: note.title,
                    excerpt: note.excerpt,
                    folder_id: note.folder_id,
                    updated_at: note.updated_at,
                });
            }

            if results.len() >= limit {
                break;
            }
        }

        Ok(results)
    }

    pub(crate) fn link_suggestions(
        &self,
        note_id: String,
        limit: Option<u32>,
    ) -> Result<Vec<LinkSuggestion>, String> {
        get_link_suggestions_internal(self.app.clone(), note_id, limit)
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct AgentSearchResult {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) excerpt: String,
    pub(crate) folder_id: Option<String>,
    pub(crate) updated_at: String,
}
