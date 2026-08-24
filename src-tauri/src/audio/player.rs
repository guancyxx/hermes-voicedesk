/// TTS playback for Hermes VoiceDesk.
///
/// Voice providers (priority order):
/// 1. **Edge-TTS** (`en-GB-RyanNeural`) — Best JARVIS-like British AI butler voice.
///    Free Microsoft Edge TTS via `edge-tts` Python CLI. Requires internet.
/// 2. **macOS say + ffmpeg** — Offline fallback. `say -v Daniel` with JARVIS
///    audio post-processing via ffmpeg filters.
/// 3. **macOS say direct** — Last resort, no dependencies.
///
/// **Parallel generation**: All sentences are TTS-generated in parallel (each on
/// its own thread). Playback starts only after ALL sentences have finished
/// generating, then plays them back in order.
///
/// Echo prevention:
///   Mic is suspended BEFORE TTS begins, resumed after cooldown when TTS finishes.
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter};

use crate::audio::capture;

/// Monotonically increasing generation counter. Incremented on every `speak()` call.
static TTS_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Staged TTS segments waiting to be played in arrival order.
static TTS_QUEUE: Mutex<Vec<Vec<String>>> = Mutex::new(Vec::new());

/// Whether a queue worker currently owns the TTS start/end lifecycle.
static QUEUE_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Handle to the currently-running child process so we can kill it before starting a new one.
static CURRENT_CHILD: Mutex<Option<Child>> = Mutex::new(None);

/// JARVIS mode: when enabled, TTS goes through the best available JARVIS voice pipeline.
static JARVIS_MODE: AtomicBool = AtomicBool::new(true);

/// Current macOS voice name (default: Daniel — British male, JARVIS-like fallback).
static VOICE_NAME: Mutex<String> = Mutex::new(String::new());

/// Edge-TTS voice name for JARVIS mode (English).
static EDGE_TTS_VOICE: Mutex<String> = Mutex::new(String::new());

/// Edge-TTS voice name for Chinese content.
static EDGE_TTS_VOICE_ZH: Mutex<String> = Mutex::new(String::new());

/// Detect whether text is predominantly Chinese (CJK characters).
/// Returns true if >30% of alphanumeric characters are CJK.
fn is_chinese_text(text: &str) -> bool {
    let mut cjk = 0usize;
    let mut other = 0usize;
    for ch in text.chars() {
        if ch.is_whitespace() || ch.is_ascii_punctuation() {
            continue;
        }
        if ('\u{4E00}'..='\u{9FFF}').contains(&ch) {
            cjk += 1;
        } else if ch.is_alphanumeric() {
            other += 1;
        }
    }
    let total = cjk + other;
    if total == 0 {
        return false;
    }
    cjk as f64 / total as f64 > 0.3
}

/// Pick the appropriate edge-tts voice based on text language.
/// - Chinese text → zh-CN male voice (JARVIS-like: YunyangNeural or YunxiNeural)
/// - English/other → en-GB-RyanNeural (British butler)
fn pick_edge_tts_voice(text: &str) -> String {
    if is_chinese_text(text) {
        let voice = EDGE_TTS_VOICE_ZH.lock().unwrap();
        if !voice.is_empty() {
            return voice.clone();
        }
        // zh-CN-YunyangNeural: professional, reliable male — most JARVIS-like for Chinese
        // zh-CN-YunxiNeural: lively, sunshine male — warmer alternative
        "zh-CN-YunyangNeural".to_string()
    } else {
        let voice = EDGE_TTS_VOICE.lock().unwrap();
        if !voice.is_empty() {
            return voice.clone();
        }
        "en-GB-RyanNeural".to_string()
    }
}

// ── Edge-TTS ─────────────────────────────────────────────────────────────────

