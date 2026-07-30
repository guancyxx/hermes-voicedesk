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

    // ── Edge case 1: Empty or near-empty audio ──
    if audio.len() < sample_rate as usize / 10 {
        // Audio shorter than 100ms — likely a click or noise spike
        log::debug!("STT: skipping clip too short ({} samples, {:.2}s)", audio.len(), audio.len() as f32 / sample_rate as f32);
        let _ = app.emit("stt:result", serde_json::json!({ "text": "[too short]", "confidence": 0.0 }));
        return;
    }

    // ── Edge case 2: Check RMS energy (detect pure noise / silence) ──
    let rms: f64 = {
        let sum_sq: f64 = audio.iter().map(|&s| (s as f64 / 32768.0).powi(2)).sum();
        (sum_sq / audio.len() as f64).sqrt()
    };
    if rms < 0.002 {
        // Very quiet — likely silence or room tone
        log::debug!("STT: skipping near-silent clip (RMS={:.6})", rms);
        let _ = app.emit("stt:result", serde_json::json!({ "text": "[silence]", "confidence": 0.0 }));
        return;
    }

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
    let duration_secs = audio.len() as f32 / sample_rate as f32;
    log::info!("Saved speech: {} ({} samples, {:.1}s, RMS={:.4})", path.display(), audio.len(), duration_secs, rms);

    let path_str = path.to_string_lossy().to_string();

    // ── Tier 1: faster-whisper (medium model) ──
    match transcribe_with_whisper(&path_str) {
        Some((text, confidence)) if confidence > 0.3 => {
            log::info!("STT (whisper): confidence={:.3} text={}", confidence, text);
            let _ = app.emit("stt:result", serde_json::json!({ "text": text, "confidence": confidence, "engine": "whisper" }));
            return;
        }
        Some((text, confidence)) => {
            // Whisper produced text but confidence is too low — still emit but note low confidence
            log::warn!("STT (whisper): low confidence={:.3} text={}", confidence, text);
            // Don't return — fall through to macOS native for a second opinion
        }
        None => {
            // Whisper failed entirely — fall through to macOS native
            log::warn!("STT (whisper): failed, trying macOS native fallback");
        }
    }

    // ── Tier 2: macOS native SFSpeechRecognizer fallback ──
    match transcribe_with_macos_native(&path_str) {
        Some(text) => {
            log::info!("STT (macOS native): text={}", text);
            let _ = app.emit("stt:result", serde_json::json!({ "text": text, "confidence": 0.8, "engine": "macos" }));
        }
        None => {
            log::warn!("STT: both whisper and macOS native failed");
            // If whisper gave us low-confidence text, use it as last resort
            let fallback = "[no speech detected]".to_string();
            let _ = app.emit("stt:result", serde_json::json!({ "text": fallback, "confidence": 0.0, "engine": "none" }));
        }
    }
}

/// Returns (text, confidence) or None on failure.
/// Confidence is 0.0–1.0 derived from avg_log_prob and no_speech_prob.
fn transcribe_with_whisper(path: &str) -> Option<(String, f64)> {
    let script = format!(
        r#"
import sys, json
try:
    from faster_whisper import WhisperModel
    model = WhisperModel("medium", device="auto", compute_type="auto")
    segments, info = model.transcribe("{}", beam_size=5, vad_filter=True)
    segments_list = list(segments)
    if not segments_list:
        print(json.dumps({{"text": "", "confidence": 0.0, "error": "no_segments"}}))
        sys.exit(0)
    # Filter by no_speech_prob and avg_log_prob
    filtered = []
    for s in segments_list:
        if s.no_speech_prob < 0.6 and s.avg_log_prob > -1.5:
            filtered.append(s)
    if not filtered:
        print(json.dumps({{"text": "", "confidence": 0.0, "error": "all_low_confidence"}}))
        sys.exit(0)
    text = " ".join(s.text.strip() for s in filtered)
    # Confidence: blend of avg_log_prob (scaled) and 1-no_speech_prob
    avg_confidence = sum(max(-1.5, min(0.0, s.avg_log_prob)) for s in filtered) / len(filtered)
    confidence = max(0.0, min(1.0, (avg_confidence + 1.5) / 1.5 * 0.7 + (1.0 - filtered[0].no_speech_prob) * 0.3))
    print(json.dumps({{"text": text.strip(), "confidence": round(confidence, 3)}}))
except Exception as e:
    print(json.dumps({{"text": "", "confidence": 0.0, "error": str(e)}}))
"#,
        path
    );

    match std::process::Command::new("python3").arg("-c").arg(&script).output() {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if !stderr.is_empty() {
                log::warn!("STT (whisper) stderr: {}", stderr);
            }
            if stdout.is_empty() {
                log::warn!("STT (whisper): empty output");
                return None;
            }
            match serde_json::from_str::<serde_json::Value>(&stdout) {
                Ok(v) => {
                    let text = v.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string();
                    let confidence = v.get("confidence").and_then(|c| c.as_f64()).unwrap_or(0.0);
                    let error = v.get("error").and_then(|e| e.as_str());
                    if let Some(err) = error {
                        if !err.is_empty() && text.is_empty() {
                            log::warn!("STT (whisper) error: {}", err);
                            return None;
                        }
                    }
                    if text.is_empty() || text == "[silence]" {
                        return None;
                    }
                    Some((text, confidence))
                }
                Err(e) => {
                    // Fallback: treat raw output as text
                    log::warn!("STT (whisper) parse error: {} raw={}", e, stdout);
                    if stdout.starts_with("[STT error") || stdout.starts_with("[silence]") {
                        None
                    } else {
                        Some((stdout, 0.5))
                    }
                }
            }
        }
        Err(e) => {
            log::error!("STT (whisper) process failed: {}", e);
            None
        }
    }
}

/// macOS native speech recognition via compiled Swift helper.
/// Returns Some(text) on success, None on failure.
fn transcribe_with_macos_native(path: &str) -> Option<String> {
    // Locate the compiled helper binary.
    // Priority: 1) next to the current executable, 2) in target dir, 3) PATH
    let helper_name = "macos-stt-helper";
    let helper_path = find_helper_binary(helper_name)?;

    match std::process::Command::new(&helper_path)
        .arg(path)
        .output()
    {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if !stderr.is_empty() {
                log::warn!("STT (macOS) stderr: {}", stderr);
            }
            if stdout.is_empty() || stdout == "[silence]" || stdout.starts_with("[macos_stt_error") {
                log::warn!("STT (macOS): no transcription — {}", stdout);
                None
            } else {
                Some(stdout)
            }
        }
        Err(e) => {
            log::error!("STT (macOS) helper failed: {}", e);
            None
        }
    }
}

/// Find the compiled macos-stt-helper binary.
fn find_helper_binary(name: &str) -> Option<std::path::PathBuf> {
    // 1. Next to current executable (production — bundled in .app/Contents/MacOS/)
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.parent().unwrap_or(std::path::Path::new(".")).join(name);
        if sibling.exists() {
            log::debug!("Found helper at: {}", sibling.display());
            return Some(sibling);
        }
    }

    // 2. In target directory (development — cargo build output)
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

    // 3. Check PATH
    if let Ok(paths) = std::env::var("PATH") {
        for dir in paths.split(':') {
            let candidate = std::path::Path::new(dir).join(name);
            if candidate.exists() {
                log::debug!("Found helper at: {}", candidate.display());
                return Some(candidate);
            }
        }
    }

    log::warn!("Helper binary '{}' not found", name);
    None
}
