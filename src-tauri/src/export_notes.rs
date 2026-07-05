use serde::Serialize;
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{Seek, Write},
    path::{Path, PathBuf},
};
use tauri::{AppHandle, Manager};

use crate::{load_note_meta, now_string, read_note_content};

#[derive(Debug, Serialize)]
pub(crate) struct ExportResult {
    path: String,
    count: usize,
}

struct ExportNote {
    title: String,
    content: String,
}

struct PreparedNote {
    markdown_name: String,
    content: String,
    assets: Vec<ExportAsset>,
}

struct ExportAsset {
    name: String,
    data: Vec<u8>,
}

#[tauri::command]
pub(crate) fn export_note_markdown(app: AppHandle, id: String) -> Result<ExportResult, String> {
    let note = load_export_note(&app, &id)?;
    let export_dir = downloads_dir(&app)?;
    fs::create_dir_all(&export_dir).map_err(|error| error.to_string())?;

    let filename = format!("{}.md", sanitize_filename(&note.title));
    let path = unique_path(&export_dir, &filename);
    let asset_dir_name = single_note_asset_dir_name(&path);
    let asset_dir = path
        .parent()
        .unwrap_or(export_dir.as_path())
        .join(&asset_dir_name);
    let content = rewrite_image_links_for_directory(&note.content, &asset_dir_name, &asset_dir)?;
    fs::write(&path, content).map_err(|error| error.to_string())?;

    Ok(ExportResult {
        path: path.to_string_lossy().to_string(),
        count: 1,
    })
}

#[tauri::command]
pub(crate) fn export_notes_markdown_zip(
    app: AppHandle,
    ids: Vec<String>,
) -> Result<ExportResult, String> {
    let mut seen = HashSet::new();
    let notes = ids
        .into_iter()
        .filter(|id| seen.insert(id.clone()))
        .map(|id| load_export_note(&app, &id))
        .collect::<Result<Vec<_>, _>>()?;

    if notes.is_empty() {
        return Err("Select at least one note to export".to_string());
    }

    let export_dir = downloads_dir(&app)?;
    fs::create_dir_all(&export_dir).map_err(|error| error.to_string())?;
    let filename = format!("Smooth notes {}.zip", timestamp_filename());
    let path = unique_path(&export_dir, &filename);
    write_markdown_zip(&path, &notes)?;

    Ok(ExportResult {
        path: path.to_string_lossy().to_string(),
        count: notes.len(),
    })
}

fn load_export_note(app: &AppHandle, id: &str) -> Result<ExportNote, String> {
    let connection = crate::open_database(app)?;
    let meta = load_note_meta(&connection, id)?;
    if meta.deleted_at.is_some() {
        return Err("Trashed notes cannot be exported".to_string());
    }

    Ok(ExportNote {
        title: meta.title,
        content: read_note_content(app, id)?,
    })
}

fn downloads_dir(app: &AppHandle) -> Result<PathBuf, String> {
    match app.path().download_dir() {
        Ok(path) => Ok(path),
        Err(_) => std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join("Downloads"))
            .ok_or_else(|| "Could not find the Downloads folder".to_string()),
    }
}

fn sanitize_filename(title: &str) -> String {
    let clean = title
        .trim()
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            character if character.is_control() => ' ',
            character => character,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let truncated = clean.chars().take(90).collect::<String>();
    if truncated.is_empty() {
        "Untitled".to_string()
    } else {
        truncated
    }
}

fn timestamp_filename() -> String {
    let value = now_string();
    if value.len() >= 12 {
        value
    } else {
        "export".to_string()
    }
}

fn unique_path(dir: &Path, filename: &str) -> PathBuf {
    let candidate = dir.join(filename);
    if !candidate.exists() {
        return candidate;
    }

    let path = Path::new(filename);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("Export");
    let extension = path.extension().and_then(|value| value.to_str());
    for index in 2.. {
        let next_name = match extension {
            Some(extension) => format!("{stem} ({index}).{extension}"),
            None => format!("{stem} ({index})"),
        };
        let next = dir.join(next_name);
        if !next.exists() {
            return next;
        }
    }

    unreachable!("unique filename search should always return")
}

