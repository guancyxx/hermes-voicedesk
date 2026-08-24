/// faster-whisper integration via Python subprocess.
/// Uses faster-whisper for high-quality offline speech recognition.

use std::process::Command;

/// Transcribe an audio file using faster-whisper.
/// Falls back to empty string if faster-whisper is not installed.
pub fn transcribe_file(path: &str, model: &str) -> Result<String, String> {
    let python_script = format!(
        r#"
import sys
try:
    from faster_whisper import WhisperModel
except ImportError:
    print("FASTER_WHISPER_NOT_INSTALLED", file=sys.stderr)
    sys.exit(1)

model = WhisperModel("{}", device="auto", compute_type="auto")
segments, info = model.transcribe("{}", beam_size=5)
text = " ".join(s.text.strip() for s in segments)
print(text)
"#,
        model, path
    );

    let output = Command::new("python3")
        .arg("-c")
        .arg(&python_script)
        .output()
        .map_err(|e| format!("Python error: {}", e))?;

    let stderr = String::from_utf8_lossy(&output.stderr);

    if stderr.contains("FASTER_WHISPER_NOT_INSTALLED") {
        return Err("faster-whisper not installed. Run: pip3 install faster-whisper".to_string());
    }

    if !output.status.success() {
        return Err(format!("Whisper error: {}", stderr.trim()));
    }

    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        return Ok("[No speech detected]".to_string());
    }
    Ok(text)
}
