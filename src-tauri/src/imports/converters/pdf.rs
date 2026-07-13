use std::path::Path;

use unpdf::{parse_file, render, render::RenderOptions};

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
        let document = parse_file(path)
            .map_err(|error| ImportFailure::permanent(format!("PDF parsing failed: {error}")))?;
        let page_count = document.page_count();
        if page_count > limits.max_pdf_pages {
            return Err(ImportFailure::permanent(format!(
                "PDF has {page_count} pages; the limit is {}",
                limits.max_pdf_pages
            )));
        }

        let empty_pages = document
            .pages
            .iter()
            .filter(|page| page.plain_text().trim().is_empty())
            .count();
        let extracted_chars = document.extraction_quality.char_count;
        let low_density = page_count > 0 && extracted_chars < page_count as usize * 20;
        let mostly_empty = page_count > 0 && empty_pages * 100 > page_count as usize * 60;
        if document.extraction_quality.is_scan_pdf || low_density || mostly_empty {
            return Err(ImportFailure::permanent(
                "OCR required: this PDF appears to contain scanned images rather than embedded text",
            ));
        }

        let mut warnings = Vec::new();
        if let Some(warning) = document.extraction_quality.warning_message() {
            warnings.push(warning);
        }
        let markdown = render::to_markdown(&document, &RenderOptions::default())
            .map_err(|error| ImportFailure::permanent(format!("PDF rendering failed: {error}")))?;
        let assets = document
            .resources
            .iter()
            .filter(|(_, resource)| resource.is_image())
            .map(|(id, resource)| ConvertedAsset {
                source_reference: id.clone(),
                suggested_name: resource.suggested_filename(id),
                bytes: resource.data.clone(),
            })
            .collect();

        Ok(ConvertedDocument {
            title: document.metadata.title.clone(),
            markdown,
            assets,
            warnings,
        })
    }
}
