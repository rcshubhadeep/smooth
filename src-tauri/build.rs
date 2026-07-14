use std::{
    collections::{HashSet, VecDeque},
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    build_system_audio_helper();
    build_diarization_helper();
    stage_llama_server();
    tauri_build::build()
}

fn stage_llama_server() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.is_empty() {
        return;
    }
    println!("cargo:rerun-if-env-changed=LLAMA_SERVER_PATH");

    let Some(source) = llama_server_source() else {
        println!(
            "cargo:warning=Managed llama.cpp sidecar was not staged. Install llama-server or set LLAMA_SERVER_PATH before packaging Smooth."
        );
        return;
    };
    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is not set"));
    let binaries_dir = manifest_dir.join("binaries");
    let output = binaries_dir.join(format!(
        "llama-server-{target}{}",
        if target.contains("windows") {
            ".exe"
        } else {
            ""
        }
    ));
    std::fs::create_dir_all(&binaries_dir).expect("Failed to create binaries directory");
    println!("cargo:rerun-if-changed={}", source.display());

    if !helper_binary_is_fresh(&source, &output) {
        std::fs::copy(&source, &output).unwrap_or_else(|error| {
            panic!(
                "Failed to copy llama-server from {} to {}: {error}",
                source.display(),
                output.display()
            )
        });
        make_executable(&output);
    }

    if target.contains("apple-darwin") {
        stage_macos_llama_libraries(&source, &output, &binaries_dir.join("llama-libs"));
    }
}

fn llama_server_source() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("LLAMA_SERVER_PATH").map(PathBuf::from) {
        if path.is_file() {
            return std::fs::canonicalize(path).ok();
        }
    }
    let executable = if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    };
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|directory| directory.join(executable))
            .find(|candidate| candidate.is_file())
            .and_then(|candidate| std::fs::canonicalize(candidate).ok())
    })
}

fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path)
            .expect("Failed to read sidecar metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).expect("Failed to set sidecar permissions");
    }
}

fn stage_macos_llama_libraries(source: &Path, output: &Path, libraries_dir: &Path) {
    if libraries_dir.exists() {
        std::fs::remove_dir_all(libraries_dir).expect("Failed to reset llama library directory");
    }
    std::fs::create_dir_all(libraries_dir).expect("Failed to create llama library directory");
    let mut pending = VecDeque::from([source.to_path_buf()]);
    let mut inspected = HashSet::new();
    let mut staged = Vec::new();

    while let Some(binary) = pending.pop_front() {
        let canonical = std::fs::canonicalize(&binary).unwrap_or(binary.clone());
        if !inspected.insert(canonical.clone()) {
            continue;
        }
        for dependency in macos_dependencies(&canonical) {
            let Some(resolved) = resolve_macos_dependency(&canonical, &dependency) else {
                continue;
            };
            let Some(name) = Path::new(&dependency).file_name() else {
                continue;
            };
            let destination = libraries_dir.join(name);
            std::fs::copy(&resolved, &destination).unwrap_or_else(|error| {
                panic!(
                    "Failed to stage llama.cpp dependency {}: {error}",
                    resolved.display()
                )
            });
            make_executable(&destination);
            pending.push_back(resolved);
            staged.push(destination);
        }
    }
    staged.sort();
    staged.dedup();

    rewrite_macos_links(output, false);
    for library in staged {
        rewrite_macos_links(&library, true);
    }
}

fn macos_dependencies(binary: &Path) -> Vec<String> {
    let output = Command::new("otool").arg("-L").arg(binary).output();
    let Ok(output) = output else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .skip(1)
        .filter_map(|line| line.trim().split_whitespace().next())
        .filter(|path| !is_system_library(path))
        .map(str::to_string)
        .collect()
}

fn resolve_macos_dependency(binary: &Path, dependency: &str) -> Option<PathBuf> {
    let parent = binary.parent()?;
    let path = if let Some(relative) = dependency.strip_prefix("@loader_path/") {
        parent.join(relative)
    } else if let Some(relative) = dependency.strip_prefix("@rpath/") {
        let name = Path::new(relative).file_name()?;
        [parent.join(relative), parent.join("../lib").join(name)]
            .into_iter()
            .find(|candidate| candidate.is_file())?
    } else {
        PathBuf::from(dependency)
    };
    path.is_file()
        .then(|| std::fs::canonicalize(&path).unwrap_or(path))
}

fn is_system_library(path: &str) -> bool {
    path.starts_with("/System/Library/") || path.starts_with("/usr/lib/")
}

