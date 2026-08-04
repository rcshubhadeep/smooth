use std::path::Path;

use pdf_inspector::{process_pdf, PdfType};

use super::{ConvertedAsset, ConvertedDocument, DocumentConverter};
use crate::imports::types::{ImportFailure, ImportLimits};

pub(crate) struct PdfConverter;

impl DocumentConverter for PdfConverter {
    fn supports(&self, format: &str) -> bool {
        format == "pdf"
    }

    fn convert(
        &self,
        path: &Path,
        limits: ImportLimits,
    ) -> Result<ConvertedDocument, ImportFailure> {
        let result = process_pdf(path)
            .map_err(|error| ImportFailure::permanent(format!("PDF parsing failed: {error}")))?;
        let page_count = result.page_count;
        if page_count > limits.max_pdf_pages {
            return Err(ImportFailure::permanent(format!(
                "PDF has {page_count} pages; the limit is {}",
                limits.max_pdf_pages
            )));
        }

        if matches!(result.pdf_type, PdfType::Scanned | PdfType::ImageBased) {
            return Err(ImportFailure::permanent(
                "OCR required: this PDF appears to contain scanned images rather than embedded text",
            ));
        }
        if result.has_encoding_issues {
            return Err(ImportFailure::permanent(
                "OCR required: this PDF's embedded text uses an encoding that could not be read reliably",
            ));
        }

        let mut warnings = Vec::new();
        if !result.pages_needing_ocr.is_empty() {
            warnings.push(format!(
                "{} of {page_count} PDF pages may require OCR; imported the readable embedded text",
                result.pages_needing_ocr.len()
            ));
        }
        let markdown = result.markdown.ok_or_else(|| {
            ImportFailure::permanent("PDF conversion did not produce readable Markdown")
        })?;

        Ok(ConvertedDocument {
            title: result.title,
            markdown,
            // pdf-inspector focuses on structured text and emits image markers,
            // but does not expose embedded image bytes for local persistence.
            assets: Vec::<ConvertedAsset>::new(),
            warnings,
        })
    }
}
