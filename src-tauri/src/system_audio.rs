use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    sync::Mutex,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager, State};

const HELPER_BASE_NAME: &str = "smooth-system-audio";
const HELPER_TIMEOUT: Duration = Duration::from_secs(20);
const CAPTURE_START_SETTLE: Duration = Duration::from_millis(650);
const CAPTURE_STOP_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Default)]
pub struct SystemAudioCaptureState {
    worker: Mutex<SystemAudioCaptureWorker>,
}

#[derive(Default)]
struct SystemAudioCaptureWorker {
    active: Option<ActiveSystemAudioCapture>,
    last_preview: Option<SystemAudioCapturePreview>,
    last_error: Option<String>,
}

struct ActiveSystemAudioCapture {
    child: Child,
    output_path: PathBuf,
    started_at: Instant,
    started_at_ms: u128,
}

impl Drop for SystemAudioCaptureState {
    fn drop(&mut self) {
        let Ok(mut worker) = self.worker.lock() else {
            return;
        };
        if let Some(mut active) = worker.active.take() {
            let _ = active.child.kill();
            let _ = active.child.wait();
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SystemAudioPermissionStatus {
    pub granted: bool,
    pub message: String,
    pub displays: usize,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SystemAudioCaptureStatus {
    pub is_recording: bool,
    pub output_path: Option<String>,
    pub elapsed_ms: Option<u128>,
    pub started_at_ms: Option<u128>,
    pub last_preview: Option<SystemAudioCapturePreview>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SystemAudioCapturePreview {
    pub path: String,
    pub duration_ms: u128,
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: u64,
}

#[derive(Debug, Deserialize)]
struct HelperCaptureEvent {
    #[serde(rename = "type")]
    kind: String,
    path: Option<String>,
    duration_ms: Option<u128>,
    sample_rate: Option<u32>,
    channels: Option<u16>,
    samples: Option<u64>,
    message: Option<String>,
    error: Option<String>,
}

#[tauri::command]
pub fn check_system_audio_permission(
    app: AppHandle,
) -> Result<SystemAudioPermissionStatus, String> {
    #[cfg(target_os = "macos")]
    {
        check_system_audio_permission_macos(&app)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Ok(SystemAudioPermissionStatus {
            granted: false,
            message: "System audio capture is only supported on macOS.".to_string(),
            displays: 0,
            error: Some("unsupported_platform".to_string()),
        })
    }
}

#[tauri::command]
pub fn get_system_audio_capture_status(
    state: State<'_, SystemAudioCaptureState>,
) -> Result<SystemAudioCaptureStatus, String> {
    let mut worker = state
        .worker
        .lock()
        .map_err(|_| "system audio capture state is unavailable".to_string())?;
    refresh_exited_capture(&mut worker)?;
    Ok(status_from_worker(&worker))
}

#[tauri::command]
pub fn start_system_audio_capture(
    app: AppHandle,
    state: State<'_, SystemAudioCaptureState>,
) -> Result<SystemAudioCaptureStatus, String> {
    let mut worker = state
        .worker
        .lock()
        .map_err(|_| "system audio capture state is unavailable".to_string())?;
    refresh_exited_capture(&mut worker)?;

    if worker.active.is_some() {
        return Ok(status_from_worker(&worker));
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        return Err("System audio capture is only supported on macOS.".to_string());
    }

    #[cfg(target_os = "macos")]
    {
        let capture_dir = audio_capture_dir(&app)?;
        fs::create_dir_all(&capture_dir)
            .map_err(|error| format!("Failed to create audio capture directory: {error}"))?;

        let output_path = capture_dir.join(format!("system-capture-{}.wav", now_ms()));
        let helper = system_audio_helper_path(&app)?;
        let mut child = Command::new(&helper)
            .arg("capture")
            .arg("--output")
            .arg(&output_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                format!(
                    "Failed to start system audio helper at {}: {error}",
                    helper.display()
                )
            })?;

        thread::sleep(CAPTURE_START_SETTLE);
        if child
            .try_wait()
            .map_err(|error| format!("Failed to check system audio helper status: {error}"))?
            .is_some()
        {
            let output = child
                .wait_with_output()
                .map_err(|error| format!("Failed to read system audio helper output: {error}"))?;
            let message = helper_capture_error(&output);
            worker.last_error = Some(message.clone());
            return Err(message);
        }

        worker.last_error = None;
        worker.active = Some(ActiveSystemAudioCapture {
            child,
            output_path,
            started_at: Instant::now(),
            started_at_ms: now_ms(),
        });

        Ok(status_from_worker(&worker))
    }
}

#[tauri::command]
pub fn stop_system_audio_capture(
    state: State<'_, SystemAudioCaptureState>,
) -> Result<SystemAudioCaptureStatus, String> {
    let mut worker = state
        .worker
        .lock()
        .map_err(|_| "system audio capture state is unavailable".to_string())?;

    let Some(mut active) = worker.active.take() else {
        return Ok(status_from_worker(&worker));
    };

    if let Some(mut stdin) = active.child.stdin.take() {
        let _ = writeln!(stdin, "stop");
    }

    let output = wait_child_with_timeout(active.child, CAPTURE_STOP_TIMEOUT)?;
    finish_capture_from_output(&mut worker, &active.output_path, active.started_at, output)?;

    Ok(status_from_worker(&worker))
}

#[cfg(target_os = "macos")]
fn check_system_audio_permission_macos(
    app: &AppHandle,
) -> Result<SystemAudioPermissionStatus, String> {
    let helper = system_audio_helper_path(app)?;
    let output = run_helper_check(&helper)?;

    if let Some(parsed) = parse_helper_status(&output.stdout) {
        return Ok(parsed);
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if output.status.success() {
        Err(format!(
            "System audio helper did not return a valid response. stdout: {stdout}"
        ))
    } else {
        Err(format!(
            "System audio helper failed. stdout: {stdout}; stderr: {stderr}"
        ))
    }
}

fn refresh_exited_capture(worker: &mut SystemAudioCaptureWorker) -> Result<(), String> {
    let Some(active) = worker.active.as_mut() else {
        return Ok(());
    };

    if active
        .child
        .try_wait()
        .map_err(|error| format!("Failed to check system audio capture status: {error}"))?
        .is_none()
    {
        return Ok(());
    }

    let active = worker.active.take().expect("active capture exists");
    let output = active
        .child
        .wait_with_output()
        .map_err(|error| format!("Failed to read system audio capture output: {error}"))?;
    finish_capture_from_output(worker, &active.output_path, active.started_at, output)
}

fn wait_child_with_timeout(mut child: Child, timeout: Duration) -> Result<Output, String> {
    let started_at = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child.wait_with_output().map_err(|error| {
                    format!("Failed to read system audio helper output: {error}")
                });
            }
            Ok(None) if started_at.elapsed() < timeout => {
                thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                let output = child.wait_with_output().map_err(|error| {
                    format!(
                        "Timed out stopping system audio helper and failed to read output: {error}"
                    )
                })?;
                return Err(format!(
                    "Timed out stopping system audio capture. {}",
                    helper_capture_error(&output)
                ));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

fn finish_capture_from_output(
    worker: &mut SystemAudioCaptureWorker,
    output_path: &PathBuf,
    elapsed: Instant,
    output: Output,
) -> Result<(), String> {
    if let Some(event) = parse_helper_capture_event(&output.stdout) {
        if event.kind == "finished" {
            let preview = preview_from_event(&event, output_path, elapsed)?;
            worker.last_error = event.error.clone();
            worker.last_preview = Some(preview);
            return Ok(());
        }

        let message = event
            .error
            .or(event.message)
            .unwrap_or_else(|| "System audio capture helper failed.".to_string());
        worker.last_error = Some(message.clone());
        return Err(message);
    }

    if output_path.exists() {
        let preview = preview_from_existing_file(output_path, elapsed)?;
        worker.last_preview = Some(preview);
        if !output.status.success() {
            worker.last_error = Some(helper_capture_error(&output));
        }
        return Ok(());
    }

    let message = helper_capture_error(&output);
    worker.last_error = Some(message.clone());
    Err(message)
}

fn preview_from_event(
    event: &HelperCaptureEvent,
    output_path: &PathBuf,
    elapsed: Instant,
) -> Result<SystemAudioCapturePreview, String> {
    let path = event
        .path
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| output_path.clone());
    let samples = event.samples.unwrap_or(0);
    if samples == 0 || !path.exists() {
        return Err(
            "No system audio samples were captured. Play audio during capture and try again."
                .to_string(),
        );
    }

    Ok(SystemAudioCapturePreview {
        path: path.to_string_lossy().into_owned(),
        duration_ms: event
            .duration_ms
            .unwrap_or_else(|| elapsed.elapsed().as_millis()),
        sample_rate: event.sample_rate.unwrap_or(48_000),
        channels: event.channels.unwrap_or(2),
        samples,
    })
}

fn preview_from_existing_file(
    output_path: &PathBuf,
    elapsed: Instant,
) -> Result<SystemAudioCapturePreview, String> {
    let metadata = fs::metadata(output_path)
        .map_err(|error| format!("Failed to inspect system audio capture: {error}"))?;
    if metadata.len() == 0 {
        return Err("System audio capture file is empty.".to_string());
    }

    match hound::WavReader::open(output_path) {
        Ok(reader) => {
            let spec = reader.spec();
            let samples = reader.duration() as u64;
            Ok(SystemAudioCapturePreview {
                path: output_path.to_string_lossy().into_owned(),
                duration_ms: samples_to_duration_ms(
                    samples as u128,
                    spec.sample_rate,
                    spec.channels,
                ),
                sample_rate: spec.sample_rate,
                channels: spec.channels,
                samples,
            })
        }
        Err(_) => Ok(SystemAudioCapturePreview {
            path: output_path.to_string_lossy().into_owned(),
            duration_ms: elapsed.elapsed().as_millis(),
            sample_rate: 48_000,
            channels: 2,
            samples: metadata.len(),
        }),
    }
}

fn parse_helper_capture_event(output: &[u8]) -> Option<HelperCaptureEvent> {
    let stdout = String::from_utf8_lossy(output);
    stdout.lines().rev().find_map(|line| {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return None;
        }
        serde_json::from_str::<HelperCaptureEvent>(trimmed).ok()
    })
}

fn helper_capture_error(output: &Output) -> String {
    if let Some(event) = parse_helper_capture_event(&output.stdout) {
        if let Some(error) = event.error {
            return error;
        }
        if let Some(message) = event.message {
            return message;
        }
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        format!("System audio helper failed: {stderr}")
    } else if !stdout.is_empty() {
        format!("System audio helper failed: {stdout}")
    } else {
        format!(
            "System audio helper exited with status {}.",
            output.status.code().unwrap_or(-1)
        )
    }
}

fn status_from_worker(worker: &SystemAudioCaptureWorker) -> SystemAudioCaptureStatus {
    let Some(active) = &worker.active else {
        return SystemAudioCaptureStatus {
            is_recording: false,
            output_path: None,
            elapsed_ms: None,
            started_at_ms: None,
            last_preview: worker.last_preview.clone(),
            last_error: worker.last_error.clone(),
        };
    };

    SystemAudioCaptureStatus {
        is_recording: true,
        output_path: Some(active.output_path.to_string_lossy().into_owned()),
        elapsed_ms: Some(active.started_at.elapsed().as_millis()),
        started_at_ms: Some(active.started_at_ms),
        last_preview: worker.last_preview.clone(),
        last_error: worker.last_error.clone(),
    }
}

fn audio_capture_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|error| format!("Failed to resolve app data directory: {error}"))?
        .join("audio-captures"))
}

fn samples_to_duration_ms(samples: u128, sample_rate: u32, channels: u16) -> u128 {
    let frames = samples / channels.max(1) as u128;
    (frames * 1000) / sample_rate.max(1) as u128
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(target_os = "macos")]
fn run_helper_check(helper: &PathBuf) -> Result<Output, String> {
    let mut child = Command::new(helper)
        .arg("check")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            format!(
                "Failed to run system audio helper at {}: {error}",
                helper.display()
            )
        })?;

    let started_at = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().map_err(|error| error.to_string()),
            Ok(None) if started_at.elapsed() < HELPER_TIMEOUT => {
                thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait_with_output();
                return Err(
                    "Timed out while checking Screen & System Audio Recording permission. Respond to any macOS permission prompt, then try again.".to_string(),
                );
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

#[cfg(target_os = "macos")]
fn parse_helper_status(output: &[u8]) -> Option<SystemAudioPermissionStatus> {
    let stdout = String::from_utf8_lossy(output);
    stdout.lines().rev().find_map(|line| {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return None;
        }
        serde_json::from_str::<SystemAudioPermissionStatus>(trimmed).ok()
    })
}

