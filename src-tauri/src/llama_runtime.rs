use crate::app_data_dir;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Mutex,
    thread,
    time::Duration,
};
use tauri::{AppHandle, Manager};

const OWNERSHIP_FILE_NAME: &str = "managed-server.json";
const MANAGED_ALIAS: &str = "smooth-local";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ManagedLlamaLaunchConfig {
    pub(crate) model: String,
    pub(crate) context_size: u32,
    pub(crate) gpu_layers: i32,
    pub(crate) flash_attention: bool,
    pub(crate) parallel: u16,
    pub(crate) cache_ram_mb: u32,
    pub(crate) context_checkpoints: u16,
    pub(crate) cache_type_k: String,
    pub(crate) cache_type_v: String,
    pub(crate) spec_type: String,
    pub(crate) spec_draft_n_max: u16,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ManagedLlamaSnapshot {
    pub(crate) running: bool,
    pub(crate) endpoint: Option<String>,
    pub(crate) cache_dir: Option<String>,
    pub(crate) log_path: Option<String>,
    pub(crate) last_error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ManagedServerOwnership {
    pid: u32,
    owner_pid: u32,
    endpoint: String,
    executable: String,
    port: u16,
}

#[derive(Default)]
struct RuntimeInner {
    child: Option<Child>,
    endpoint: Option<String>,
    cache_dir: Option<PathBuf>,
    log_path: Option<PathBuf>,
    ownership_path: Option<PathBuf>,
    last_error: Option<String>,
}

#[derive(Default)]
pub(crate) struct LlamaRuntimeState {
    inner: Mutex<RuntimeInner>,
}

impl LlamaRuntimeState {
    pub(crate) fn cleanup_stale(&self, app: &AppHandle) -> Result<(), String> {
        let cache_dir = app_data_dir(app)?.join("models").join("llama");
        let executable = llama_server_path(app)?;
        let result = cleanup_stale_owned_server(&cache_dir, &executable);
        if let Err(error) = &result {
            if let Ok(mut inner) = self.inner.lock() {
                inner.last_error = Some(error.clone());
            }
        }
        result
    }

    pub(crate) fn ensure_running(
        &self,
        app: &AppHandle,
        config: &ManagedLlamaLaunchConfig,
    ) -> Result<String, String> {
        let mut inner = self.inner.lock().map_err(|error| error.to_string())?;
        refresh_child(&mut inner);
        if inner.child.is_some() {
            return inner
                .endpoint
                .clone()
                .ok_or_else(|| "Managed llama.cpp has no endpoint".to_string());
        }

        let executable = llama_server_path(app)?;
        let cache_dir = app_data_dir(app)?.join("models").join("llama");
        fs::create_dir_all(&cache_dir).map_err(|error| {
            format!(
                "Could not create llama.cpp model cache at {}: {error}",
                cache_dir.display()
            )
        })?;
        cleanup_stale_owned_server(&cache_dir, &executable)?;
        let log_path = cache_dir.join("llama-server.log");
        let log = File::create(&log_path)
            .map_err(|error| format!("Could not create {}: {error}", log_path.display()))?;
        let log_error = log.try_clone().map_err(|error| error.to_string())?;

        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|error| format!("Could not reserve a llama.cpp port: {error}"))?;
        let port = listener
            .local_addr()
            .map_err(|error| error.to_string())?
            .port();
        drop(listener);

        let mut command = Command::new(&executable);
        let hugging_face_home = cache_dir.join("huggingface");
        let hugging_face_hub = hugging_face_home.join("hub");
        command
            .arg("-hf")
            .arg(&config.model)
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .arg("--ctx-size")
            .arg(config.context_size.to_string())
            .arg("--n-gpu-layers")
            .arg(config.gpu_layers.to_string())
            .arg("--flash-attn")
            .arg(if config.flash_attention { "on" } else { "off" })
            .arg("--parallel")
            .arg(config.parallel.to_string())
            .arg("--cache-ram")
            .arg(config.cache_ram_mb.to_string())
            .arg("--ctx-checkpoints")
            .arg(config.context_checkpoints.to_string())
            .arg("--cache-type-k")
            .arg(&config.cache_type_k)
            .arg("--cache-type-v")
            .arg(&config.cache_type_v)
            .arg("--spec-type")
            .arg(&config.spec_type)
            .arg("--spec-draft-n-max")
            .arg(config.spec_draft_n_max.to_string())
            .arg("--alias")
            .arg(MANAGED_ALIAS)
            .arg("--no-webui")
            .env("LLAMA_CACHE", &cache_dir)
            .env("HF_HOME", &hugging_face_home)
            .env("HF_HUB_CACHE", &hugging_face_hub)
            .env("HUGGINGFACE_HUB_CACHE", &hugging_face_hub)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_error));

        if let Some(library_dir) = llama_library_dir(app, &executable) {
            command.env("DYLD_LIBRARY_PATH", library_dir);
        }

        let mut child = command.spawn().map_err(|error| {
            format!(
                "Could not start managed llama.cpp at {}: {error}",
                executable.display()
            )
        })?;
        let endpoint = format!("http://127.0.0.1:{port}");
        let ownership_path = cache_dir.join(OWNERSHIP_FILE_NAME);
        let ownership = ManagedServerOwnership {
            pid: child.id(),
            owner_pid: std::process::id(),
            endpoint: endpoint.clone(),
            executable: executable.to_string_lossy().into_owned(),
            port,
        };
        if let Err(error) = write_ownership(&ownership_path, &ownership) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        inner.child = Some(child);
        inner.endpoint = Some(endpoint.clone());
        inner.cache_dir = Some(cache_dir);
        inner.log_path = Some(log_path);
        inner.ownership_path = Some(ownership_path);
        inner.last_error = None;
        Ok(endpoint)
    }

    pub(crate) fn stop(&self) -> Result<(), String> {
        let mut inner = self.inner.lock().map_err(|error| error.to_string())?;
        stop_inner(&mut inner)
    }

    pub(crate) fn snapshot(&self, app: &AppHandle) -> ManagedLlamaSnapshot {
        let mut inner = match self.inner.lock() {
            Ok(inner) => inner,
            Err(error) => {
                return ManagedLlamaSnapshot {
                    running: false,
                    endpoint: None,
                    cache_dir: None,
                    log_path: None,
                    last_error: Some(error.to_string()),
                }
            }
        };
        refresh_child(&mut inner);
        let default_cache = app_data_dir(app)
            .ok()
            .map(|path| path.join("models").join("llama"));
        ManagedLlamaSnapshot {
            running: inner.child.is_some(),
            endpoint: inner.endpoint.clone(),
            cache_dir: inner
                .cache_dir
                .as_ref()
                .or(default_cache.as_ref())
                .map(|path| path.to_string_lossy().into_owned()),
            log_path: inner
                .log_path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            last_error: inner.last_error.clone(),
        }
    }

    pub(crate) fn shutdown(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            let _ = stop_inner(&mut inner);
        }
    }
}