/// Edge-TTS binary paths to try (in order).
const EDGE_TTS_PATHS: &[&str] = &[
    "edge-tts",                   // In PATH
    "/opt/homebrew/bin/edge-tts", // Homebrew
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

/// Pick the macOS `say` voice based on text language.
/// - Chinese text → Tingting (zh_CN female) or a male zh_CN voice if available
/// - English/other → the configured VOICE_NAME (default: Daniel)
fn get_voice_for_text(text: &str) -> String {
    if is_chinese_text(text) {
        // Try male Chinese voices first, fall back to Tingting
        // macOS 26+ has new voices like Reed, Rocko (zh_CN male)
        // Tingting is the classic always-available zh_CN voice
        "Tingting".to_string()
    } else {
        get_voice()
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

// ── Audio generation (produces a playable file, no playback) ────────────────

/// Result of generating audio for one sentence.
struct GeneratedClip {
    /// Path to the generated audio file
    path: String,
    /// True if the file should be deleted after playback
    is_temp: bool,
}

/// Generate audio file for a sentence using Edge-TTS.
/// Returns path to the generated .mp3 file.
fn generate_edgetts(text: &str, clip_id: usize) -> Result<GeneratedClip, String> {
    let output_path = format!("/tmp/hermes_tts_clip_{}.mp3", clip_id);
    let voice = pick_edge_tts_voice(text);
    let is_zh = is_chinese_text(text);
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

    // Chinese voices sound better at near-normal rate/pitch.
    // English uses the JARVIS butler adjustments (-5% rate, -3Hz pitch).
    let rate_arg = if is_zh { "+0%" } else { "-5%" };
    let pitch_arg = if is_zh { "+0Hz" } else { "-3Hz" };

    log::debug!(
        "Edge-TTS: voice={} rate={} pitch={} is_zh={} text=\"{}\"",
        voice,
        rate_arg,
        pitch_arg,
        is_zh,
        &text[..text.len().min(50)]
    );

    let status = command
        .arg("--voice")
        .arg(&voice)
        .arg(format!("--rate={}", rate_arg))
        .arg(format!("--pitch={}", pitch_arg))
        .arg("--text")
        .arg(text)
        .arg("--write-media")
        .arg(&output_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("Failed to run edge-tts: {}", e))?;

    if !status.success() {
        return Err(format!("Edge-TTS exited with status: {}", status));
    }

    Ok(GeneratedClip {
        path: output_path,
        is_temp: true,
    })
}

/// Generate audio file using macOS say + ffmpeg JARVIS filter.
fn generate_jarvis_ffmpeg(
    text: &str,
    voice: &str,
    clip_id: usize,
) -> Result<GeneratedClip, String> {
    let raw_path = format!("/tmp/hermes_tts_raw_{}.aiff", clip_id);
    let processed_path = format!("/tmp/hermes_tts_jarvis_{}.aiff", clip_id);

    // Step 1: Generate raw audio with `say`
    let status = Command::new("say")
        .arg("-v")
        .arg(voice)
        .arg("-o")
        .arg(&raw_path)
        .arg(text)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
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

    // Clean up raw file
    let _ = std::fs::remove_file(&raw_path);

    if !ffmpeg_status.success() {
        // Fallback: use the raw file directly
        return Ok(GeneratedClip {
            path: raw_path,
            is_temp: true,
        });
    }

    Ok(GeneratedClip {
        path: processed_path,
        is_temp: true,
    })
}

/// Generate audio file using macOS say directly (no processing).
fn generate_say_direct(text: &str, voice: &str, clip_id: usize) -> Result<GeneratedClip, String> {
    let output_path = format!("/tmp/hermes_tts_say_{}.aiff", clip_id);

    let status = Command::new("say")
        .arg("-v")
        .arg(voice)
        .arg("-o")
        .arg(&output_path)
        .arg(text)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("Failed to run say: {}", e))?;

    if !status.success() {
        return Err(format!("say exited with status: {}", status));
    }

    Ok(GeneratedClip {
        path: output_path,
        is_temp: true,
    })
}

/// Generate audio for a single sentence using the best available pipeline.
/// Does NOT play — only produces the file.
fn generate_clip(text: &str, clip_id: usize) -> Result<GeneratedClip, String> {
    let use_jarvis = JARVIS_MODE.load(Ordering::SeqCst);

    if use_jarvis && has_edge_tts() {
        match generate_edgetts(text, clip_id) {
            Ok(clip) => return Ok(clip),
            Err(e) => {
                log::warn!(
                    "Edge-TTS generation failed for clip {}: {}, falling back",
                    clip_id,
                    e
                );
            }
        }
    }

    let voice = get_voice_for_text(text);

    if use_jarvis && has_ffmpeg() {
        match generate_jarvis_ffmpeg(text, &voice, clip_id) {
            Ok(clip) => return Ok(clip),
            Err(e) => {
                log::warn!(
                    "JARVIS ffmpeg generation failed for clip {}: {}, falling back to say",
                    clip_id,
                    e
                );
            }
        }
    }

    generate_say_direct(text, &voice, clip_id)
}

// ── Core speak pipeline (parallel generation, sequential playback) ──────────

/// Speak text using the best available TTS pipeline.
///
/// **Parallel mode**: When a single sentence is requested, generates and plays
/// normally. The parallel generation is orchestrated by the frontend which calls
/// `speak_text` for each sentence — but we now also support a batch mode.
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
        // Generate audio file
        let pid = std::process::id() as u64;
        let clip_id = (pid * 1000 + gen) as usize;

        let clip_result = {
            if use_jarvis && has_edge_tts() {
                match generate_edgetts(&text_owned, clip_id) {
                    Ok(c) => Ok(c),
                    Err(_) => {
                        let voice = get_voice_for_text(&text_owned);
                        if has_ffmpeg() {
                            generate_jarvis_ffmpeg(&text_owned, &voice, clip_id)
                        } else {
                            generate_say_direct(&text_owned, &voice, clip_id)
                        }
                    }
                }
            } else {
                let voice = get_voice_for_text(&text_owned);
                if use_jarvis && has_ffmpeg() {
                    generate_jarvis_ffmpeg(&text_owned, &voice, clip_id)
                } else {
                    generate_say_direct(&text_owned, &voice, clip_id)
                }
            }
        };

        let clip = match clip_result {
            Ok(c) => c,
            Err(e) => {
                log::error!("TTS generation error: {}", e);
                finalize_tts(&app, gen);
                return;
            }
        };

        // Play the generated audio file
        if let Err(e) = play_file(&clip.path) {
            log::error!("TTS playback error: {}", e);
        }

        if clip.is_temp {
            let _ = std::fs::remove_file(&clip.path);
        }

        finalize_tts(&app, gen);
    });

    Ok(())
}

