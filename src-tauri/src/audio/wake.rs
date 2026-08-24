//! Wake word detection module.
//!
//! Two modes:
//! 1. Porcupine (Python subprocess) — requires Picovoice access key.
//! 2. VAD fallback — built-in speech energy detection (any sustained speech triggers).
//!
//! Architecture: wake word detector and mic capture take turns using the mic.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

static WAKE_ACTIVE: AtomicBool = AtomicBool::new(false);
static WAKE_WORD_DETECTED: AtomicBool = AtomicBool::new(false);

/// Start wake word detection. Uses Porcupine if access_key provided, else VAD.
pub fn start_wake_word(app: AppHandle, access_key: Option<String>, keyword: Option<String>) {
    if WAKE_ACTIVE.load(Ordering::SeqCst) {
        log::info!("Wake word detector already active");
        return;
    }

    WAKE_ACTIVE.store(true, Ordering::SeqCst);
    WAKE_WORD_DETECTED.store(false, Ordering::SeqCst);

    let kw = keyword.unwrap_or_else(|| "jarvis".to_string());

    if let Some(key) = access_key {
        if !key.is_empty() {
            log::info!("Starting Porcupine wake word: keyword={}", kw);
            spawn_porcupine(app, key, kw);
            return;
        }
    }

    log::info!("No Picovoice key — using VAD-based speech activation (keyword={})", kw);
    spawn_vad_wake(app);
}

/// Stop wake word detection.
pub fn stop_wake_word() {
    WAKE_ACTIVE.store(false, Ordering::SeqCst);
}

/// Whether wake word has been detected.
pub fn is_wake_detected() -> bool {
    WAKE_WORD_DETECTED.load(Ordering::SeqCst)
}

/// Reset the detection flag (called after transitioning to listening).
pub fn reset_wake_detected() {
    WAKE_WORD_DETECTED.store(false, Ordering::SeqCst);
}

// ── Porcupine subprocess ──────────────────────────────────────────────

