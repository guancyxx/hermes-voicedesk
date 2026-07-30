/// TTS playback via macOS `say` command.
///
/// Queue management:
/// - Only ONE `say` process runs at a time. Starting new speech kills the previous one.
/// - A generation counter ensures stale TTS completion callbacks don't reset the state
///   after a newer utterance has started.
///
/// Echo prevention:
/// - `notify_tts_start()` is called BEFORE spawning `say` so the audio callback sees the
///   flag before the first sample hits the mic.
/// - `notify_tts_end()` is only called when the *current* generation finishes (not stale ones).
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::audio::capture;

/// Monotonically increasing generation counter. Incremented on every `speak()` call.
/// The spawned thread captures the generation it was started with; when `say` exits,
/// it only calls `notify_tts_end()` if its generation still matches (i.e. it hasn't
/// been superseded by a newer `speak()` call).
static TTS_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Handle to the currently-running `say` process so we can kill it before starting
/// a new one.
static CURRENT_SAY: Mutex<Option<Child>> = Mutex::new(None);

/// Speak text using macOS `say` command.
///
/// Kills any in-progress `say` before starting. Returns immediately;
/// the actual speech runs on a background thread.
pub fn speak(text: &str) -> Result<(), String> {
    // 1. Kill the previous `say` process if one is running
    stop_say_process();

    // 2. Bump the generation counter — this invalidates any pending TTS-end callbacks
    let gen = TTS_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;

    // 3. Signal TTS-start BEFORE spawning `say` so the audio callback
    //    sees the flag before sound exits the speakers
    capture::notify_tts_start();

    let text_owned = text.to_string();

    std::thread::spawn(move || {
        // Spawn `say` as a child process so we can kill it later
        let child = Command::new("say")
            .arg(&text_owned)
            .spawn();

        match child {
            Ok(c) => {
                // Store the handle so future `speak()` calls can kill it
                *CURRENT_SAY.lock().unwrap() = Some(c);

                // Re-acquire the child to wait on it (take it back from the static)
                if let Some(mut child_to_wait) = CURRENT_SAY.lock().unwrap().take() {
                    // Wait for `say` to finish
                    let _ = child_to_wait.wait();
                }
            }
            Err(e) => {
                log::error!("Failed to spawn say: {}", e);
            }
        }

        // Only notify TTS-end if this generation is still the current one.
        // If a newer `speak()` call bumped the generation, a newer TTS is playing
        // and we must NOT reset the flag.
        if TTS_GENERATION.load(Ordering::SeqCst) == gen {
            capture::notify_tts_end();
        }
    });

    Ok(())
}

/// Kill the currently-running `say` process (if any).
/// Called by `speak()` before starting new speech, and by the frontend's
/// `stop_speaking` command.
fn stop_say_process() {
    if let Some(mut child) = CURRENT_SAY.lock().unwrap().take() {
        let _ = child.kill();
        let _ = child.wait(); // reap the zombie
    }
}

/// Stop current speech immediately.
/// Called by the frontend when the user interrupts or a new response arrives.
pub fn stop() -> Result<(), String> {
    stop_say_process();

    // Also kill any stray `say` processes (safety net)
    let _ = Command::new("killall")
        .arg("say")
        .output();

    // Signal TTS ended (interrupted)
    capture::notify_tts_end();

    Ok(())
}