#[cfg(target_os = "macos")]
fn system_audio_helper_path(app: &AppHandle) -> Result<PathBuf, String> {
    let name = helper_binary_name();

    if let Ok(resource_dir) = app.path().resource_dir() {
        for candidate in [
            resource_dir.join(&name),
            resource_dir.join("binaries").join(&name),
        ] {
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }

    let dev_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("binaries")
        .join(&name);
    if dev_path.exists() {
        return Ok(dev_path);
    }

    Err(format!(
        "System audio helper binary was not found. Expected {}",
        dev_path.display()
    ))
}

#[cfg(target_os = "macos")]
fn helper_binary_name() -> String {
    format!("{HELPER_BASE_NAME}-{}", target_triple())
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn target_triple() -> &'static str {
    "aarch64-apple-darwin"
}

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
fn target_triple() -> &'static str {
    "x86_64-apple-darwin"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn parses_last_json_line_from_helper_output() {
        let output = br#"noise
{"granted":true,"message":"ok","displays":2}"#;

        let parsed = parse_helper_status(output).expect("expected helper status");

        assert!(parsed.granted);
        assert_eq!(parsed.message, "ok");
        assert_eq!(parsed.displays, 2);
    }

    #[test]
    fn parses_capture_finished_event() {
        let output = br#"{"type":"started","path":"/tmp/a.wav"}
{"type":"finished","path":"/tmp/a.wav","duration_ms":1200,"sample_rate":48000,"channels":2,"samples":115200}"#;

        let event = parse_helper_capture_event(output).expect("expected helper event");

        assert_eq!(event.kind, "finished");
        assert_eq!(event.duration_ms, Some(1200));
        assert_eq!(event.sample_rate, Some(48_000));
        assert_eq!(event.channels, Some(2));
        assert_eq!(event.samples, Some(115_200));
    }
}
