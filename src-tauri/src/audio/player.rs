/// TTS playback for Hermes VoiceDesk.
///
/// Voice providers (priority order):
/// 1. **Edge-TTS** (`en-GB-RyanNeural`) — Best JARVIS-like British AI butler voice.
///    Free Microsoft Edge TTS via `edge-tts` Python CLI. Requires internet.
/// 2. **macOS say + ffmpeg** — Offline fallback. `say -v Daniel` with JARVIS
///    audio post-processing via ffmpeg filters.
/// 3. **macOS say direct** — Last resort, no dependencies.
///
/// Echo prevention:
///   Mic is suspended BEFORE TTS begins, resumed after cooldown when TTS finishes.
///
/// Queue management:
///   Only ONE utterance runs at a time. Starting new speech kills the previous one.
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use tauri::{AppHandle, Emitter};

use crate::audio::capture;

/// Monotonically increasing generation counter. Incremented on every `speak()` call.
static TTS_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Handle to the currently-running child process so we can kill it before starting a new one.
static CURRENT_CHILD: Mutex<Option<Child>> = Mutex::new(None);

/// JARVIS mode: when enabled, TTS goes through the best available JARVIS voice pipeline.
static JARVIS_MODE: AtomicBool = AtomicBool::new(true);

/// Current macOS voice name (default: Daniel — British male, JARVIS-like fallback).
static VOICE_NAME: Mutex<String> = Mutex::new(String::new());

/// Edge-TTS voice name for JARVIS mode.
static EDGE_TTS_VOICE: Mutex<String> = Mutex::new(String::new());

// ── Edge-TTS ─────────────────────────────────────────────────────────────────

/// Edge-TTS binary paths to try (in order).
const EDGE_TTS_PATHS: &[&str] = &[
    "edge-tts",                                                     // In PATH
    "/opt/homebrew/bin/edge-tts",                                   // Homebrew
];

/// Find the edge-tts binary, trying multiple paths and `python3 -m edge_tts`.
fn find_edge_tts() -> Option<String> {
    // 1. Try direct binary paths
    for path in EDGE_TTS_PATHS {
        if Command::new("which")
            .arg(path)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return Some(path.to_string());
        }
    }

    // 2. Try user-local Python bin (macOS homebrew/standard pip)
    let home = std::env::var("HOME").unwrap_or_default();
    let user_python_bins = [
        format!("{}/Library/Python/3.9/bin/edge-tts", home),
        format!("{}/Library/Python/3.10/bin/edge-tts", home),
        format!("{}/Library/Python/3.11/bin/edge-tts", home),
        format!("{}/Library/Python/3.12/bin/edge-tts", home),
        format!("{}/Library/Python/3.13/bin/edge-tts", home),
        format!("{}/.local/bin/edge-tts", home),
    ];
    for path in &user_python_bins {
        if PathBuf::from(path).exists() {
            return Some(path.clone());
        }
    }

    // 3. Try `python3 -m edge_tts`
    if Command::new("python3")
        .args(["-c", "import edge_tts"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return Some("python3 -m edge_tts".to_string());
    }

    None
}

/// Check if edge-tts is available.
fn has_edge_tts() -> bool {
    find_edge_tts().is_some()
}

/// Get the edge-tts command (binary path or python3 -m edge_tts).
fn edge_tts_cmd() -> String {
    find_edge_tts().unwrap_or_else(|| "edge-tts".to_string())
}

/// Get the configured edge-tts voice.
fn get_edge_tts_voice() -> String {
    let voice = EDGE_TTS_VOICE.lock().unwrap();
    if voice.is_empty() {
        "en-GB-RyanNeural".to_string()
    } else {
        voice.clone()
    }
}

// ── Voice getter ─────────────────────────────────────────────────────────────

fn get_voice() -> String {
    let voice = VOICE_NAME.lock().unwrap();
    if voice.is_empty() {
        "Daniel".to_string()
    } else {
        voice.clone()
    }
}

// ── FFmpeg ───────────────────────────────────────────────────────────────────

/// Enhanced JARVIS ffmpeg audio filter chain.
///
/// Effect breakdown:
/// - asetrate+atempo: pitch shift down 5% without changing speed (deeper, more butler-like)
/// - aecho x 2: dual-tap reverb simulating a small room presence
/// - highpass: remove sub-bass rumble
/// - lowpass: gentle high-end rolloff for warmth
/// - compand: smooth dynamic range for broadcast-consistent levels
const JARVIS_FILTER: &str = concat!(
    "asetrate=44100*0.95,atempo=1/0.95,",
    "aecho=0.8:0.7:20:0.3,aecho=0.8:0.6:50:0.2,",
    "highpass=f=100,lowpass=f=8000,",
    "compand=attacks=0.003:decays=0.1:points=-90/-90|-30/-15|0/-3:gain=3"
);

