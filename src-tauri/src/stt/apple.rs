/// macOS built-in speech recognition via NSSpeechRecognizer.
/// Uses the macOS `say` command in dictation mode for MVP.
/// TODO: Replace with direct NSSpeechRecognizer FFI via objc crate.

use std::process::Command;

/// Transcribe an audio file using macOS dictation.
pub fn transcribe_file(path: &str) -> Result<String, String> {
    // For MVP, return placeholder.
    // Real implementation: use NSSpeechRecognizer via objc bindings
    // or call faster-whisper via Python subprocess.
    let _ = path;
    Ok(String::new())
}
