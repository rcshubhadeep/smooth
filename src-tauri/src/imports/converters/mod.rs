mod anytomd;
mod pdf;

use std::path::Path;

use super::types::{ImportFailure, ImportLimits};

pub(crate) struct ConvertedAsset {
    pub(crate) source_reference: String,
    pub(crate) suggested_name: String,
    pub(crate) bytes: Vec<u8>,
}

pub(crate) struct ConvertedDocument {
    pub(crate) title: Option<String>,
    pub(crate) markdown: String,
    pub(crate) assets: Vec<ConvertedAsset>,
    pub(crate) warnings: Vec<String>,
}

trait DocumentConverter {
    fn supports(&self, format: &str) -> bool;
    fn convert(
        &self,
        path: &Path,
        limits: ImportLimits,
    ) -> Result<ConvertedDocument, ImportFailure>;
}

pub(crate) fn convert(
    path: &Path,
    format: &str,
    limits: ImportLimits,
) -> Result<ConvertedDocument, ImportFailure> {
    let converters: [&dyn DocumentConverter; 2] = [&pdf::PdfConverter, &anytomd::AnyToMdConverter];
    let converter = converters
        .into_iter()
        .find(|converter| converter.supports(format))
        .ok_or_else(|| ImportFailure::permanent(format!("Unsupported file format: {format}")))?;
    converter.convert(path, limits)
}
