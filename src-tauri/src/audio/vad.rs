// VAD (Voice Activity Detection) module.
// Will use silero-vad via ONNX Runtime (ort crate).
// Placeholder for Phase 1 MVP — uses simple RMS energy threshold.

/// Detect if audio frame contains speech based on RMS energy.
pub fn is_speech(samples: &[i16], threshold: f64) -> bool {
    if samples.is_empty() {
        return false;
    }
    let sum: f64 = samples.iter().map(|&s| (s as f64).powi(2)).sum();
    let rms = (sum / samples.len() as f64).sqrt();
    rms > threshold
}