fn rewrite_macos_links(binary: &Path, set_id: bool) {
    let dependencies = macos_dependencies(binary);
    let mut command = Command::new("install_name_tool");
    if set_id {
        if let Some(name) = binary.file_name().and_then(|name| name.to_str()) {
            command.arg("-id").arg(format!("@rpath/{name}"));
        }
    }
    for dependency in dependencies {
        if let Some(name) = Path::new(&dependency)
            .file_name()
            .and_then(|name| name.to_str())
        {
            command
                .arg("-change")
                .arg(&dependency)
                .arg(format!("@rpath/{name}"));
        }
    }
    let status = command.arg(binary).status();
    if !matches!(status, Ok(status) if status.success()) {
        panic!(
            "Failed to make llama.cpp libraries relocatable for {}",
            binary.display()
        );
    }
    let signed = Command::new("codesign")
        .args(["--force", "--sign", "-"])
        .arg(binary)
        .status();
    if !matches!(signed, Ok(status) if status.success()) {
        panic!(
            "Failed to ad-hoc sign staged llama.cpp binary {}",
            binary.display()
        );
    }
}

fn build_system_audio_helper() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("apple-darwin") {
        return;
    }

    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is not set"));
    let source = manifest_dir.join("swift").join("smooth_system_audio.swift");
    let binaries_dir = manifest_dir.join("binaries");
    let output = binaries_dir.join(format!("smooth-system-audio-{target}"));

    println!("cargo:rerun-if-changed={}", source.display());

    std::fs::create_dir_all(&binaries_dir).expect("Failed to create binaries directory");

    if helper_binary_is_fresh(&source, &output) {
        return;
    }

    let status = Command::new("xcrun")
        .arg("swiftc")
        .arg("-parse-as-library")
        .arg(&source)
        .arg("-framework")
        .arg("AppKit")
        .arg("-framework")
        .arg("Foundation")
        .arg("-framework")
        .arg("AVFoundation")
        .arg("-framework")
        .arg("CoreMedia")
        .arg("-framework")
        .arg("ScreenCaptureKit")
        .arg("-o")
        .arg(&output)
        .status()
        .expect("Failed to invoke xcrun swiftc for ScreenCaptureKit helper");

    if !status.success() {
        panic!("Failed to compile ScreenCaptureKit helper");
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&output)
            .expect("Failed to read ScreenCaptureKit helper metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&output, permissions)
            .expect("Failed to set ScreenCaptureKit helper permissions");
    }
}

fn helper_binary_is_fresh(source: &PathBuf, output: &PathBuf) -> bool {
    let Ok(source_modified) = std::fs::metadata(source).and_then(|metadata| metadata.modified())
    else {
        return false;
    };
    let Ok(output_modified) = std::fs::metadata(output).and_then(|metadata| metadata.modified())
    else {
        return false;
    };

    output_modified >= source_modified
}

fn build_diarization_helper() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.is_empty() {
        return;
    }

    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is not set"));
    let sidecar_dir = manifest_dir.join("sidecars").join("smooth-diarize");
    let sidecar_manifest = sidecar_dir.join("Cargo.toml");
    let sidecar_lock = sidecar_dir.join("Cargo.lock");
    let sidecar_source = sidecar_dir.join("src").join("main.rs");
    let binaries_dir = manifest_dir.join("binaries");
    let output = binaries_dir.join(format!("smooth-diarize-{target}"));

    println!("cargo:rerun-if-changed={}", sidecar_manifest.display());
    println!("cargo:rerun-if-changed={}", sidecar_lock.display());
    println!("cargo:rerun-if-changed={}", sidecar_source.display());

    std::fs::create_dir_all(&binaries_dir).expect("Failed to create binaries directory");

    if helper_binary_is_fresh(&sidecar_manifest, &output)
        && helper_binary_is_fresh(&sidecar_lock, &output)
        && helper_binary_is_fresh(&sidecar_source, &output)
    {
        return;
    }

    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
    let mut command = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string()));
    command
        .arg("build")
        .arg("--manifest-path")
        .arg(&sidecar_manifest)
        .arg("--target")
        .arg(&target);
    if profile == "release" {
        command.arg("--release");
    }

    let status = command
        .status()
        .expect("Failed to invoke cargo for diarization helper");
    if !status.success() {
        panic!("Failed to compile diarization helper");
    }

    let executable_name = if target.contains("windows") {
        "smooth-diarize.exe"
    } else {
        "smooth-diarize"
    };
    let built = sidecar_dir
        .join("target")
        .join(&target)
        .join(&profile)
        .join(executable_name);
    std::fs::copy(&built, &output).unwrap_or_else(|error| {
        panic!(
            "Failed to copy diarization helper from {} to {}: {error}",
            built.display(),
            output.display()
        )
    });

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&output)
            .expect("Failed to read diarization helper metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&output, permissions)
            .expect("Failed to set diarization helper permissions");
    }
}
