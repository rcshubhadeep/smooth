use serde::{Deserialize, Serialize};

pub(crate) const MIB: u64 = 1024 * 1024;

#[derive(Clone, Copy)]
pub(crate) struct ImportLimits {
    pub(crate) max_files_per_batch: usize,
    pub(crate) max_batch_bytes: u64,
    pub(crate) max_pdf_bytes: u64,
    pub(crate) max_office_bytes: u64,
    pub(crate) max_structured_bytes: u64,
    pub(crate) max_text_bytes: u64,
    pub(crate) max_pdf_pages: u32,
    pub(crate) max_expanded_archive_bytes: usize,
    pub(crate) max_archive_entries: usize,
    pub(crate) max_archive_expansion_ratio: u64,
    pub(crate) markdown_warning_bytes: usize,
    pub(crate) max_markdown_bytes: usize,
    pub(crate) max_asset_bytes: usize,
    pub(crate) max_total_asset_bytes: usize,
}

impl Default for ImportLimits {
    fn default() -> Self {
        Self {
            max_files_per_batch: 50,
            max_batch_bytes: 500 * MIB,
            max_pdf_bytes: 150 * MIB,
            max_office_bytes: 100 * MIB,
            max_structured_bytes: 50 * MIB,
            max_text_bytes: 20 * MIB,
            max_pdf_pages: 750,
            max_expanded_archive_bytes: 750 * MIB as usize,
            max_archive_entries: 20_000,
            max_archive_expansion_ratio: 100,
            markdown_warning_bytes: 2 * MIB as usize,
            max_markdown_bytes: 8 * MIB as usize,
            max_asset_bytes: 30 * MIB as usize,
            max_total_asset_bytes: 300 * MIB as usize,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct EnqueueImportsRequest {
    pub(crate) paths: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ImportJobRecord {
    pub(crate) id: String,
    pub(crate) source_name: String,
    pub(crate) source_size: u64,
    pub(crate) format: String,
    pub(crate) status: String,
    pub(crate) attempts: u32,
    pub(crate) max_attempts: u32,
    pub(crate) note_id: Option<String>,
    pub(crate) warnings: Vec<String>,
    pub(crate) error: Option<String>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ImportJobClaim {
    pub(crate) id: String,
    pub(crate) source_path: String,
    pub(crate) source_name: String,
    pub(crate) source_size: u64,
    pub(crate) format: String,
    pub(crate) attempts: u32,
    pub(crate) allow_duplicate: bool,
}

pub(crate) struct ImportFailure {
    pub(crate) message: String,
    pub(crate) retryable: bool,
}

impl ImportFailure {
    pub(crate) fn permanent(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
        }
    }

    pub(crate) fn retryable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
        }
    }
}