fn spawn_porcupine(app: AppHandle, access_key: String, keyword: String) {
    let app_handle = app.clone();
    let kw = keyword.clone();

    std::thread::spawn(move || {
        let script_path = find_script_path("wake_word.py");
        log::info!("Porcupine script path: {:?}", script_path);

        let mut child = match Command::new("python3")
            .arg(script_path.as_os_str())
            .arg("--keyword")
            .arg(&kw)
            .arg("--access-key")
            .arg(&access_key)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                log::error!("Failed to spawn wake word detector: {}", e);
                let _ = app_handle.emit(
                    "wake:error",
                    serde_json::json!({"error": format!("Failed to spawn: {}", e)}),
                );
                WAKE_ACTIVE.store(false, Ordering::SeqCst);
                return;
            }
        };

        // Take stdout BEFORE moving child into the Arc<Mutex>
        let stdout = match child.stdout.take() {
            Some(s) => s,
            None => {
                log::error!("Wake word process has no stdout");
                let _ = child.kill();
                WAKE_ACTIVE.store(false, Ordering::SeqCst);
                return;
            }
        };

        // Also capture stderr for debugging
        let stderr = child.stderr.take();

        // Keep child handle for cleanup when WAKE_ACTIVE goes false
        let child_handle = Arc::new(Mutex::new(Some(child)));
        let child_for_cleanup = child_handle.clone();

        // Spawn a watcher thread that kills the process when WAKE_ACTIVE goes false
        std::thread::spawn(move || {
            while WAKE_ACTIVE.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            if let Ok(mut guard) = child_for_cleanup.lock() {
                if let Some(ref mut c) = *guard {
                    // Try graceful stop via stdin
                    if let Some(ref mut stdin) = c.stdin {
                        use std::io::Write;
                        let _ = writeln!(stdin, "stop");
                    }
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    let _ = c.kill();
                    let _ = c.wait();
                }
            }
        });

        // Spawn a stderr reader thread
        if let Some(stderr) = stderr {
            std::thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines() {
                    match line {
                        Ok(l) if !l.is_empty() => {
                            log::warn!("Porcupine stderr: {}", l);
                        }
                        _ => break,
                    }
                }
            });
        }

        let reader = BufReader::new(stdout);

        let _ = app_handle.emit(
            "wake:state",
            serde_json::json!({"state": "waiting", "mode": "porcupine", "keyword": kw.clone()}),
        );

        for line in reader.lines() {
            if !WAKE_ACTIVE.load(Ordering::SeqCst) {
                break;
            }

            match line {
                Ok(l) => {
                    log::info!("Porcupine output: {}", l);
                    if let Ok(event) = serde_json::from_str::<serde_json::Value>(&l) {
                        let etype = event
                            .get("event")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        match etype {
                            "ready" => {
                                let mode = event
                                    .get("mode")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("porcupine");
                                log::info!("Porcupine ready (mode={})", mode);
                            }
                            "debug" => {
                                // Debug messages from the Python script
                                let msg = event
                                    .get("message")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                log::debug!("Porcupine debug: {}", msg);
                            }
                            "wake_word" => {
                                let detected = event
                                    .get("keyword")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("jarvis");
                                let mode = event
                                    .get("mode")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("porcupine");
                                log::info!(
                                    "Wake word detected: {} (mode={})",
                                    detected,
                                    mode
                                );
                                WAKE_WORD_DETECTED.store(true, Ordering::SeqCst);
                                let _ = app_handle.emit(
                                    "wake:detected",
                                    serde_json::json!({"keyword": detected, "mode": mode}),
                                );
                                WAKE_ACTIVE.store(false, Ordering::SeqCst);
                                break;
                            }
                            "error" => {
                                let msg = event
                                    .get("message")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("unknown");
                                log::error!("Porcupine error: {}", msg);
                                let _ = app_handle.emit(
                                    "wake:error",
                                    serde_json::json!({"error": msg}),
                                );
                                WAKE_ACTIVE.store(false, Ordering::SeqCst);
                                break;
                            }
                            _ => {}
                        }
                    } else {
                        log::debug!("Porcupine non-JSON output: {}", l);
                    }
                }
                Err(e) => {
                    log::error!("Wake word stdout read error: {}", e);
                    break;
                }
            }
        }

        // Cleanup
        if let Ok(mut guard) = child_handle.lock() {
            if let Some(ref mut c) = *guard {
                let _ = c.wait();
            }
            *guard = None;
        }
        WAKE_ACTIVE.store(false, Ordering::SeqCst);
        log::info!("Porcupine wake word detection ended");
    });
}

// ── VAD fallback ──────────────────────────────────────────────────────

