use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager};

const HELPER_BASE_NAME: &str = "smooth-system-audio";
const HELPER_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SystemAudioPermissionStatus {
    pub granted: bool,
    pub message: String,
    pub displays: usize,
    #[serde(default)]
    pub error: Option<String>,
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
    #[cfg(target_os = "macos")]
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
}
