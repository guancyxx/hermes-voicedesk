/// Speak text using macOS AVSpeechSynthesizer via `say` command.
/// TODO: Replace with direct AVSpeechSynthesizer FFI via objc crate.
pub fn speak(text: &str) -> Result<(), String> {
    std::process::Command::new("say")
        .arg(text)
        .spawn()
        .map_err(|e| format!("TTS failed: {}", e))?;
    Ok(())
}

/// Stop current speech.
pub fn stop() -> Result<(), String> {
    std::process::Command::new("killall")
        .arg("say")
        .output()
        .map_err(|e| format!("Stop TTS failed: {}", e))?;
    Ok(())
}