fn write_markdown_zip(path: &Path, notes: &[ExportNote]) -> Result<(), String> {
    let mut file = File::create(path).map_err(|error| error.to_string())?;
    let mut used_names = HashSet::new();
    let mut entries = Vec::new();

    for note in notes {
        let base_name = sanitize_filename(&note.title);
        let markdown_name = unique_entry_name(&base_name, &mut used_names);
        let prepared = prepare_note_for_zip(note, markdown_name)?;
        write_zip_entry(
            &mut file,
            &mut entries,
            &prepared.markdown_name,
            prepared.content.as_bytes(),
        )?;
        for asset in prepared.assets {
            write_zip_entry(&mut file, &mut entries, &asset.name, &asset.data)?;
        }
    }

    let central_directory_offset =
        file.stream_position().map_err(|error| error.to_string())? as u32;
    for entry in &entries {
        write_central_directory_header(&mut file, entry)?;
    }
    let central_directory_size = file.stream_position().map_err(|error| error.to_string())? as u32
        - central_directory_offset;
    write_end_of_central_directory(
        &mut file,
        entries.len() as u16,
        central_directory_size,
        central_directory_offset,
    )?;
    Ok(())
}

fn write_zip_entry(
    file: &mut File,
    entries: &mut Vec<ZipEntry>,
    name: &str,
    data: &[u8],
) -> Result<(), String> {
    let offset = file.stream_position().map_err(|error| error.to_string())? as u32;
    let crc = crc32(data);

    write_local_file_header(file, name, data.len() as u32, crc)?;
    file.write_all(data).map_err(|error| error.to_string())?;

    entries.push(ZipEntry {
        name: name.to_string(),
        size: data.len() as u32,
        crc,
        offset,
    });
    Ok(())
}

struct ZipEntry {
    name: String,
    size: u32,
    crc: u32,
    offset: u32,
}

fn unique_entry_name(base_name: &str, used_names: &mut HashSet<String>) -> String {
    for index in 1.. {
        let name = if index == 1 {
            format!("{base_name}.md")
        } else {
            format!("{base_name} ({index}).md")
        };
        if used_names.insert(name.clone()) {
            return name;
        }
    }

    unreachable!("unique ZIP entry search should always return")
}