fn spawn_vad_wake(app: AppHandle) {
    let app_handle = app.clone();

    std::thread::spawn(move || {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

        // VAD parameters (tuned for reliable speech detection)
        // RMS threshold: 0.003 balances sensitivity vs noise rejection.
        // 0.002 was too sensitive — ambient noise triggered endless wake→listen loops.
        const RMS_THRESHOLD: f64 = 0.003;
        // Frames of sustained speech needed to trigger (~0.45s at 30ms frames)
        const TRIGGER_FRAMES: u32 = 15;
        // Silence frames needed to reset the counter (~1.2s)
        const SILENCE_RESET_FRAMES: u32 = 40;

        // The input device can be renegotiated under us (observed 2026-08-24:
        // "Device sample rate changed" right after launch when the default mic
        // gets reset, e.g. by a virtual audio driver). cpal then kills the
        // stream via the error callback; only logging it left wake detection
        // permanently dead. Rebuild the stream with a FRESH device + config.
        const MAX_STREAM_RETRIES: u32 = 5;
        const RETRY_DELAY_MS: u64 = 500;

        let mut retries: u32 = 0;

        loop {
            let host = cpal::default_host();
            let device = match host.default_input_device() {
                Some(d) => d,
                None => {
                    log::error!("VAD wake: no microphone");
                    let _ = app_handle.emit(
                        "wake:error",
                        serde_json::json!({"error": "No microphone"}),
                    );
                    WAKE_ACTIVE.store(false, Ordering::SeqCst);
                    return;
                }
            };

            let supported_cfg = match device.default_input_config() {
                Ok(c) => c,
                Err(e) => {
                    log::error!("VAD wake: config error: {}", e);
                    WAKE_ACTIVE.store(false, Ordering::SeqCst);
                    return;
                }
            };

            let sample_rate = supported_cfg.sample_rate();
            log::info!(
                "VAD wake: {}Hz microphone, config={:?}",
                sample_rate,
                supported_cfg
            );

            let config = supported_cfg.config().clone();

            // Set by the cpal error callback — tells the keep-alive loop to
            // drop the stream and rebuild with a fresh device + fresh counters.
            let stream_failed = Arc::new(AtomicBool::new(false));
            let stream_failed_err = stream_failed.clone();

            // Shared state for the audio callback (fresh per stream attempt —
            // stale speech/silence counters must not leak across rebuilds)
            let speech_frames = Arc::new(AtomicU32::new(0));
            let silence_frames = Arc::new(AtomicU32::new(0));
            let last_rms = Arc::new(std::sync::atomic::AtomicU64::new(0));

            let speech_frames_cb = speech_frames.clone();
            let silence_frames_cb = silence_frames.clone();
            let last_rms_cb = last_rms.clone();
            let app_cb = app_handle.clone();

            let _ = app_handle.emit(
                "wake:state",
                serde_json::json!({
                    "state": "waiting",
                    "mode": "vad",
                    "keyword": "[any speech]",
                    "rms_threshold": RMS_THRESHOLD,
                    "trigger_frames": TRIGGER_FRAMES,
                    "retry": retries,
                }),
            );

            log::info!(
                "VAD wake: RMS threshold={}, trigger={} frames (~{:.1}s)",
                RMS_THRESHOLD,
                TRIGGER_FRAMES,
                TRIGGER_FRAMES as f64 * 0.03,
            );

            let stream = match device.build_input_stream(
                config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if !WAKE_ACTIVE.load(Ordering::SeqCst) {
                        return;
                    }

                    let sum: f64 = data.iter().map(|&s| (s as f64).powi(2)).sum();
                    let rms = (sum / data.len() as f64).sqrt();
                    let is_speech = rms > RMS_THRESHOLD;

                    // Store RMS for debug (as fixed-point u64: integer part + 6 decimal places)
                    last_rms_cb.store((rms * 1_000_000.0) as u64, Ordering::Relaxed);

                    if is_speech {
                        let sf = speech_frames_cb.fetch_add(1, Ordering::SeqCst) + 1;
                        silence_frames_cb.store(0, Ordering::SeqCst);

                        // Log progress every 10 frames
                        if sf % 10 == 0 {
                            log::debug!(
                                "VAD wake: speech frame {}/{} (RMS={:.6})",
                                sf,
                                TRIGGER_FRAMES,
                                rms
                            );
                        }

                        if sf >= TRIGGER_FRAMES {
                            log::info!(
                                "VAD wake: speech detected ({} frames, RMS={:.6})",
                                sf,
                                rms
                            );
                            WAKE_WORD_DETECTED.store(true, Ordering::SeqCst);
                            let _ = app_cb.emit(
                                "wake:detected",
                                serde_json::json!({"keyword": "[speech]", "mode": "vad", "rms": rms, "frames": sf}),
                            );
                            WAKE_ACTIVE.store(false, Ordering::SeqCst);
                        }
                    } else {
                        let sil = silence_frames_cb.fetch_add(1, Ordering::SeqCst) + 1;
                        if sil > SILENCE_RESET_FRAMES {
                            let old = speech_frames_cb.swap(0, Ordering::SeqCst);
                            if old > 5 {
                                log::debug!(
                                    "VAD wake: reset speech counter (was {} frames, RMS at reset={:.6})",
                                    old,
                                    rms
                                );
                            }
                        }
                    }
                },
                move |err| {
                    log::error!("VAD wake audio error: {}", err);
                    stream_failed_err.store(true, Ordering::SeqCst);
                },
                None,
            ) {
                Ok(s) => s,
                Err(e) => {
                    log::error!("VAD wake: stream build error: {}", e);
                    let _ = app_handle.emit(
                        "wake:error",
                        serde_json::json!({"error": format!("Stream build error: {}", e)}),
                    );
                    WAKE_ACTIVE.store(false, Ordering::SeqCst);
                    return;
                }
            };

            if let Err(e) = stream.play() {
                log::error!("VAD wake: stream play error: {}", e);
                let _ = app_handle.emit(
                    "wake:error",
                    serde_json::json!({"error": format!("Stream play error: {}", e)}),
                );
                WAKE_ACTIVE.store(false, Ordering::SeqCst);
                return;
            }

            // Emit RMS debug values periodically (every second) — tied to this
            // stream attempt so the old thread exits when the stream dies.
            let last_rms_ref = last_rms.clone();
            let stream_failed_dbg = stream_failed.clone();
            std::thread::spawn(move || {
                while WAKE_ACTIVE.load(Ordering::SeqCst)
                    && !stream_failed_dbg.load(Ordering::SeqCst)
                {
                    std::thread::sleep(std::time::Duration::from_millis(1000));
                    let raw = last_rms_ref.load(Ordering::Relaxed);
                    let rms = raw as f64 / 1_000_000.0;
                    log::debug!("VAD wake: current RMS={:.6} (threshold={})", rms, RMS_THRESHOLD);
                }
            });

            // Keep stream alive while active and healthy
            while WAKE_ACTIVE.load(Ordering::SeqCst)
                && !stream_failed.load(Ordering::SeqCst)
            {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }

            drop(stream);

            if !WAKE_ACTIVE.load(Ordering::SeqCst) {
                // Stopped normally (user stop_wake_word, or wake detected)
                log::info!("VAD wake detection ended");
                return;
            }

            // Stream died while still active → rebuild after a short delay
            retries += 1;
            if retries > MAX_STREAM_RETRIES {
                log::error!(
                    "VAD wake: stream kept failing, giving up after {} retries",
                    MAX_STREAM_RETRIES
                );
                let _ = app_handle.emit(
                    "wake:error",
                    serde_json::json!({"error": format!("VAD stream failed after {} retries", MAX_STREAM_RETRIES)}),
                );
                WAKE_ACTIVE.store(false, Ordering::SeqCst);
                return;
            }

            log::warn!(
                "VAD wake: stream died, rebuilding ({}/{}), waiting {}ms",
                retries,
                MAX_STREAM_RETRIES,
                RETRY_DELAY_MS
            );
            std::thread::sleep(std::time::Duration::from_millis(RETRY_DELAY_MS));
        }
    });
}

// ── Utility ───────────────────────────────────────────────────────────

fn find_script_path(name: &str) -> std::path::PathBuf {
    // 1. Next to executable (production bundle)
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("scripts")
            .join(name);
        if sibling.exists() {
            log::info!("Found script at exe sibling: {:?}", sibling);
            return sibling;
        }
    }

    // 2. In project src-tauri/scripts/ (development)
    let mut current =
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    loop {
        let candidate = current.join("src-tauri").join("scripts").join(name);
        if candidate.exists() {
            log::info!("Found script in project tree: {:?}", candidate);
            return candidate;
        }
        if !current.pop() {
            break;
        }
    }

    // 3. Fallback
    log::warn!("Script '{}' not found, using bare filename", name);
    std::path::PathBuf::from(name)
}
