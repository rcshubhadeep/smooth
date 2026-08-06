use std::path::Path;

use anydoc::ConvertError;

use super::{ConvertedDocument, DocumentConverter};
use crate::imports::types::{ImportFailure, ImportLimits};

pub(crate) struct AnydocConverter;

pub(crate) fn supports_format(format: &str) -> bool {
    matches!(
        format,
        "doc"
            | "docx"
            | "docm"
            | "ppt"
            | "pps"
            | "pot"
            | "pptx"
            | "pptm"
            | "ppsx"
            | "ppsm"
            | "xls"
            | "xlsx"
            | "xlsm"
            | "xlsb"
            | "odt"
            | "ods"
            | "odp"
            | "rtf"
            | "epub"
            | "csv"
            | "pdf"
    )
}

impl DocumentConverter for AnydocConverter {
    fn supports(&self, format: &str) -> bool {
        supports_format(format)
    }

    fn convert(
        &self,
        path: &Path,
        _limits: ImportLimits,
    ) -> Result<ConvertedDocument, ImportFailure> {
        let markdown =
            anydoc::to_markdown(path).map_err(|error| conversion_failure(error, path))?;

        Ok(ConvertedDocument {
            title: None,
            markdown,
            // Anydoc preserves embedded assets in its document model and emits
            // their alt text in Markdown. Its direct Markdown API does not
            // expose those bytes, so there is nothing to persist here.
            assets: Vec::new(),
            warnings: Vec::new(),
        })
    }
}

fn conversion_failure(error: ConvertError, path: &Path) -> ImportFailure {
    let is_pdf = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"));

    match error {
        ConvertError::Unsupported(_) if is_pdf => ImportFailure::permanent(
            "This PDF appears to be scanned or image-only, so it can't be imported. OCR is required before trying again.",
        ),
        ConvertError::Encrypted => ImportFailure::permanent(
            "This document is password-protected. Remove the password and try again.",
        ),
        ConvertError::Io(error) => {
            ImportFailure::retryable(format!("Could not read document: {error}"))
        }
        error => ImportFailure::permanent(format!("Document conversion failed: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanned_pdf_error_explains_that_ocr_is_required() {
        let failure = conversion_failure(
            ConvertError::Unsupported("PDF has no extractable text".to_string()),
            Path::new("scan.PDF"),
        );

        assert_eq!(
            failure.message,
            "This PDF appears to be scanned or image-only, so it can't be imported. OCR is required before trying again."
        );
        assert!(!failure.retryable);
    }

    #[test]
    fn supports_every_anydoc_extension() {
        for format in [
            "doc", "docx", "docm", "ppt", "pps", "pot", "pptx", "pptm", "ppsx", "ppsm", "xls",
            "xlsx", "xlsm", "xlsb", "odt", "ods", "odp", "rtf", "epub", "csv", "pdf",
        ] {
            assert!(supports_format(format), "missing {format}");
        }
    }
}
