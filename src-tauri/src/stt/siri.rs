/// macOS Siri speech recognition (SFSpeechRecognizer) via compiled Swift helper.
/// This is the PRIMARY STT engine — fast, on-device, no API key needed.
/// Works offline for Chinese and English on macOS.
///
/// The Swift helper binary is compiled by build.rs from macos_stt_helper.swift,
/// which uses SFSpeechRecognizer with on-device recognition.
/// It handles authorization, file reading, and returns transcription text.

/// Transcribe a WAV audio file using macOS SFSpeechRecognizer (Siri).
/// Returns Some(text) on success, None if transcription failed (silence, error, timeout, or helper not found).
pub fn transcribe_file(path: &str) -> Option<String> {
    let helper_name = "macos-stt-helper";
    let helper_path = find_helper_binary(helper_name)?;

    match std::process::Command::new(&helper_path).arg(path).output() {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if !stderr.is_empty() {
                log::warn!("STT (Siri) stderr: {}", stderr);
            }
            if stdout.is_empty() || stdout == "[silence]" || stdout.starts_with("[macos_stt_error")
            {
                log::info!("STT (Siri): no transcription — {}", stdout);
                None
            } else {
                log::info!("STT (Siri): {}", stdout);
                Some(stdout)
            }
        }
        Err(e) => {
            log::error!("STT (Siri) helper failed: {}", e);
            None
        }
    }
}

/// Find the compiled macos-stt-helper binary.
/// Priority: 1) next to current executable (bundled in .app/Contents/MacOS/),
/// 2) in ../Resources/ (Tauri resource bundle), 3) in target/{release,debug}
/// dir (development), 4) in PATH.
fn find_helper_binary(name: &str) -> Option<std::path::PathBuf> {
    // 1. Next to current executable (production — bundled in .app/Contents/MacOS/)
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.parent().unwrap_or(std::path::Path::new(".")).join(name);
        if sibling.exists() {
            log::debug!("Found helper at: {}", sibling.display());
            return Some(sibling);
        }

        // 1b. In ../Resources/ (Tauri v2 resource bundling)
        let resources_dir = exe
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("../Resources");
        if resources_dir.exists() {
            // Check for direct placement
            let direct = resources_dir.join(name);
            if direct.exists() {
                log::debug!("Found helper at: {}", direct.display());
                return Some(direct);
            }
            // Check for path-preserved placement (e.g., _up_/target/release/macos-stt-helper)
            if let Ok(entries) = std::fs::read_dir(&resources_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        // Walk up to 3 levels deep looking for the binary
                        if let Some(found) = find_in_dir(&path, name, 3) {
                            log::debug!("Found helper at: {}", found.display());
                            return Some(found);
                        }
                    }
                }
            }
        }
    }

    // 3. In target directory (development — cargo build output)
    // Walk up from current dir to find target/
    let mut current = std::env::current_dir().ok()?;
    loop {
        let candidate = current.join("target").join("release").join(name);
        if candidate.exists() {
            log::debug!("Found helper at: {}", candidate.display());
            return Some(candidate);
        }
        let candidate_debug = current.join("target").join("debug").join(name);
        if candidate_debug.exists() {
            log::debug!("Found helper at: {}", candidate_debug.display());
            return Some(candidate_debug);
        }
        if !current.pop() {
            break;
        }
    }

    // 4. Check PATH
    if let Ok(paths) = std::env::var("PATH") {
        for dir in paths.split(':') {
            let candidate = std::path::Path::new(dir).join(name);
            if candidate.exists() {
                log::debug!("Found helper at: {}", candidate.display());
                return Some(candidate);
            }
        }
    }

    log::warn!("Siri helper binary '{}' not found", name);
    None
}

/// Recursively search a directory for a file, up to max_depth levels.
fn find_in_dir(dir: &std::path::Path, name: &str, max_depth: u32) -> Option<std::path::PathBuf> {
    if max_depth == 0 {
        return None;
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.file_name().map(|n| n == name).unwrap_or(false) && path.is_file() {
                return Some(path);
            }
            if path.is_dir() {
                if let Some(found) = find_in_dir(&path, name, max_depth - 1) {
                    return Some(found);
                }
            }
        }
    }
    None
}
