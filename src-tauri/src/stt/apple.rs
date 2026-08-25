/// macOS built-in speech recognition.
/// Uses NSSpeechRecognizer via a Python helper with PyObjC.
/// Falls back to an error message if PyObjC is not available.
use std::process::Command;

/// Transcribe an audio file using macOS built-in dictation.
pub fn transcribe_file(path: &str) -> Result<String, String> {
    let python_script = format!(
        r#"
import sys
try:
    import AppKit
    import AVFoundation
except ImportError:
    print("PYOBJC_NOT_INSTALLED", file=sys.stderr)
    sys.exit(1)

# Use SFSpeechRecognizer for audio file transcription
from Foundation import NSURL
recognizer = AppKit.NSSpeechRecognizer.alloc().init()
if recognizer is None:
    print("Speech recognition not available", file=sys.stderr)
    sys.exit(1)

url = NSURL.fileURLWithPath_("{}")
result, error = recognizer.recognizeSpeechFromURL_error_(url, None)
if error:
    print(str(error), file=sys.stderr)
    sys.exit(1)
print(result)
"#,
        path
    );

    let output = Command::new("python3")
        .arg("-c")
        .arg(&python_script)
        .output()
        .map_err(|e| format!("Python error: {}", e))?;

    let stderr = String::from_utf8_lossy(&output.stderr);

    if stderr.contains("PYOBJC_NOT_INSTALLED") {
        return Err("PyObjC not installed. Using faster-whisper fallback.".to_string());
    }

    if !output.status.success() {
        return Err(format!("Apple STT error: {}", stderr.trim()));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
