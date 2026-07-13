use std::path::Path;

use anytomd::{convert_file, ConversionOptions};

use super::{ConvertedAsset, ConvertedDocument, DocumentConverter};
use crate::imports::types::{ImportFailure, ImportLimits};

pub(crate) struct AnyToMdConverter;

impl DocumentConverter for AnyToMdConverter {
    fn supports(&self, format: &str) -> bool {
        format != "pdf"
    }

    fn convert(
        &self,
        path: &Path,
        limits: ImportLimits,
    ) -> Result<ConvertedDocument, ImportFailure> {
        if matches!(
            path.extension()
                .and_then(|value| value.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("docx" | "pptx" | "xlsx")
        ) {
            validate_archive(path, limits)?;
        }
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

fn validate_archive(path: &Path, limits: ImportLimits) -> Result<(), ImportFailure> {
    let file = std::fs::File::open(path)
        .map_err(|error| ImportFailure::retryable(format!("Could not open document: {error}")))?;
    let source_size = file
        .metadata()
        .map_err(|error| ImportFailure::retryable(format!("Could not inspect document: {error}")))?
        .len()
        .max(1);
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| ImportFailure::permanent(format!("Invalid Office archive: {error}")))?;
    if archive.len() > limits.max_archive_entries {
        return Err(ImportFailure::permanent(format!(
            "Office document contains {} archive entries; the limit is {}",
            archive.len(),
            limits.max_archive_entries
        )));
    }

    let mut expanded = 0_u64;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| ImportFailure::permanent(format!("Invalid archive entry: {error}")))?;
        if entry.enclosed_name().is_none() {
            return Err(ImportFailure::permanent(
                "Office document contains an unsafe archive path",
            ));
        }
        expanded = expanded.saturating_add(entry.size());
        if expanded > limits.max_expanded_archive_bytes as u64 {
            return Err(ImportFailure::permanent(format!(
                "Expanded Office document exceeds {} MiB",
                limits.max_expanded_archive_bytes / 1024 / 1024
            )));
        }
    }
    if expanded / source_size > limits.max_archive_expansion_ratio {
        return Err(ImportFailure::permanent(format!(
            "Office document expansion ratio exceeds {}x",
            limits.max_archive_expansion_ratio
        )));
    }
    Ok(())
}
