use std::path::Path;

use anytomd::{convert_file, ConversionOptions};

use super::{ConvertedAsset, ConvertedDocument, DocumentConverter};
use crate::imports::types::{ImportFailure, ImportLimits};

pub(crate) struct AnyToMdConverter;

impl DocumentConverter for AnyToMdConverter {
    fn supports(&self, format: &str) -> bool {
        !super::anydoc::supports_format(format)
    }

    fn convert(
        &self,
        path: &Path,
        limits: ImportLimits,
    ) -> Result<ConvertedDocument, ImportFailure> {
        let options = ConversionOptions {
            extract_images: true,
            extract_comments: false,
            max_total_image_bytes: limits.max_total_asset_bytes,
            strict: false,
            max_input_bytes: limits.max_office_bytes as usize,
            max_uncompressed_zip_bytes: limits.max_expanded_archive_bytes,
            image_describer: None,
        };
        let result = convert_file(path, &options)
            .map_err(|error| ImportFailure::permanent(format!("Conversion failed: {error}")))?;
        let warnings = result
            .warnings
            .into_iter()
            .map(|warning| match warning.location {
                Some(location) => format!("{} ({location})", warning.message),
                None => warning.message,
            })
            .collect();
        let assets = result
            .images
            .into_iter()
            .map(|(filename, bytes)| ConvertedAsset {
                source_reference: filename.clone(),
                suggested_name: filename,
                bytes,
            })
            .collect();

        Ok(ConvertedDocument {
            title: result.title,
            markdown: result.markdown,
            assets,
            warnings,
        })
    }
}