fn has_ffmpeg() -> bool {
    Command::new("which")
        .arg("ffmpeg")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Enable or disable JARVIS-mode TTS.
pub fn set_jarvis_mode(enabled: bool) {
    JARVIS_MODE.store(enabled, Ordering::SeqCst);
}

/// Get current JARVIS mode.
pub fn get_jarvis_mode() -> bool {
    JARVIS_MODE.load(Ordering::SeqCst)
}

/// Set the macOS voice to use for TTS fallback (e.g., "Daniel", "Oliver").
pub fn set_voice(name: &str) {
    *VOICE_NAME.lock().unwrap() = name.to_string();
}

/// Get current macOS voice name.
pub fn get_voice_name() -> String {
    get_voice()
}

/// Set the Edge-TTS voice (e.g., "en-GB-RyanNeural", "en-GB-ThomasNeural").
pub fn set_edge_tts_voice(name: &str) {
    *EDGE_TTS_VOICE.lock().unwrap() = name.to_string();
}

/// Check if Edge-TTS is available on this system.
pub fn has_jarvis_voice() -> bool {
    has_edge_tts()
}

/// Get the current TTS provider description.
pub fn get_tts_provider() -> &'static str {
    if has_edge_tts() {
        "Edge-TTS (en-GB-RyanNeural)"
    } else if has_ffmpeg() {
        "macOS say + ffmpeg JARVIS filter"
    } else {
        "macOS say (direct)"
    }
}

// ── Core speak pipeline ──────────────────────────────────────────────────────

/// Speak text using the best available TTS pipeline.
///
/// Priority:
/// 1. JARVIS mode + edge-tts available → Edge-TTS en-GB-RyanNeural
/// 2. JARVIS mode + ffmpeg available → say + JARVIS ffmpeg filter
/// 3. Normal mode → say direct (with optional JARVIS ffmpeg if enabled)
pub fn speak(text: &str, app: AppHandle) -> Result<(), String> {
    // 1. Kill previous utterance
    stop_child_process();

    // 2. Bump generation counter — invalidates pending callbacks
    let gen = TTS_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;

    // 3. Signal TTS-start and suspend mic
    capture::notify_tts_start();
    capture::suspend_mic();

    let text_owned = text.to_string();
    let use_jarvis = JARVIS_MODE.load(Ordering::SeqCst);

    std::thread::spawn(move || {
        let result = if use_jarvis && has_edge_tts() {
            // Best: Edge-TTS with JARVIS voice
            speak_with_edgetts(&text_owned)
        } else if use_jarvis && has_ffmpeg() {
            // Fallback: macOS say + ffmpeg JARVIS filters
            let voice = get_voice();
            speak_with_jarvis_ffmpeg(&text_owned, &voice)
        } else {
            // Direct: macOS say (no processing)
            let voice = get_voice();
            speak_direct(&text_owned, &voice)
        };

        if let Err(ref e) = result {
            log::error!("TTS error: {}", e);
        }

        // Only finalize if this generation is still current
        if TTS_GENERATION.load(Ordering::SeqCst) == gen {
            let _ = app.emit("tts:complete", serde_json::json!({}));
            let _ = app.emit("audio:state", serde_json::json!({"state": "idle"}));

            // Brief cooldown for residual echo before re-enabling mic
            std::thread::sleep(std::time::Duration::from_millis(500));

            capture::resume_mic();
            capture::notify_tts_end();
        }
    });

    Ok(())
}

// ── Edge-TTS pipeline ────────────────────────────────────────────────────────