/// Normalize and concatenate batch clips into one WAV file.
///
/// Returns the merged path plus every intermediate file that must be cleaned up.
fn concat_batch_clips(
    clips: &[&GeneratedClip],
    pid: u64,
    gen: u64,
) -> Result<(String, Vec<String>), String> {
    let prefix = format!("/tmp/hermes_tts_batch_{}_{}", pid, gen);
    let list_path = format!("{}_concat.txt", prefix);
    let merged_path = format!("{}_merged.wav", prefix);
    let mut temporary_paths = Vec::with_capacity(clips.len() + 2);

    for (idx, clip) in clips.iter().enumerate() {
        let normalized_path = format!("{}_norm_{}.wav", prefix, idx);
        temporary_paths.push(normalized_path.clone());

        let status = match Command::new("ffmpeg")
            .arg("-y")
            .arg("-i")
            .arg(&clip.path)
            .args(["-ar", "44100", "-ac", "2", "-c:a", "pcm_s16le"])
            .arg(&normalized_path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
        {
            Ok(status) => status,
            Err(e) => {
                cleanup_files(&temporary_paths);
                return Err(format!("failed to normalize clip {}: {}", idx, e));
            }
        };

        if !status.success() {
            cleanup_files(&temporary_paths);
            return Err(format!(
                "ffmpeg normalization failed for clip {} with status {}",
                idx, status
            ));
        }
    }

    let concat_list = temporary_paths
        .iter()
        .map(|path| format!("file '{}'\n", path.replace('\'', "'\\''")))
        .collect::<String>();
    if let Err(e) = std::fs::write(&list_path, concat_list) {
        cleanup_files(&temporary_paths);
        return Err(format!("failed to write concat list: {}", e));
    }
    temporary_paths.push(list_path.clone());
    temporary_paths.push(merged_path.clone());

    let status = match Command::new("ffmpeg")
        .args(["-y", "-f", "concat", "-safe", "0", "-i"])
        .arg(&list_path)
        .args(["-c", "copy"])
        .arg(&merged_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(status) => status,
        Err(e) => {
            cleanup_files(&temporary_paths);
            return Err(format!("failed to run ffmpeg concat: {}", e));
        }
    };

    if !status.success() {
        cleanup_files(&temporary_paths);
        return Err(format!("ffmpeg concat failed with status {}", status));
    }

    Ok((merged_path, temporary_paths))
}

fn cleanup_files(paths: &[String]) {
    for path in paths {
        let _ = std::fs::remove_file(path);
    }
}

/// Generate audio for multiple sentences in PARALLEL, then concatenate and play
/// them as one file. Falls back to sequential playback if concatenation fails.
pub fn speak_batch(texts: Vec<String>, app: AppHandle) -> Result<(), String> {
    if texts.is_empty() {
        return Ok(());
    }

    // Single sentence — just use regular speak()
    if texts.len() == 1 {
        return speak(&texts[0], app);
    }

    // 1. Kill previous utterance
    stop_child_process();

    // 2. Bump generation counter
    let gen = TTS_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;

    // 3. Signal TTS-start and suspend mic
    capture::notify_tts_start();
    capture::suspend_mic();

    std::thread::spawn(move || {
        run_segment(texts, gen);
        finalize_tts(&app, gen);
    });

    Ok(())
}

/// Generate and play one segment. Returns true when a newer generation aborted it.
fn run_segment(texts: Vec<String>, gen: u64) -> bool {
    let pid = std::process::id() as u64;
    let base_clip_id = (pid * 10000 + gen * 100) as usize;
    let use_jarvis = JARVIS_MODE.load(Ordering::SeqCst);
    let has_et = has_edge_tts();
    let has_ff = has_ffmpeg();
    let results: Arc<Mutex<Vec<Result<GeneratedClip, String>>>> = Arc::new(Mutex::new(
        (0..texts.len())
            .map(|_| Err("pending".to_string()))
            .collect(),
    ));
    let mut handles = Vec::new();

    for (idx, text) in texts.iter().enumerate() {
        let results = results.clone();
        let text = text.clone();
        let clip_id = base_clip_id + idx;
        let voice = get_voice_for_text(&text);
        handles.push(std::thread::spawn(move || {
            let clip_result = if use_jarvis && has_et {
                match generate_edgetts(&text, clip_id) {
                    Ok(c) => Ok(c),
                    Err(_) if has_ff => generate_jarvis_ffmpeg(&text, &voice, clip_id),
                    Err(_) => generate_say_direct(&text, &voice, clip_id),
                }
            } else if use_jarvis && has_ff {
                generate_jarvis_ffmpeg(&text, &voice, clip_id)
            } else {
                generate_say_direct(&text, &voice, clip_id)
            };
            if let Ok(mut guard) = results.lock() {
                guard[idx] = clip_result;
            }
        }));
    }
    for handle in handles {
        let _ = handle.join();
    }

    log::info!("TTS batch: all {} clips generated in parallel", texts.len());
    let clips = results.lock().unwrap();
    let generated_clips: Vec<&GeneratedClip> = clips
        .iter()
        .enumerate()
        .filter_map(|(idx, result)| match result {
            Ok(clip) => Some(clip),
            Err(e) => {
                log::warn!("TTS generation failed for clip {}: {}", idx, e);
                None
            }
        })
        .collect();
    let cleanup_originals = || {
        for clip in &generated_clips {
            if clip.is_temp {
                let _ = std::fs::remove_file(&clip.path);
            }
        }
    };

    if TTS_GENERATION.load(Ordering::SeqCst) != gen {
        log::info!(
            "TTS batch: generation {} superseded, aborting playback",
            gen
        );
        cleanup_originals();
        return true;
    }
    if generated_clips.is_empty() {
        return false;
    }

    let mut played_merged = false;
    if has_ff {
        match concat_batch_clips(&generated_clips, pid, gen) {
            Ok((merged_path, temporary_paths)) => {
                if TTS_GENERATION.load(Ordering::SeqCst) != gen {
                    log::info!(
                        "TTS batch: generation {} superseded, aborting playback",
                        gen
                    );
                    cleanup_files(&temporary_paths);
                    cleanup_originals();
                    return true;
                }
                if let Err(e) = play_file(&merged_path) {
                    log::error!("TTS merged playback error: {}", e);
                } else {
                    played_merged = true;
                }
                cleanup_files(&temporary_paths);
            }
            Err(e) => log::warn!(
                "TTS batch concat failed, falling back to sequential playback: {}",
                e
            ),
        }
    } else {
        log::warn!(
            "TTS batch concat unavailable: ffmpeg not found; falling back to sequential playback"
        );
    }

    if !played_merged {
        for (idx, clip) in generated_clips.iter().enumerate() {
            if TTS_GENERATION.load(Ordering::SeqCst) != gen {
                log::info!(
                    "TTS batch: generation {} superseded, aborting playback",
                    gen
                );
                cleanup_originals();
                return true;
            }
            if let Err(e) = play_file(&clip.path) {
                log::error!("TTS playback error for clip {}: {}", idx, e);
            }
        }
    }
    cleanup_originals();
    false
}

/// Queue a staged batch without interrupting the segment currently playing.
pub fn speak_batch_queued(texts: Vec<String>, app: AppHandle) -> Result<(), String> {
    if texts.is_empty() {
        return Ok(());
    }

    let should_start = {
        let mut queue = TTS_QUEUE.lock().unwrap();
        queue.push(texts);
        !QUEUE_ACTIVE.swap(true, Ordering::SeqCst)
    };
    if !should_start {
        return Ok(());
    }

    let gen = TTS_GENERATION.load(Ordering::SeqCst);
    capture::notify_tts_start();
    capture::suspend_mic();
    std::thread::spawn(move || loop {
        let segment = {
            let mut queue = TTS_QUEUE.lock().unwrap();
            if TTS_GENERATION.load(Ordering::SeqCst) != gen || !QUEUE_ACTIVE.load(Ordering::SeqCst)
            {
                return;
            }
            if queue.is_empty() {
                QUEUE_ACTIVE.store(false, Ordering::SeqCst);
                None
            } else {
                Some(queue.remove(0))
            }
        };

        let Some(segment) = segment else {
            let _ = app.emit("tts:complete", serde_json::json!({}));
            let _ = app.emit("audio:state", serde_json::json!({"state": "idle"}));
            std::thread::sleep(std::time::Duration::from_millis(500));
            capture::resume_mic();
            capture::notify_tts_end();
            return;
        };

        if run_segment(segment, gen) {
            return;
        }
    });
    Ok(())
}

/// Clear all staged speech and invalidate the active queue worker.
pub fn reset_tts_queue() -> Result<(), String> {
    TTS_GENERATION.fetch_add(1, Ordering::SeqCst);
    TTS_QUEUE.lock().unwrap().clear();
    stop_child_process();

    capture::resume_mic();
    if QUEUE_ACTIVE.swap(false, Ordering::SeqCst) {
        capture::notify_tts_end();
    }
    Ok(())
}

/// Play an audio file, blocking until playback completes.
/// Uses the CURRENT_CHILD mechanism so playback can be interrupted.
fn play_file(path: &str) -> Result<(), String> {
    let child = Command::new("afplay")
        .arg(path)
        .spawn()
        .map_err(|e| format!("Failed to spawn afplay: {}", e))?;

    *CURRENT_CHILD.lock().unwrap() = Some(child);

    if let Some(mut child_to_wait) = CURRENT_CHILD.lock().unwrap().take() {
        child_to_wait
            .wait()
            .map_err(|e| format!("afplay process error: {}", e))?;
    }

    Ok(())
}

/// Finalize TTS: emit completion events and resume mic.
/// The notify_tts_end() counter decrement must happen on EVERY exit path —
/// including when this generation was superseded — otherwise TTS_PLAYING_COUNT
/// leaks upward and the mic stays suppressed forever. Only the user-facing
/// events (tts:complete / audio:state) are gated on the generation check.
fn finalize_tts(app: &AppHandle, gen: u64) {
    if TTS_GENERATION.load(Ordering::SeqCst) == gen {
        let _ = app.emit("tts:complete", serde_json::json!({}));
        let _ = app.emit("audio:state", serde_json::json!({"state": "idle"}));

        // Brief cooldown for residual echo before re-enabling mic
        std::thread::sleep(std::time::Duration::from_millis(500));

        capture::resume_mic();
    }
    capture::notify_tts_end();
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
    reset_tts_queue()?;

    // Safety net: kill any stray `say`, `afplay`, or `edge-tts` processes
    let _ = Command::new("killall").arg("say").output();
    let _ = Command::new("killall").arg("afplay").output();
    let _ = Command::new("killall").arg("edge-tts").output();

    Ok(())
}