impl Drop for LlamaRuntimeState {
    fn drop(&mut self) {
        if let Ok(inner) = self.inner.get_mut() {
            let _ = stop_inner(inner);
        }
    }
}

fn refresh_child(inner: &mut RuntimeInner) {
    let Some(child) = inner.child.as_mut() else {
        return;
    };
    match child.try_wait() {
        Ok(Some(status)) => {
            inner.last_error = Some(format!("llama.cpp exited with {status}"));
            inner.child = None;
            inner.endpoint = None;
            remove_ownership(inner);
        }
        Ok(None) => {}
        Err(error) => {
            inner.last_error = Some(format!("Could not inspect llama.cpp: {error}"));
        }
    }
}

fn stop_inner(inner: &mut RuntimeInner) -> Result<(), String> {
    let Some(mut child) = inner.child.take() else {
        inner.endpoint = None;
        remove_ownership(inner);
        return Ok(());
    };
    if child
        .try_wait()
        .map_err(|error| format!("Could not inspect managed llama.cpp: {error}"))?
        .is_some()
    {
        inner.endpoint = None;
        remove_ownership(inner);
        return Ok(());
    }
    let kill_result = child.kill();
    let wait_result = child.wait();
    inner.endpoint = None;
    let result = kill_result
        .and(wait_result.map(|_| ()))
        .map_err(|error| format!("Could not stop managed llama.cpp: {error}"));
    if result.is_ok() {
        remove_ownership(inner);
    }
    result
}

