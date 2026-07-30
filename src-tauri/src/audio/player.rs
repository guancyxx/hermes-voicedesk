/// TTS playback via macOS `say` command.
///
/// Queue management:
/// - Only ONE `say` process runs at a time. Starting new speech kills the previous one.
/// - A generation counter ensures stale TTS completion callbacks don't reset the state
///   after a newer utterance has started.
///
/// Echo prevention (BUG 2 fix):
/// - `capture::suspend_mic()` is called BEFORE spawning `say` so the mic callback
///   returns immediately without processing any samples. This completely prevents
///   TTS echo from being captured by the microphone.
/// - After TTS finishes, a cooldown delay is observed before `capture::resume_mic()`
///   is called, ensuring residual reverb/echo has subsided.
///
/// State management (BUG 1 fix):
/// - When TTS finishes, an `audio:state {state: "listening"}` event is emitted to the
///   frontend so it can transition from "speaking" back to "listening" at the right time
///   (instead of the previous hardcoded 1-second timeout).
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use tauri::{AppHandle, Emitter};

use crate::audio::capture;

/// Monotonically increasing generation counter. Incremented on every `speak()` call.
static TTS_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Handle to the currently-running `say` process so we can kill it before starting
/// a new one.
static CURRENT_SAY: Mutex<Option<Child>> = Mutex::new(None);

/// Speak text using macOS `say` command.
///
/// Kills any in-progress `say` before starting. Returns immediately;
/// the actual speech runs on a background thread.
///
/// When TTS finishes, emits `audio:state {state: "listening"}` so the frontend
/// can transition UI state at the right time.
pub fn speak(text: &str, app: AppHandle) -> Result<(), String> {
    // 1. Kill the previous `say` process if one is running
    stop_say_process();

    // 2. Bump the generation counter — invalidates any pending TTS-end callbacks
    let gen = TTS_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;

    // 3. Signal TTS-start and suspend mic BEFORE spawning `say`
    //    so the flags are set before sound exits the speakers
    capture::notify_tts_start();
    capture::suspend_mic();

    let text_owned = text.to_string();

    std::thread::spawn(move || {
        let child = Command::new("say")
            .arg(&text_owned)
            .spawn();

        match child {
            Ok(c) => {
                *CURRENT_SAY.lock().unwrap() = Some(c);
                if let Some(mut child_to_wait) = CURRENT_SAY.lock().unwrap().take() {
                    let _ = child_to_wait.wait();
                }
            }
            Err(e) => {
                log::error!("Failed to spawn say: {}", e);
            }
        }

        // Only finalize if this generation is still the current one.
        // If a newer `speak()` superseded us, we must NOT touch the flags.
        if TTS_GENERATION.load(Ordering::SeqCst) == gen {
            // BUG 1 fix: Tell frontend TTS actually finished (not a guess!)
            let _ = app.emit("audio:state", serde_json::json!({"state": "listening"}));

            // Brief cooldown for residual echo before re-enabling mic
            std::thread::sleep(std::time::Duration::from_millis(500));

            // BUG 2 fix: Resume mic after echo has dissipated
            capture::resume_mic();
            capture::notify_tts_end();
        }
    });

    Ok(())
}

/// Kill the currently-running `say` process (if any).
fn stop_say_process() {
    if let Some(mut child) = CURRENT_SAY.lock().unwrap().take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Stop current speech immediately.
/// Called when the user interrupts or a new response arrives.
pub fn stop() -> Result<(), String> {
    stop_say_process();

    // Also kill any stray `say` processes (safety net)
    let _ = Command::new("killall").arg("say").output();

    // Resume mic immediately so the user can speak again
    capture::resume_mic();
    capture::notify_tts_end();

    Ok(())
}
