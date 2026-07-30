use tauri::{AppHandle, Emitter};

/// Start microphone capture with VAD detection.
/// Audio chunks are emitted to the frontend via Tauri events.
pub async fn start_mic_capture(app: AppHandle) -> Result<(), String> {
    // TODO: Implement with cpal + silero-vad
    let _ = app.emit("audio:state", serde_json::json!({ "state": "listening" }));
    Ok(())
}

/// Stop microphone capture.
pub async fn stop_mic_capture() -> Result<(), String> {
    Ok(())
}
