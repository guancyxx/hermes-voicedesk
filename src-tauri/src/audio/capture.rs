use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;
use tauri::{AppHandle, Emitter};

static IS_CAPTURING: AtomicBool = AtomicBool::new(false);

/// When true, the mic callback returns immediately without processing any audio.
/// Set during TTS playback to completely prevent echo capture.
static MIC_SUSPENDED: AtomicBool = AtomicBool::new(false);

/// TTS playback counter. > 0 means TTS is currently playing.
/// Using a counter instead of a bool prevents a stale TTS-end callback from
/// resetting the state while a newer TTS utterance is still in progress.
static TTS_PLAYING_COUNT: AtomicI32 = AtomicI32::new(0);

/// Timestamp (monotonic millis) when the last TTS utterance ended.
/// Used for a cooldown period after TTS to prevent echo from triggering
/// speech detection.
static LAST_TTS_END_MS: AtomicU64 = AtomicU64::new(0);

/// Cooldown after TTS ends (milliseconds) during which speech detection
/// is suppressed to avoid capturing TTS echo through the mic.
const TTS_ECHO_COOLDOWN_MS: u64 = 1500;

/// Module-level static so both start_mic_capture and stop_mic_capture share the same stream handle.
/// Previously each function declared its own local static — stop_mic_capture was setting a
/// *different* Option to None, so the real stream never dropped.
static ACTIVE_STREAM: Mutex<Option<cpal::Stream>> = Mutex::new(None);

/// Suspend the microphone callback entirely — no samples are processed, no events emitted.
/// Used during TTS playback to completely prevent echo capture.
pub fn suspend_mic() {
    MIC_SUSPENDED.store(true, Ordering::SeqCst);
}

/// Resume the microphone callback — samples are processed again normally.
pub fn resume_mic() {
    MIC_SUSPENDED.store(false, Ordering::SeqCst);
}

/// Signal that TTS started — capture should pause speech detection.
pub fn notify_tts_start() {
    TTS_PLAYING_COUNT.fetch_add(1, Ordering::SeqCst);
}

/// Signal that TTS ended — capture may resume speech detection after cooldown.
pub fn notify_tts_end() {
    let prev = TTS_PLAYING_COUNT.fetch_sub(1, Ordering::SeqCst);
    // Clamp to zero (defense against mismatched start/end)
    if prev <= 1 {
        TTS_PLAYING_COUNT.store(0, Ordering::SeqCst);
        LAST_TTS_END_MS.store(now_millis(), Ordering::SeqCst);
    }
}

/// Returns true if TTS is currently playing (counter > 0).
fn is_tts_playing() -> bool {
    TTS_PLAYING_COUNT.load(Ordering::SeqCst) > 0
}

/// Returns true if we're still within the echo cooldown window.
fn in_tts_cooldown() -> bool {
    let last = LAST_TTS_END_MS.load(Ordering::SeqCst);
    if last == 0 {
        return false;
    }
    now_millis().saturating_sub(last) < TTS_ECHO_COOLDOWN_MS
}

fn now_millis() -> u64 {
    // Use std::time::Instant for monotonic time, but we need a u64 millis value.
    // We use a lazy-static base Instant to compute relative millis.
    use std::sync::OnceLock;
    static BASE: OnceLock<Instant> = OnceLock::new();
    let base = BASE.get_or_init(Instant::now);
    base.elapsed().as_millis() as u64
}

struct SpeechDetector {
    is_speaking: bool,
    buffer: Vec<i16>,
    silence_samples: u32,
    sample_rate: u32,
    /// When true, speech detection is fully suppressed (TTS playing or cooldown).
    suppressed: bool,
}

impl SpeechDetector {
    fn new(sample_rate: u32) -> Self {
        Self {
            is_speaking: false,
            buffer: Vec::new(),
            silence_samples: 0,
            sample_rate,
            suppressed: false,
        }
    }