fn write_ownership(path: &Path, ownership: &ManagedServerOwnership) -> Result<(), String> {
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let contents = serde_json::to_vec(ownership).map_err(|error| error.to_string())?;
    fs::write(&temporary, contents)
        .map_err(|error| format!("Could not write {}: {error}", temporary.display()))?;
    fs::rename(&temporary, path)
        .map_err(|error| format!("Could not save {}: {error}", path.display()))
}

fn remove_ownership(inner: &mut RuntimeInner) {
    if let Some(path) = inner.ownership_path.take() {
        let _ = fs::remove_file(path);
    }
}

fn cleanup_stale_owned_server(cache_dir: &Path, executable: &Path) -> Result<(), String> {
    let path = cache_dir.join(OWNERSHIP_FILE_NAME);
    let contents = match fs::read(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("Could not read {}: {error}", path.display())),
    };
    let ownership = match serde_json::from_slice::<ManagedServerOwnership>(&contents) {
        Ok(ownership) => ownership,
        Err(_) => {
            fs::remove_file(&path)
                .map_err(|error| format!("Could not remove {}: {error}", path.display()))?;
            return Ok(());
        }
    };

    if owned_process_is_running(&ownership, executable)? {
        if ownership.owner_pid != std::process::id()
            && process_id_is_running(ownership.owner_pid)?
        {
            return Err(format!(
                "Managed llama.cpp is owned by another running Smooth process ({})",
                ownership.owner_pid
            ));
        }
        terminate_owned_process(&ownership, executable)?;
    }
    fs::remove_file(&path)
        .or_else(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Ok(())
            } else {
                Err(error)
            }
        })
        .map_err(|error| format!("Could not remove {}: {error}", path.display()))
}

#[cfg(unix)]
fn process_id_is_running(pid: u32) -> Result<bool, String> {
    Command::new("/bin/ps")
        .args(["-p", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .map_err(|error| format!("Could not inspect Smooth process {pid}: {error}"))
}

#[cfg(not(unix))]
fn process_id_is_running(_pid: u32) -> Result<bool, String> {
    Ok(false)
}

#[cfg(unix)]
fn owned_process_is_running(
    ownership: &ManagedServerOwnership,
    executable: &Path,
) -> Result<bool, String> {
    let output = Command::new("/bin/ps")
        .args(["-ww", "-p", &ownership.pid.to_string(), "-o", "command="])
        .output()
        .map_err(|error| format!("Could not inspect managed llama.cpp: {error}"))?;
    if !output.status.success() {
        return Ok(false);
    }
    let command = String::from_utf8_lossy(&output.stdout);
    Ok(command_matches_ownership(&command, ownership, executable))
}

#[cfg(not(unix))]
fn owned_process_is_running(
    _ownership: &ManagedServerOwnership,
    _executable: &Path,
) -> Result<bool, String> {
    Ok(false)
}

fn command_matches_ownership(
    command: &str,
    ownership: &ManagedServerOwnership,
    executable: &Path,
) -> bool {
    let recorded_executable = Path::new(&ownership.executable);
    recorded_executable.file_name() == executable.file_name()
        && command.contains(&ownership.executable)
        && command.contains(&format!("--port {}", ownership.port))
        && command.contains(&format!("--alias {MANAGED_ALIAS}"))
}

#[cfg(unix)]
fn terminate_owned_process(
    ownership: &ManagedServerOwnership,
    executable: &Path,
) -> Result<(), String> {
    let signal = |name: &str| {
        Command::new("/bin/kill")
            .args([name, &ownership.pid.to_string()])
            .status()
            .map_err(|error| format!("Could not signal managed llama.cpp: {error}"))
    };
    let status = signal("-TERM")?;
    if !status.success() {
        if !owned_process_is_running(ownership, executable)? {
            return Ok(());
        }
        return Err(format!(
            "Could not terminate stale managed llama.cpp process {}",
            ownership.pid
        ));
    }
    for _ in 0..20 {
        thread::sleep(Duration::from_millis(50));
        if !owned_process_is_running(ownership, executable)? {
            return Ok(());
        }
    }
    if !owned_process_is_running(ownership, executable)? {
        return Ok(());
    }
    let status = signal("-KILL")?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "Could not kill stale managed llama.cpp process {}",
            ownership.pid
        ))
    }
}

