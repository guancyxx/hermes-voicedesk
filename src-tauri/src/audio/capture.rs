use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};

static IS_CAPTURING: AtomicBool = AtomicBool::new(false);

/// Simple state machine for speech detection.
struct SpeechDetector {
    is_speaking: bool,
    buffer: Vec<i16>,
    silence_frames: u32,
    sample_rate: u32,
}

impl SpeechDetector {
    fn new(sample_rate: u32) -> Self {
        Self { is_speaking: false, buffer: Vec::new(), silence_frames: 0, sample_rate }
    }

    /// Returns Some(audio_data) when a speech segment ends.
    fn process(&mut self, rms: f64, samples: &[i16]) -> Option<Vec<i16>> {
        let is_loud = rms > 0.01; // RMS threshold for speech

        if is_loud {
            self.silence_frames = 0;
            if !self.is_speaking {
                self.is_speaking = true;
                self.buffer.clear();
            }
            self.buffer.extend_from_slice(samples);
        } else if self.is_speaking {
            self.buffer.extend_from_slice(samples);
            self.silence_frames += 1;
            // 1 second of silence = end of speech
            let silence_threshold = self.sample_rate / (samples.len() as u32);
            if self.silence_frames >= silence_threshold || self.buffer.len() > (self.sample_rate * 15) as usize {
                self.is_speaking = false;
                let data = std::mem::take(&mut self.buffer);
                self.silence_frames = 0;
                return Some(data);
            }
        }
        None
    }
}

/// Start microphone capture with speech detection.
pub async fn start_mic_capture(app: AppHandle) -> Result<(), String> {
    if IS_CAPTURING.load(Ordering::SeqCst) {
        return Ok(());
    }

    let host = cpal::default_host();
    let device = host.default_input_device().ok_or("No microphone found")?;
    let supported_config = device.default_input_config().map_err(|e| format!("Input config error: {}", e))?;
    let sample_rate: u32 = supported_config.sample_rate();
    log::info!("Mic: {}Hz", sample_rate);

    let config = supported_config.config();
    IS_CAPTURING.store(true, Ordering::SeqCst);
    let app_handle = app.clone();
    let detector = Mutex::new(SpeechDetector::new(sample_rate));

    let stream = device
        .build_input_stream(
            config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                // Convert f32 to i16
                let samples: Vec<i16> = data.iter()
                    .map(|&s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
                    .collect();

                // RMS volume
                let sum: f64 = data.iter().map(|&s| (s as f64).powi(2)).sum();
                let rms = (sum / data.len() as f64).sqrt();
                let volume = (rms.min(0.5) * 200.0) as u32;
                let _ = app_handle.emit("audio:volume", serde_json::json!({ "rms": rms, "pct": volume }));

                // Speech detection
                if let Ok(mut det) = detector.lock() {
                    if let Some(audio_data) = det.process(rms, &samples) {
                        let sr = det.sample_rate;
                        let app = app_handle.clone();
                        std::thread::spawn(move || {
                            let _ = app.emit("audio:state", serde_json::json!({ "state": "thinking" }));
                            save_and_transcribe(&audio_data, sr, app);
                        });
                    }
                }
            },
            move |err| log::error!("Audio error: {}", err),
            None,
        )
        .map_err(|e| format!("Stream start error: {}", e))?;

    stream.play().map_err(|e| format!("Stream play error: {}", e))?;
    std::mem::forget(stream);

    let _ = app.emit("audio:state", serde_json::json!({ "state": "listening" }));
    Ok(())
}

pub async fn stop_mic_capture() -> Result<(), String> {
    IS_CAPTURING.store(false, Ordering::SeqCst);
    Ok(())
}

/// Save audio to WAV, transcribe, and send to Hermes.
fn save_and_transcribe(audio: &[i16], sample_rate: u32, app: AppHandle) {
    use std::io::Write;

    // Save WAV
    let dir = std::env::temp_dir().join("hermes-voicedesk");
    std::fs::create_dir_all(&dir).ok();
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros();
    let path = dir.join(format!("speech_{}.wav", ts));

    let mut wav = Vec::new();
    let data_size = (audio.len() * 2) as u32;
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36u32 + data_size).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());   // PCM
    wav.extend_from_slice(&1u16.to_le_bytes());   // mono
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    for &s in audio { wav.extend_from_slice(&s.to_le_bytes()); }

    std::fs::write(&path, &wav).ok();
    log::info!("Saved speech: {} ({} samples, {:.1}s)", path.display(), audio.len(), audio.len() as f32 / sample_rate as f32);

    // Transcribe with faster-whisper
    let path_str = path.to_string_lossy().to_string();
    let text = transcribe_with_whisper(&path_str);

    // Emit transcribed text to frontend
    let _ = app.emit("stt:result", serde_json::json!({ "text": text }));
}

fn transcribe_with_whisper(path: &str) -> String {
    let script = format!(
        r#"
import sys
try:
    from faster_whisper import WhisperModel
    model = WhisperModel("base", device="auto", compute_type="auto")
    segments, _ = model.transcribe("{}", beam_size=5)
    text = " ".join(s.text.strip() for s in segments)
    print(text.strip() or "[silence]")
except Exception as e:
    print(f"[STT error: {{e}}]")
"#,
        path
    );

    match std::process::Command::new("python3").arg("-c").arg(&script).output() {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if text.is_empty() { "[no speech detected]".to_string() } else { text }
        }
        Err(e) => format!("[STT failed: {}]", e),
    }
}