fn prepare_note_for_zip(note: &ExportNote, markdown_name: String) -> Result<PreparedNote, String> {
    let asset_prefix = Path::new(&markdown_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("assets");
    let asset_dir_name = format!("{asset_prefix} assets");
    let mut used_assets = HashSet::new();
    let mut assets = Vec::new();
    let content = rewrite_markdown_image_links(&note.content, |url| {
        let source_path = local_image_path_from_url(url)?;
        let data = fs::read(&source_path).ok()?;
        let filename = source_path
            .file_name()
            .and_then(|value| value.to_str())
            .map(sanitize_filename)
            .unwrap_or_else(|| "image".to_string());
        let asset_filename = unique_asset_name(&filename, &mut used_assets);
        let asset_name = format!("{asset_dir_name}/{asset_filename}");
        let markdown_url = markdown_url_escape(&asset_name);
        assets.push(ExportAsset {
            name: asset_name,
            data,
        });
        Some(markdown_url)
    });

    Ok(PreparedNote {
        markdown_name,
        content,
        assets,
    })
}

fn rewrite_image_links_for_directory(
    content: &str,
    asset_dir_name: &str,
    asset_dir: &Path,
) -> Result<String, String> {
    let mut used_assets = HashSet::new();
    let mut created_asset_dir = false;
    let rewritten = rewrite_markdown_image_links(content, |url| {
        let source_path = local_image_path_from_url(url)?;
        let filename = source_path
            .file_name()
            .and_then(|value| value.to_str())
            .map(sanitize_filename)
            .unwrap_or_else(|| "image".to_string());
        let asset_filename = unique_asset_name(&filename, &mut used_assets);
        if !created_asset_dir {
            if fs::create_dir_all(asset_dir).is_err() {
                return None;
            }
            created_asset_dir = true;
        }
        let output_path = asset_dir.join(&asset_filename);
        if fs::copy(&source_path, output_path).is_err() {
            return None;
        }
        Some(markdown_url_escape(&format!(
            "{asset_dir_name}/{asset_filename}"
        )))
    });
    Ok(rewritten)
}

fn rewrite_markdown_image_links<F>(content: &str, mut rewrite_url: F) -> String
where
    F: FnMut(&str) -> Option<String>,
{
    let mut output = String::with_capacity(content.len());
    let mut cursor = 0;

    while let Some(relative_image_start) = content[cursor..].find("![") {
        let image_start = cursor + relative_image_start;
        let Some(relative_label_end) = content[image_start..].find("](") else {
            break;
        };
        let url_start = image_start + relative_label_end + 2;
        let Some(relative_url_end) = content[url_start..].find(')') else {
            break;
        };
        let url_end = url_start + relative_url_end;
        let url = &content[url_start..url_end];

        output.push_str(&content[cursor..url_start]);
        if let Some(rewritten_url) = rewrite_url(strip_markdown_url_wrapper(url)) {
            output.push_str(&rewritten_url);
        } else {
            output.push_str(url);
        }
        cursor = url_end;
    }

    output.push_str(&content[cursor..]);
    output
}

fn strip_markdown_url_wrapper(url: &str) -> &str {
    let trimmed = url.trim();
    trimmed
        .strip_prefix('<')
        .and_then(|value| value.strip_suffix('>'))
        .unwrap_or(trimmed)
}

fn single_note_asset_dir_name(markdown_path: &Path) -> String {
    let stem = markdown_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("Assets");
    format!("{stem} assets")
}

fn local_image_path_from_url(url: &str) -> Option<PathBuf> {
    let without_fragment = url.split(['#', '?']).next().unwrap_or(url);
    if let Some(path) = without_fragment.strip_prefix("asset://localhost") {
        return Some(normalize_decoded_path(&percent_decode(path)?));
    }
    if let Some(path) = without_fragment.strip_prefix("file://") {
        return Some(normalize_decoded_path(&percent_decode(path)?));
    }
    if without_fragment.starts_with('/') {
        return Some(normalize_decoded_path(&percent_decode(without_fragment)?));
    }
    None
}

fn normalize_decoded_path(path: &str) -> PathBuf {
    if path.starts_with("//") {
        PathBuf::from(&path[1..])
    } else {
        PathBuf::from(path)
    }
}

fn unique_asset_name(filename: &str, used_assets: &mut HashSet<String>) -> String {
    let path = Path::new(filename);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("image");
    let extension = path.extension().and_then(|value| value.to_str());

    for index in 1.. {
        let name = match (index, extension) {
            (1, Some(extension)) => format!("{stem}.{extension}"),
            (_, Some(extension)) => format!("{stem} ({index}).{extension}"),
            (1, None) => stem.to_string(),
            _ => format!("{stem} ({index})"),
        };
        if used_assets.insert(name.clone()) {
            return name;
        }
    }

    unreachable!("unique asset search should always return")
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return None;
            }
            let high = hex_value(bytes[index + 1])?;
            let low = hex_value(bytes[index + 2])?;
            output.push((high << 4) | low);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output).ok()
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn markdown_url_escape(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                output.push(char::from(byte))
            }
            _ => output.push_str(&format!("%{byte:02X}")),
        }
    }
    output
}

fn write_local_file_header(file: &mut File, name: &str, size: u32, crc: u32) -> Result<(), String> {
    write_u32(file, 0x0403_4b50)?;
    write_u16(file, 20)?;
    write_u16(file, 0)?;
    write_u16(file, 0)?;
    write_u16(file, 0)?;
    write_u16(file, 33)?;
    write_u32(file, crc)?;
    write_u32(file, size)?;
    write_u32(file, size)?;
    write_u16(file, name.len() as u16)?;
    write_u16(file, 0)?;
    file.write_all(name.as_bytes())
        .map_err(|error| error.to_string())
}