#[cfg(not(unix))]
fn terminate_owned_process(
    _ownership: &ManagedServerOwnership,
    _executable: &Path,
) -> Result<(), String> {
    Ok(())
}

fn llama_server_path(app: &AppHandle) -> Result<PathBuf, String> {
    let name = format!("llama-server-{}", target_triple());
    let packaged_name = if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    };
    let mut candidates = Vec::new();
    if let Ok(resource_dir) = app.path().resource_dir() {
        candidates.push(resource_dir.join(packaged_name));
        candidates.push(resource_dir.join(&name));
        candidates.push(resource_dir.join("binaries").join(packaged_name));
        candidates.push(resource_dir.join("binaries").join(&name));
    }
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(parent) = current_exe.parent() {
            candidates.push(parent.join(packaged_name));
            candidates.push(parent.join(&name));
        }
    }
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(&name),
    );
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            "The managed llama-server sidecar is not installed. Set LLAMA_SERVER_PATH when building Smooth, or use External server mode.".to_string()
        })
}

fn llama_library_dir(app: &AppHandle, executable: &Path) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(resource_dir) = app.path().resource_dir() {
        candidates.push(resource_dir.join("binaries").join("llama-libs"));
        candidates.push(resource_dir.join("llama-libs"));
    }
    if let Some(parent) = executable.parent() {
        candidates.push(parent.join("llama-libs"));
        candidates.push(parent.join("../lib"));
    }
    candidates.into_iter().find(|path| path.is_dir())
}

#[cfg(all(target_arch = "aarch64", target_os = "macos"))]
fn target_triple() -> &'static str {
    "aarch64-apple-darwin"
}

#[cfg(all(target_arch = "x86_64", target_os = "macos"))]
fn target_triple() -> &'static str {
    "x86_64-apple-darwin"
}

#[cfg(all(target_arch = "x86_64", target_os = "windows"))]
fn target_triple() -> &'static str {
    "x86_64-pc-windows-msvc.exe"
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
fn target_triple() -> &'static str {
    "x86_64-unknown-linux-gnu"
}

#[cfg(not(any(
    all(target_arch = "aarch64", target_os = "macos"),
    all(target_arch = "x86_64", target_os = "macos"),
    all(target_arch = "x86_64", target_os = "windows"),
    all(target_arch = "x86_64", target_os = "linux")
)))]
fn target_triple() -> &'static str {
    "unsupported-target"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_match_requires_executable_port_and_managed_alias() {
        let ownership = ManagedServerOwnership {
            pid: 42,
            owner_pid: 41,
            endpoint: "http://127.0.0.1:53157".to_string(),
            executable: "/old/Smooth.app/Contents/Resources/llama-server".to_string(),
            port: 53157,
        };
        let current_executable = Path::new("/new/Smooth.app/Contents/Resources/llama-server");
        let command = "/old/Smooth.app/Contents/Resources/llama-server \
                       --port 53157 --alias smooth-local --no-webui";

        assert!(command_matches_ownership(
            command,
            &ownership,
            current_executable
        ));
        assert!(!command_matches_ownership(
            &command.replace("--port 53157", "--port 55576"),
            &ownership,
            current_executable
        ));
        assert!(!command_matches_ownership(
            &command.replace("--alias smooth-local", "--alias someone-else"),
            &ownership,
            current_executable
        ));
        assert!(!command_matches_ownership(
            command,
            &ownership,
            Path::new("/new/Smooth.app/Contents/Resources/another-server")
        ));
    }
}
