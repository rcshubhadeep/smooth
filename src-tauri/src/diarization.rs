use crate::app_data_dir;
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::AppHandle;

#[derive(Clone, Debug, Serialize)]
pub struct DiarizationTurn {
    pub speaker_id: String,
    pub start_ms: i64,
    pub end_ms: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiarizationResult {
    pub turns: Vec<DiarizationTurn>,
    pub engine: String,
}

#[tauri::command]
pub async fn diarize_capture_file(
    app: AppHandle,
    path: String,
) -> Result<DiarizationResult, String> {
    let audio_path = validate_capture_audio_path(&app, path)?;
    let output_path = app_data_dir(&app)?
        .join("audio-captures")
        .join(format!("diarization-{}.rttm", now_ms()));

    tauri::async_runtime::spawn_blocking(move || diarize_with_polyvoice(audio_path, output_path))
        .await
        .map_err(|error| format!("Diarization worker failed: {error}"))?
}

fn diarize_with_polyvoice(
    audio_path: PathBuf,
    output_path: PathBuf,
) -> Result<DiarizationResult, String> {
    let polyvoice = find_polyvoice_binary()
        .ok_or_else(|| "Polyvoice CLI is not installed or not on PATH".to_string())?;

    let output = Command::new(&polyvoice)
        .arg("diarize")
        .arg(&audio_path)
        .arg("--output")
        .arg(&output_path)
        .output()
        .map_err(|error| format!("Failed to run Polyvoice: {error}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let message = if !stderr.is_empty() { stderr } else { stdout };
        return Err(if message.is_empty() {
            "Polyvoice diarization failed".to_string()
        } else {
            format!("Polyvoice diarization failed: {message}")
        });
    }

    let rttm = fs::read_to_string(&output_path)
        .map_err(|error| format!("Failed to read Polyvoice RTTM output: {error}"))?;
    let _ = fs::remove_file(&output_path);

    Ok(DiarizationResult {
        turns: parse_rttm_turns(&rttm),
        engine: "polyvoice-cli".to_string(),
    })
}

fn validate_capture_audio_path(app: &AppHandle, path: String) -> Result<PathBuf, String> {
    let requested = PathBuf::from(path);
    let capture_dir = app_data_dir(app)?.join("audio-captures");
    let canonical_dir = capture_dir
        .canonicalize()
        .map_err(|error| format!("Failed to resolve audio capture directory: {error}"))?;
    let canonical_path = requested
        .canonicalize()
        .map_err(|error| format!("Failed to resolve audio capture file: {error}"))?;
    if !canonical_path.starts_with(&canonical_dir) {
        return Err("Audio capture file is outside the capture directory".to_string());
    }
    if canonical_path.extension().and_then(|value| value.to_str()) != Some("wav") {
        return Err("Audio capture file must be a WAV file".to_string());
    }
    Ok(canonical_path)
}

fn find_polyvoice_binary() -> Option<PathBuf> {
    let candidates = [
        PathBuf::from("polyvoice"),
        PathBuf::from("/opt/homebrew/bin/polyvoice"),
        PathBuf::from("/usr/local/bin/polyvoice"),
    ];

    candidates.into_iter().find(|candidate| {
        if candidate.components().count() == 1 {
            Command::new(candidate)
                .arg("--help")
                .output()
                .map(|_| true)
                .unwrap_or(false)
        } else {
            candidate.is_file() && is_executable(candidate)
        }
    })
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.exists()
}

fn parse_rttm_turns(input: &str) -> Vec<DiarizationTurn> {
    let mut turns = Vec::new();
    for line in input.lines() {
        let parts = line.split_whitespace().collect::<Vec<_>>();
        if parts.len() < 8 || parts[0] != "SPEAKER" {
            continue;
        }
        let Ok(start_seconds) = parts[3].parse::<f64>() else {
            continue;
        };
        let Ok(duration_seconds) = parts[4].parse::<f64>() else {
            continue;
        };
        let start_ms = (start_seconds * 1000.0).round() as i64;
        let end_ms = ((start_seconds + duration_seconds) * 1000.0).round() as i64;
        if end_ms <= start_ms {
            continue;
        }
        turns.push(DiarizationTurn {
            speaker_id: normalize_speaker_id(parts[7]),
            start_ms,
            end_ms,
        });
    }
    turns.sort_by_key(|turn| (turn.start_ms, turn.end_ms));
    turns
}

fn normalize_speaker_id(value: &str) -> String {
    let clean = value
        .trim()
        .trim_matches('"')
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_");

    if clean.is_empty() {
        "speaker".to_string()
    } else {
        clean
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rttm_speaker_turns() {
        let turns = parse_rttm_turns(
            "SPEAKER meeting 1 0.250 2.500 <NA> <NA> Speaker_0 <NA> <NA>\n\
             SPEAKER meeting 1 3.000 1.250 <NA> <NA> speaker-1 <NA> <NA>\n",
        );

        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].speaker_id, "speaker_0");
        assert_eq!(turns[0].start_ms, 250);
        assert_eq!(turns[0].end_ms, 2750);
        assert_eq!(turns[1].speaker_id, "speaker_1");
    }
}
