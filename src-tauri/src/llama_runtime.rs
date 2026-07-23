use crate::app_data_dir;
use serde::Serialize;
use std::{
    fs::{self, File},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Mutex,
};
use tauri::{AppHandle, Manager};

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

#[derive(Default)]
struct RuntimeInner {
    child: Option<Child>,
    endpoint: Option<String>,
    cache_dir: Option<PathBuf>,
    log_path: Option<PathBuf>,
    last_error: Option<String>,
}

#[derive(Default)]
pub(crate) struct LlamaRuntimeState {
    inner: Mutex<RuntimeInner>,
}

impl LlamaRuntimeState {
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
            .arg("smooth-local")
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

        let child = command.spawn().map_err(|error| {
            format!(
                "Could not start managed llama.cpp at {}: {error}",
                executable.display()
            )
        })?;
        let endpoint = format!("http://127.0.0.1:{port}");
        inner.child = Some(child);
        inner.endpoint = Some(endpoint.clone());
        inner.cache_dir = Some(cache_dir);
        inner.log_path = Some(log_path);
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
        return Ok(());
    };
    if child
        .try_wait()
        .map_err(|error| format!("Could not inspect managed llama.cpp: {error}"))?
        .is_some()
    {
        inner.endpoint = None;
        return Ok(());
    }
    let kill_result = child.kill();
    let wait_result = child.wait();
    inner.endpoint = None;
    kill_result
        .and(wait_result.map(|_| ()))
        .map_err(|error| format!("Could not stop managed llama.cpp: {error}"))
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