/// Speak using Edge-TTS for the best JARVIS-like British AI butler voice.
///
/// Pipeline:
///   edge-tts --voice en-GB-RyanNeural --text "..." --write-media /tmp/tts.mp3
///   → afplay /tmp/tts.mp3
fn speak_with_edgetts(text: &str) -> Result<(), String> {
    let pid = std::process::id();
    let output_path = format!("/tmp/hermes_tts_edgetts_{}.mp3", pid);
    let voice = get_edge_tts_voice();

    // Edge-TTS supports --rate and --pitch for JARVIS-like adjustments
    // Slightly slower rate (-5%) and lower pitch (-3Hz) for measured butler delivery
    let cmd = edge_tts_cmd();

    let parts: Vec<&str> = cmd.split_whitespace().collect();
    let mut command = if parts.len() > 1 {
        // python3 -m edge_tts
        let mut c = Command::new(parts[0]);
        c.args(&parts[1..]);
        c
    } else {
        Command::new(&cmd)
    };

    let status = command
        .arg("--voice")
        .arg(&voice)
        .arg("--rate=-5%")
        .arg("--pitch=-3Hz")
        .arg("--text")
        .arg(text)
        .arg("--write-media")
        .arg(&output_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("Failed to run edge-tts: {}", e))?;

    if !status.success() {
        // Edge-TTS failed — fall back to say + ffmpeg
        log::warn!("Edge-TTS failed, falling back to macOS say + ffmpeg");
        let voice = get_voice();
        if has_ffmpeg() {
            return speak_with_jarvis_ffmpeg(text, &voice);
        } else {
            return speak_direct(text, &voice);
        }
    }

    // Play with afplay
    let child = Command::new("afplay")
        .arg(&output_path)
        .spawn()
        .map_err(|e| format!("Failed to spawn afplay: {}", e))?;

    *CURRENT_CHILD.lock().unwrap() = Some(child);

    if let Some(mut child_to_wait) = CURRENT_CHILD.lock().unwrap().take() {
        child_to_wait
            .wait()
            .map_err(|e| format!("afplay process error: {}", e))?;
    }

    // Clean up
    let _ = std::fs::remove_file(&output_path);

    Ok(())
}

// ── macOS say (direct) ───────────────────────────────────────────────────────

/// Direct playback: `say -v <voice> <text>` (no post-processing).
fn speak_direct(text: &str, voice: &str) -> Result<(), String> {
    let child = Command::new("say")
        .arg("-v")
        .arg(voice)
        .arg(text)
        .spawn()
        .map_err(|e| format!("Failed to spawn say: {}", e))?;

    *CURRENT_CHILD.lock().unwrap() = Some(child);

    if let Some(mut child_to_wait) = CURRENT_CHILD.lock().unwrap().take() {
        child_to_wait
            .wait()
            .map_err(|e| format!("say process error: {}", e))?;
    }

    Ok(())
}

// ── macOS say + ffmpeg JARVIS filter (fallback) ──────────────────────────────

/// JARVIS pipeline: say → raw aiff → ffmpeg → processed aiff → afplay.
fn speak_with_jarvis_ffmpeg(text: &str, voice: &str) -> Result<(), String> {
    let pid = std::process::id();
    let raw_path = format!("/tmp/hermes_tts_raw_{}.aiff", pid);
    let processed_path = format!("/tmp/hermes_tts_jarvis_{}.aiff", pid);

    // Step 1: Generate raw audio with `say`
    let status = Command::new("say")
        .arg("-v")
        .arg(voice)
        .arg("-o")
        .arg(&raw_path)
        .arg(text)
        .status()
        .map_err(|e| format!("Failed to run say: {}", e))?;

    if !status.success() {
        return Err(format!("say exited with status: {}", status));
    }

    // Step 2: Apply JARVIS audio effects with ffmpeg
    let ffmpeg_status = Command::new("ffmpeg")
        .arg("-y")
        .arg("-i")
        .arg(&raw_path)
        .arg("-af")
        .arg(JARVIS_FILTER)
        .arg(&processed_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("Failed to run ffmpeg: {}", e))?;

    // Clean up raw file immediately
    let _ = std::fs::remove_file(&raw_path);

    if !ffmpeg_status.success() {
        log::warn!("ffmpeg processing failed, falling back to direct say");
        return speak_direct(text, voice);
    }

    // Step 3: Play processed audio with afplay
    let child = Command::new("afplay")
        .arg(&processed_path)
        .spawn()
        .map_err(|e| format!("Failed to spawn afplay: {}", e))?;

    *CURRENT_CHILD.lock().unwrap() = Some(child);

    if let Some(mut child_to_wait) = CURRENT_CHILD.lock().unwrap().take() {
        child_to_wait
            .wait()
            .map_err(|e| format!("afplay process error: {}", e))?;
    }

    // Clean up processed file
    let _ = std::fs::remove_file(&processed_path);

    Ok(())
}

// ── Stop ─────────────────────────────────────────────────────────────────────

/// Kill the currently-running child process (say, afplay, ffmpeg, or edge-tts).
fn stop_child_process() {
    if let Some(mut child) = CURRENT_CHILD.lock().unwrap().take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Stop current speech immediately.
/// Called when the user interrupts or a new response arrives.
pub fn stop() -> Result<(), String> {
    stop_child_process();

    // Safety net: kill any stray `say`, `afplay`, or `edge-tts` processes
    let _ = Command::new("killall").arg("say").output();
    let _ = Command::new("killall").arg("afplay").output();
    let _ = Command::new("killall").arg("edge-tts").output();

    // Resume mic immediately so the user can speak again
    capture::resume_mic();
    capture::notify_tts_end();

    Ok(())
}
