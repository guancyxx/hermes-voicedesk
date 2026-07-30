/// TTS playback via macOS `say` command with JARVIS-like voice effects.
///
/// Voice selection:
/// - Default voice: `Daniel` (en_GB) — British male, closest to Paul Bettany's JARVIS.
/// - Configurable via `set_voice()` / `set_jarvis_mode()`.
///
/// JARVIS audio post-processing (via ffmpeg):
///   say -v <voice> -o raw.aiff  →  ffmpeg filters  →  afplay processed.aiff
/// Filters: subtle pitch shift (-3%), dual-delay reverb for room presence,
///          light EQ for clarity, compression for consistent volume.
///
/// Queue management:
/// - Only ONE utterance runs at a time. Starting new speech kills the previous one.
/// - A generation counter ensures stale TTS completion callbacks don't reset state.
///
/// Echo prevention:
/// - Mic is suspended BEFORE TTS begins, resumed after cooldown when TTS finishes.
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use tauri::{AppHandle, Emitter};

use crate::audio::capture;

/// Monotonically increasing generation counter. Incremented on every `speak()` call.
static TTS_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Handle to the currently-running child process so we can kill it before starting a new one.
static CURRENT_CHILD: Mutex<Option<Child>> = Mutex::new(None);

/// JARVIS mode: when enabled, audio goes through ffmpeg post-processing.
static JARVIS_MODE: AtomicBool = AtomicBool::new(false);

/// Current macOS voice name (default: Daniel — British male, closest to JARVIS).
static VOICE_NAME: Mutex<String> = Mutex::new(String::new());

/// Get the configured voice name.
fn get_voice() -> String {
    let voice = VOICE_NAME.lock().unwrap();
    if voice.is_empty() {
        "Daniel".to_string()
    } else {
        voice.clone()
    }
}

/// JARVIS ffmpeg audio filter chain.
///
/// Effect breakdown:
/// - asetrate+atempo: pitch shift down ~3% without changing speed
/// - aecho x 2: dual-tap reverb simulating medium room reflections
/// - highpass: remove sub-bass rumble
/// - treble: slight clarity boost in presence range
/// - compand: smooth dynamic range for broadcast-consistent levels
const JARVIS_FILTER: &str = concat!(
    "asetrate=44100*0.97,atempo=1/0.97,",
    "aecho=0.8:0.7:30:0.25,aecho=0.8:0.7:60:0.15,",
    "highpass=f=120,",
    "treble=g=2:f=4000,",
    "compand=attacks=0.005:decays=0.1:points=-80/-80|-30/-12|0/-3:gain=2"
);

/// Check whether ffmpeg is available on the system.
fn has_ffmpeg() -> bool {
    Command::new("which")
        .arg("ffmpeg")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Enable or disable JARVIS-mode audio post-processing.
pub fn set_jarvis_mode(enabled: bool) {
    JARVIS_MODE.store(enabled, Ordering::SeqCst);
}

/// Get current JARVIS mode.
pub fn get_jarvis_mode() -> bool {
    JARVIS_MODE.load(Ordering::SeqCst)
}

/// Set the macOS voice to use for TTS (e.g., "Daniel", "Samantha").
pub fn set_voice(name: &str) {
    *VOICE_NAME.lock().unwrap() = name.to_string();
}

/// Get current voice name.
pub fn get_voice_name() -> String {
    get_voice()
}

/// Speak text using macOS `say` command with optional JARVIS post-processing.
///
/// Pipeline:
/// 1. Kill any in-progress utterance
/// 2. Bump generation counter
/// 3. Suspend mic
/// 4. Spawn background thread:
///    a. `say -v <voice> -o raw.aiff <text>`
///    b. If JARVIS mode + ffmpeg available: `ffmpeg -i raw.aiff -af <filter> processed.aiff`
///    c. Play: `afplay <file>`
///    d. Cleanup temp files, resume mic, emit events
///
/// When TTS finishes, emits `audio:state {state: "idle"}` so the frontend
/// transitions to idle — user must explicitly click Listen again.
pub fn speak(text: &str, app: AppHandle) -> Result<(), String> {
    // 1. Kill previous utterance
    stop_child_process();

    // 2. Bump generation counter — invalidates pending callbacks
    let gen = TTS_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;

    // 3. Signal TTS-start and suspend mic
    capture::notify_tts_start();
    capture::suspend_mic();

    let text_owned = text.to_string();
    let voice = get_voice();
    let use_jarvis = JARVIS_MODE.load(Ordering::SeqCst) && has_ffmpeg();

    std::thread::spawn(move || {
        let result = if use_jarvis {
            speak_with_jarvis(&text_owned, &voice)
        } else {
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

/// JARVIS pipeline: say → raw aiff → ffmpeg → processed aiff → afplay.
fn speak_with_jarvis(text: &str, voice: &str) -> Result<(), String> {
    // Use a temp file path unique to this process (PID-based)
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
        .arg("-y") // Overwrite output
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
        // Fall back to playing raw file if ffmpeg fails
        log::warn!("ffmpeg processing failed, playing raw audio");
        // Raw file is already deleted, just use say directly
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

/// Kill the currently-running child process (say, afplay, or ffmpeg).
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

    // Safety net: kill any stray `say` or `afplay` processes
    let _ = Command::new("killall").arg("say").output();
    let _ = Command::new("killall").arg("afplay").output();

    // Resume mic immediately so the user can speak again
    capture::resume_mic();
    capture::notify_tts_end();

    Ok(())
}
