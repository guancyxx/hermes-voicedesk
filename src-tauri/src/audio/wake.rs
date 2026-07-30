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

    log::info!("No Picovoice key — using VAD-based speech activation");
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
                    serde_json::json!({"error": format!("{}", e)}),
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

        let reader = BufReader::new(stdout);

        let _ = app_handle.emit(
            "wake:state",
            serde_json::json!({"state": "waiting", "mode": "porcupine", "keyword": kw}),
        );

        for line in reader.lines() {
            if !WAKE_ACTIVE.load(Ordering::SeqCst) {
                break;
            }

            match line {
                Ok(l) => {
                    log::debug!("Porcupine output: {}", l);
                    if let Ok(event) = serde_json::from_str::<serde_json::Value>(&l) {
                        let etype = event
                            .get("event")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        match etype {
                            "ready" => {
                                log::info!("Porcupine ready");
                            }
                            "wake_word" => {
                                let detected = event
                                    .get("keyword")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("jarvis");
                                log::info!("Wake word detected: {}", detected);
                                WAKE_WORD_DETECTED.store(true, Ordering::SeqCst);
                                let _ = app_handle.emit(
                                    "wake:detected",
                                    serde_json::json!({"keyword": detected}),
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
        log::info!("VAD wake: {}Hz microphone", sample_rate);

        let config = supported_cfg.config().clone();

        // Shared state for the audio callback
        let speech_frames = Arc::new(AtomicU32::new(0));
        let silence_frames = Arc::new(AtomicU32::new(0));

        let speech_frames_cb = speech_frames.clone();
        let silence_frames_cb = silence_frames.clone();
        let app_cb = app_handle.clone();

        let _ = app_handle.emit(
            "wake:state",
            serde_json::json!({"state": "waiting", "mode": "vad", "keyword": "[any speech]"}),
        );

        let stream = match device.build_input_stream(
            config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                if !WAKE_ACTIVE.load(Ordering::SeqCst) {
                    return;
                }

                let sum: f64 = data.iter().map(|&s| (s as f64).powi(2)).sum();
                let rms = (sum / data.len() as f64).sqrt();
                let is_speech = rms > 0.015;

                if is_speech {
                    let sf = speech_frames_cb.fetch_add(1, Ordering::SeqCst) + 1;
                    silence_frames_cb.store(0, Ordering::SeqCst);

                    // ~1.5 seconds of sustained speech triggers activation
                    // At typical 30ms frames: ~50 frames/sec, so ~75 frames
                    if sf >= 75 {
                        log::info!("VAD wake: speech detected ({} frames)", sf);
                        WAKE_WORD_DETECTED.store(true, Ordering::SeqCst);
                        let _ = app_cb.emit(
                            "wake:detected",
                            serde_json::json!({"keyword": "[speech]", "mode": "vad"}),
                        );
                        WAKE_ACTIVE.store(false, Ordering::SeqCst);
                    }
                } else {
                    let sil = silence_frames_cb.fetch_add(1, Ordering::SeqCst) + 1;
                    // ~2 seconds of silence resets speech counter
                    if sil > 100 {
                        speech_frames_cb.store(0, Ordering::SeqCst);
                    }
                }
            },
            move |err| {
                log::error!("VAD wake audio error: {}", err);
            },
            None,
        ) {
            Ok(s) => s,
            Err(e) => {
                log::error!("VAD wake: stream build error: {}", e);
                WAKE_ACTIVE.store(false, Ordering::SeqCst);
                return;
            }
        };

        if let Err(e) = stream.play() {
            log::error!("VAD wake: stream play error: {}", e);
            WAKE_ACTIVE.store(false, Ordering::SeqCst);
            return;
        }

        // Keep stream alive while active
        while WAKE_ACTIVE.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        drop(stream);
        log::info!("VAD wake detection ended");
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
            return sibling;
        }
    }

    // 2. In project src-tauri/scripts/ (development)
    let mut current =
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    loop {
        let candidate = current.join("src-tauri").join("scripts").join(name);
        if candidate.exists() {
            return candidate;
        }
        if !current.pop() {
            break;
        }
    }

    // 3. Fallback
    std::path::PathBuf::from(name)
}