fn write_central_directory_header(file: &mut File, entry: &ZipEntry) -> Result<(), String> {
    write_u32(file, 0x0201_4b50)?;
    write_u16(file, 20)?;
    write_u16(file, 20)?;
    write_u16(file, 0)?;
    write_u16(file, 0)?;
    write_u16(file, 0)?;
    write_u16(file, 33)?;
    write_u32(file, entry.crc)?;
    write_u32(file, entry.size)?;
    write_u32(file, entry.size)?;
    write_u16(file, entry.name.len() as u16)?;
    write_u16(file, 0)?;
    write_u16(file, 0)?;
    write_u16(file, 0)?;
    write_u16(file, 0)?;
    write_u32(file, 0)?;
    write_u32(file, entry.offset)?;
    file.write_all(entry.name.as_bytes())
        .map_err(|error| error.to_string())
}

fn write_end_of_central_directory(
    file: &mut File,
    entry_count: u16,
    central_directory_size: u32,
    central_directory_offset: u32,
) -> Result<(), String> {
    write_u32(file, 0x0605_4b50)?;
    write_u16(file, 0)?;
    write_u16(file, 0)?;
    write_u16(file, entry_count)?;
    write_u16(file, entry_count)?;
    write_u32(file, central_directory_size)?;
    write_u32(file, central_directory_offset)?;
    write_u16(file, 0)
}

fn write_u16(file: &mut File, value: u16) -> Result<(), String> {
    file.write_all(&value.to_le_bytes())
        .map_err(|error| error.to_string())
}

fn write_u32(file: &mut File, value: u32) -> Result<(), String> {
    file.write_all(&value.to_le_bytes())
        .map_err(|error| error.to_string())
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_standard_crc32() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn writes_basic_zip_archive() {
        let path = std::env::temp_dir().join(format!("smooth-export-test-{}.zip", now_string()));
        let notes = vec![
            ExportNote {
                title: "First / note".to_string(),
                content: "# First".to_string(),
            },
            ExportNote {
                title: "First / note".to_string(),
                content: "Second".to_string(),
            },
        ];

        write_markdown_zip(&path, &notes).expect("write zip");
        let bytes = fs::read(&path).expect("read zip");
        let _ = fs::remove_file(path);

        assert!(bytes.starts_with(&[0x50, 0x4b, 0x03, 0x04]));
        assert!(bytes
            .windows(4)
            .any(|window| window == [0x50, 0x4b, 0x01, 0x02]));
        assert!(bytes
            .windows(4)
            .any(|window| window == [0x50, 0x4b, 0x05, 0x06]));
        let archive_text = String::from_utf8_lossy(&bytes);
        assert!(archive_text.contains("First - note.md"));
        assert!(archive_text.contains("First - note (2).md"));
    }

    #[test]
    fn decodes_tauri_asset_urls_to_local_paths() {
        let path = local_image_path_from_url(
            "asset://localhost/%2FUsers%2Fme%2FLibrary%2FApplication%20Support%2Fsmooth%2Fsnapshot.png",
        )
        .expect("local image path");

        assert_eq!(
            path,
            PathBuf::from("/Users/me/Library/Application Support/smooth/snapshot.png")
        );
    }

    #[test]
    fn rewrites_image_links_and_collects_zip_assets() {
        let temp_dir = std::env::temp_dir().join(format!("smooth-export-assets-{}", now_string()));
        fs::create_dir_all(&temp_dir).expect("create temp dir");
        let image_path = temp_dir.join("snapshot one.png");
        fs::write(&image_path, b"image-data").expect("write image");
        let encoded_path = markdown_url_escape(image_path.to_string_lossy().as_ref());

        let note = ExportNote {
            title: "Meeting".to_string(),
            content: format!("Before\n\n![Shot](asset://localhost{encoded_path})\n\nAfter"),
        };

        let prepared = prepare_note_for_zip(&note, "Meeting.md".to_string()).expect("prepare note");
        let _ = fs::remove_file(image_path);
        let _ = fs::remove_dir(temp_dir);

        assert_eq!(prepared.assets.len(), 1);
        assert_eq!(prepared.assets[0].name, "Meeting assets/snapshot one.png");
        assert_eq!(prepared.assets[0].data, b"image-data");
        assert!(prepared
            .content
            .contains("![Shot](Meeting%20assets/snapshot%20one.png)"));
    }
}