    /// 3 seconds of silence = end of speech
    fn silence_threshold(&self) -> u32 {
        self.sample_rate * 3
    }

    fn max_buffer(&self) -> usize {
        (self.sample_rate * 15) as usize
    }

    /// Reset detector state (called when TTS starts to flush any in-flight speech).
    fn reset(&mut self) {
        self.is_speaking = false;
        self.buffer.clear();
        self.silence_samples = 0;
    }

    fn process(&mut self, rms: f64, samples: &[i16]) -> Option<Vec<i16>> {
        // If suppressed, discard samples entirely
        if self.suppressed {
            return None;
        }

        let is_loud = rms > 0.01;

        if is_loud {
            self.silence_samples = 0;
            if !self.is_speaking {
                self.is_speaking = true;
                self.buffer.clear();
            }
            self.buffer.extend_from_slice(samples);
        } else if self.is_speaking {
            self.buffer.extend_from_slice(samples);
            self.silence_samples += samples.len() as u32;
            if self.silence_samples >= self.silence_threshold() || self.buffer.len() > self.max_buffer() {
                self.is_speaking = false;
                let data = std::mem::take(&mut self.buffer);
                self.silence_samples = 0;
                return Some(data);
            }
        }
        None
    }
}

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
                if !IS_CAPTURING.load(Ordering::SeqCst) {
                    return;
                }

                // If mic is suspended (TTS playing), drop all samples immediately.
                // No volume events, no RMS, no speech detection — complete silence to frontend.
                if MIC_SUSPENDED.load(Ordering::SeqCst) {
                    return;
                }

                let samples: Vec<i16> = data.iter()
                    .map(|&s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
                    .collect();

                // RMS volume
                let sum: f64 = data.iter().map(|&s| (s as f64).powi(2)).sum();
                let rms = (sum / data.len() as f64).sqrt();
                let pct = (rms.min(0.5) * 200.0) as u32;
                let _ = app_handle.emit("audio:volume", serde_json::json!({ "rms": rms, "pct": pct }));

                // Determine suppression state
                let tts_active = is_tts_playing();
                let tts_cooldown = in_tts_cooldown();

                if let Ok(mut det) = detector.lock() {
                    // Update suppression state
                    let should_suppress = tts_active || tts_cooldown;
                    if should_suppress != det.suppressed {
                        if should_suppress {
                            // TTS just started or is in cooldown — reset detector to flush
                            // any in-progress speech detection
                            det.reset();
                        }
                        det.suppressed = should_suppress;
                    }

                    if should_suppress {
                        return;
                    }

                    // Speech detection
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

    // Store stream so stop_mic_capture can drop it
    *ACTIVE_STREAM.lock().unwrap() = Some(stream);

    let _ = app.emit("audio:state", serde_json::json!({ "state": "listening" }));
    Ok(())
}

pub async fn stop_mic_capture() -> Result<(), String> {
    IS_CAPTURING.store(false, Ordering::SeqCst);
    // Drop the stream to stop audio capture
    *ACTIVE_STREAM.lock().unwrap() = None;
    Ok(())
}

fn save_and_transcribe(audio: &[i16], sample_rate: u32, app: AppHandle) {
    use std::io::Write;

    let dir = std::env::temp_dir().join("hermes-voicedesk");
    std::fs::create_dir_all(&dir).ok();
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros();
    let path = dir.join(format!("speech_{}.wav", ts));

    let mut wav = Vec::new();
    let data_size = (audio.len() * 2) as u32;
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36u32 + data_size).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    for &s in audio { wav.extend_from_slice(&s.to_le_bytes()); }

    std::fs::write(&path, &wav).ok();
    log::info!("Saved speech: {} ({} samples, {:.1}s)", path.display(), audio.len(), audio.len() as f32 / sample_rate as f32);

    let path_str = path.to_string_lossy().to_string();
    let text = transcribe_with_whisper(&path_str);
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
