use crate::{app_data_dir, audio_preprocess::load_wav_for_whisper};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, State};

const SESSION_SAMPLE_RATE: u32 = 16_000;

#[derive(Default)]
pub struct DiarizationState {
    sessions: Mutex<HashMap<String, DiarizationSession>>,
}

struct DiarizationSession {
    path: PathBuf,
    samples: Vec<i16>,
    stable_turns: Vec<DiarizationTurn>,
    next_speaker_index: usize,
}

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
    pub session_id: Option<String>,
    pub chunk_start_ms: i64,
    pub chunk_end_ms: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiarizationSessionStarted {
    pub session_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiarizationCaptureInput {
    pub path: String,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[tauri::command]
pub fn start_diarization_session(
    app: AppHandle,
    state: State<'_, DiarizationState>,
) -> Result<DiarizationSessionStarted, String> {
    let session_id = format!("diarization-{}", now_ms());
    let session_dir = app_data_dir(&app)?.join("audio-captures");
    fs::create_dir_all(&session_dir)
        .map_err(|error| format!("Failed to create audio capture directory: {error}"))?;
    let path = session_dir.join(format!("{session_id}.wav"));

    let mut sessions = state
        .sessions
        .lock()
        .map_err(|_| "Diarization state is unavailable".to_string())?;
    sessions.insert(
        session_id.clone(),
        DiarizationSession {
            path,
            samples: Vec::new(),
            stable_turns: Vec::new(),
            next_speaker_index: 0,
        },
    );

    Ok(DiarizationSessionStarted { session_id })
}

#[tauri::command]
pub fn stop_diarization_session(
    state: State<'_, DiarizationState>,
    session_id: String,
) -> Result<(), String> {
    let mut sessions = state
        .sessions
        .lock()
        .map_err(|_| "Diarization state is unavailable".to_string())?;
    if let Some(session) = sessions.remove(&session_id) {
        let _ = fs::remove_file(session.path);
    }
    Ok(())
}

#[tauri::command]
pub async fn diarize_capture_file(
    app: AppHandle,
    state: State<'_, DiarizationState>,
    input: DiarizationCaptureInput,
) -> Result<DiarizationResult, String> {
    let audio_path = validate_capture_audio_path(&app, input.path)?;
    let (diarization_path, session_id, chunk_start_ms, chunk_end_ms) =
        if let Some(session_id) = input.session_id {
            append_to_session(&state, &session_id, &audio_path)?
        } else {
            let duration_ms = wav_duration_ms(&audio_path)?;
            (audio_path, None, 0, duration_ms)
        };
    let output_path = app_data_dir(&app)?
        .join("audio-captures")
        .join(format!("diarization-{}.rttm", now_ms()));

    let raw_turns = tauri::async_runtime::spawn_blocking(move || {
        diarize_with_polyvoice(diarization_path, output_path)
    })
    .await
    .map_err(|error| format!("Diarization worker failed: {error}"))??;
    let turns = if let Some(session_id) = session_id.as_deref() {
        reconcile_session_turns(&state, session_id, raw_turns)?
    } else {
        raw_turns
    };

    Ok(DiarizationResult {
        turns,
        engine: "polyvoice-cli".to_string(),
        session_id,
        chunk_start_ms,
        chunk_end_ms,
    })
}

fn diarize_with_polyvoice(
    audio_path: PathBuf,
    output_path: PathBuf,
) -> Result<Vec<DiarizationTurn>, String> {
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

    Ok(parse_rttm_turns(&rttm))
}

fn reconcile_session_turns(
    state: &State<'_, DiarizationState>,
    session_id: &str,
    raw_turns: Vec<DiarizationTurn>,
) -> Result<Vec<DiarizationTurn>, String> {
    let mut sessions = state
        .sessions
        .lock()
        .map_err(|_| "Diarization state is unavailable".to_string())?;
    let session = sessions
        .get_mut(session_id)
        .ok_or_else(|| "Diarization session was not found".to_string())?;

    let mut raw_speakers = Vec::<String>::new();
    for turn in &raw_turns {
        if !raw_speakers.contains(&turn.speaker_id) {
            raw_speakers.push(turn.speaker_id.clone());
        }
    }

    let mut mapping = HashMap::<String, String>::new();
    let mut claimed_stable_speakers = Vec::<String>::new();
    for raw_speaker in raw_speakers {
        let mut best_stable_speaker: Option<String> = None;
        let mut best_overlap = 0_i64;
        for previous in &session.stable_turns {
            if claimed_stable_speakers.contains(&previous.speaker_id) {
                continue;
            }
            let overlap = overlap_for_speakers(
                &raw_turns,
                &raw_speaker,
                &session.stable_turns,
                &previous.speaker_id,
            );
            if overlap > best_overlap {
                best_overlap = overlap;
                best_stable_speaker = Some(previous.speaker_id.clone());
            }
        }

        let stable_speaker = if best_overlap > 0 {
            best_stable_speaker.expect("best speaker exists when overlap is positive")
        } else {
            let speaker = format!("speaker_{}", session.next_speaker_index);
            session.next_speaker_index += 1;
            speaker
        };
        claimed_stable_speakers.push(stable_speaker.clone());
        mapping.insert(raw_speaker, stable_speaker);
    }

    let stable_turns = raw_turns
        .into_iter()
        .filter_map(|turn| {
            mapping
                .get(&turn.speaker_id)
                .map(|stable_speaker| DiarizationTurn {
                    speaker_id: stable_speaker.clone(),
                    start_ms: turn.start_ms,
                    end_ms: turn.end_ms,
                })
        })
        .collect::<Vec<_>>();
    session.stable_turns = stable_turns.clone();

    Ok(stable_turns)
}

fn overlap_for_speakers(
    current_turns: &[DiarizationTurn],
    current_speaker: &str,
    previous_turns: &[DiarizationTurn],
    previous_speaker: &str,
) -> i64 {
    current_turns
        .iter()
        .filter(|turn| turn.speaker_id == current_speaker)
        .map(|current| {
            previous_turns
                .iter()
                .filter(|turn| turn.speaker_id == previous_speaker)
                .map(|previous| {
                    (current.end_ms.min(previous.end_ms) - current.start_ms.max(previous.start_ms))
                        .max(0)
                })
                .sum::<i64>()
        })
        .sum()
}

fn append_to_session(
    state: &State<'_, DiarizationState>,
    session_id: &str,
    audio_path: &Path,
) -> Result<(PathBuf, Option<String>, i64, i64), String> {
    let audio = load_wav_for_whisper(audio_path)?;
    let chunk_samples = audio
        .samples
        .iter()
        .map(|sample| (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
        .collect::<Vec<_>>();
    let chunk_duration_ms = duration_ms_for_samples(chunk_samples.len());

    let mut sessions = state
        .sessions
        .lock()
        .map_err(|_| "Diarization state is unavailable".to_string())?;
    let session = sessions
        .get_mut(session_id)
        .ok_or_else(|| "Diarization session was not found".to_string())?;
    let chunk_start_ms = duration_ms_for_samples(session.samples.len());
    session.samples.extend(chunk_samples);
    let chunk_end_ms = chunk_start_ms + chunk_duration_ms;
    write_session_wav(&session.path, &session.samples)?;

    Ok((
        session.path.clone(),
        Some(session_id.to_string()),
        chunk_start_ms,
        chunk_end_ms,
    ))
}

fn write_session_wav(path: &Path, samples: &[i16]) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SESSION_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .map_err(|error| format!("Failed to create diarization session WAV: {error}"))?;
    for sample in samples {
        writer
            .write_sample(*sample)
            .map_err(|error| format!("Failed to write diarization session WAV: {error}"))?;
    }
    writer
        .finalize()
        .map_err(|error| format!("Failed to finalize diarization session WAV: {error}"))
}

fn wav_duration_ms(path: &Path) -> Result<i64, String> {
    let reader =
        hound::WavReader::open(path).map_err(|error| format!("Failed to open WAV: {error}"))?;
    let spec = reader.spec();
    let channels = spec.channels.max(1) as u64;
    let frames = reader.duration() as u64 / channels;
    Ok(((frames * 1000) / spec.sample_rate.max(1) as u64) as i64)
}

fn duration_ms_for_samples(samples: usize) -> i64 {
    ((samples as u128 * 1000) / SESSION_SAMPLE_RATE as u128) as i64
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

    #[test]
    fn computes_speaker_overlap_across_diarization_runs() {
        let current = vec![
            DiarizationTurn {
                speaker_id: "raw_a".to_string(),
                start_ms: 0,
                end_ms: 1000,
            },
            DiarizationTurn {
                speaker_id: "raw_b".to_string(),
                start_ms: 1000,
                end_ms: 2000,
            },
        ];
        let previous = vec![DiarizationTurn {
            speaker_id: "speaker_0".to_string(),
            start_ms: 250,
            end_ms: 1250,
        }];

        assert_eq!(
            overlap_for_speakers(&current, "raw_a", &previous, "speaker_0"),
            750
        );
        assert_eq!(
            overlap_for_speakers(&current, "raw_b", &previous, "speaker_0"),
            250
        );
    }
}
