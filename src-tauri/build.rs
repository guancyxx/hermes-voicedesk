fn main() {
    // Compile the macOS STT fallback helper
    compile_macos_stt_helper();
    tauri_build::build()
}

fn compile_macos_stt_helper() {
    let src = "src/macos_stt_helper.swift";
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());

    // Output to project-root/target/<profile>/macos-stt-helper
    let out_path = std::path::Path::new(&manifest_dir)
        .join("..")
        .join("target")
        .join(&profile)
        .join("macos-stt-helper");

    // Skip if source hasn't changed and binary exists
    if out_path.exists() {
        if let Ok(src_meta) = std::fs::metadata(src) {
            if let Ok(out_meta) = std::fs::metadata(&out_path) {
                if let (Ok(src_time), Ok(out_time)) = (src_meta.modified(), out_meta.modified()) {
                    if out_time >= src_time {
                        println!(
                            "cargo:warning=macos-stt-helper up to date: {}",
                            out_path.display()
                        );
                        return;
                    }
                }
            }
        }
    }

    // Ensure parent directory exists
    if let Some(parent) = out_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let status = std::process::Command::new("swiftc")
        .args(["-O", "-o"])
        .arg(&out_path)
        .arg(src)
        .status();

    match status {
        Ok(s) if s.success() => {
            println!(
                "cargo:warning=compiled macos-stt-helper -> {}",
                out_path.display()
            );
        }
        Ok(s) => {
            println!(
                "cargo:warning=macos-stt-helper compilation warning (non-fatal): exit={}",
                s
            );
        }
        Err(e) => {
            println!(
                "cargo:warning=swiftc not found, skipping macos-stt-helper: {}",
                e
            );
        }
    }
}
