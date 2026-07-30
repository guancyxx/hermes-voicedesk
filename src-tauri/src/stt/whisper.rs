/// faster-whisper integration via Python subprocess.
/// Uses the same faster-whisper installation that Hermes uses.

use std::process::Command;

/// Transcribe audio file using faster-whisper.
pub fn transcribe_file(path: &str, model: &str) -> Result<String, String> {
    // For MVP, use the faster-whisper CLI or Python script.
    // Hermes already has faster-whisper installed.
    let output = Command::new("python3")
        .args([
            "-c",
            &format!(
                "from faster_whisper import WhisperModel; \
                 model = WhisperModel('{}', device='auto', compute_type='auto'); \
                 segments, _ = model.transcribe('{}'); \
                 print(' '.join(s.text for s in segments))",
                model, path
            ),
        ])
        .output()
        .map_err(|e| format!("faster-whisper failed: {}", e))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}
