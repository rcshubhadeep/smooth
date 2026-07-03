use std::{path::PathBuf, process::Command};

fn main() {
    build_system_audio_helper();
    tauri_build::build()
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
        .arg("Foundation")
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
