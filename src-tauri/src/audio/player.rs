/// TTS playback for Hermes VoiceDesk.
///
/// Voice providers (priority order):
/// 1. **Edge-TTS** (`en-GB-RyanNeural`) — Best JARVIS-like British AI butler voice.
///    Free Microsoft Edge TTS via `edge-tts` Python CLI. Requires internet.
/// 2. **Qwen3-TTS** (mlx-audio subprocess, voice "Ryan") — offline fallback when
///    Edge-TTS fails. Requires `pip3 install mlx-audio` and a local model.
/// 3. **macOS say + ffmpeg** — Offline fallback. `say -v Daniel` with JARVIS
///    audio post-processing via ffmpeg filters.
/// 4. **macOS say direct** — Last resort, no dependencies.
///
/// **Parallel generation**: All sentences are TTS-generated in parallel (each on
/// its own thread). Playback starts only after ALL sentences have finished
/// generating, then plays them back in order.
///
/// Echo prevention:
///   Mic is suspended BEFORE TTS begins, resumed after cooldown when TTS finishes.
use std::cell::Cell;
use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use tauri::{AppHandle, Emitter};

use crate::audio::capture;

/// Monotonically increasing generation counter. Incremented on every `speak()` call.
static TTS_GENERATION: AtomicU64 = AtomicU64::new(0);

struct QueueState {
    segments: VecDeque<Vec<String>>,
    finished: bool,
    active: bool,
    round_id: u64,
}

static QUEUE: Mutex<Option<QueueState>> = Mutex::new(None);
static QUEUE_CV: Condvar = Condvar::new();

/// Handle to the currently-running child process so we can kill it before starting a new one.
static CURRENT_CHILD: Mutex<Option<Child>> = Mutex::new(None);

/// Generation epoch: bumped by every reset_tts_queue(). Generation threads
/// capture the epoch at start; a killed sub-process returning failure must NOT
/// fall back to spawning a NEW sub-process (say/ffmpeg) after a reset — that
/// would escape the kill snapshot and keep burning CPU/disk after the round
/// was cancelled.
static GENERATION_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Generation processes are tracked separately from the playback child.
static GENERATION_PIDS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

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

thread_local! {
    /// Epoch captured by the generation thread BEFORE it starts spawning
    /// sub-processes. run_tracked re-checks it after spawn: if a reset
    /// happened in the window between the caller's epoch check and the
    /// child being registered in GENERATION_PIDS (i.e. it escaped the kill
    /// snapshot), we kill it ourselves immediately.
    static GEN_EPOCH_AT_SPAWN: Cell<u64> = const { Cell::new(0) };
}

pub fn set_generation_epoch(epoch: u64) {
    GEN_EPOCH_AT_SPAWN.with(|e| e.set(epoch));
}

fn run_tracked(cmd: &mut Command) -> io::Result<ExitStatus> {
    let epoch_at_spawn = GEN_EPOCH_AT_SPAWN.with(|e| e.get());
    let mut child = cmd.spawn()?;
    let pid = child.id();
    // Register BEFORE checking the epoch: registration and the kill snapshot
    // in reset_tts_queue both take GENERATION_PIDS' lock, so once we are in
    // the table either the kill sees us (and TERMs the pid) or it happened
    // before us and the epoch check below kills the child here. Either way
    // the process cannot escape.
    GENERATION_PIDS.lock().unwrap().push(pid);
    if GENERATION_EPOCH.load(Ordering::SeqCst) != epoch_at_spawn {
        let _ = child.kill();
        let _ = child.wait();
        // Note: leave the pid in GENERATION_PIDS — if kill() itself failed
        // (process already reaped etc.) a later reset snapshot can still TERM it.
        return Err(io::Error::new(io::ErrorKind::Other, "cancelled by reset"));
    }
    let result = child.wait();
    GENERATION_PIDS
        .lock()
        .unwrap()
        .retain(|tracked| *tracked != pid);
    result
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

    let status = run_tracked(
        command
            .arg("--voice")
            .arg(&voice)
            .arg(format!("--rate={}", rate_arg))
            .arg(format!("--pitch={}", pitch_arg))
            .arg("--text")
            .arg(text)
            .arg("--write-media")
            .arg(&output_path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null()),
    )
    .map_err(|e| {
        let _ = std::fs::remove_file(&output_path);
        format!("Failed to run edge-tts: {}", e)
    })?;

    if !status.success() {
        // Clean up any partial output — Err carries no GeneratedClip, so
        // cleanup_originals in run_segment cannot see this file.
        let _ = std::fs::remove_file(&output_path);
        return Err(format!("Edge-TTS exited with status: {}", status));
    }

    Ok(GeneratedClip {
        path: output_path,
        is_temp: true,
    })
}

/// Locate qwen3_tts.py: next to executable (bundle Resources/scripts), then
/// project tree (src-tauri/scripts/).
fn find_qwen3_script() -> Option<std::path::PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("scripts").join("qwen3_tts.py");
            if sibling.exists() {
                return Some(sibling);
            }
            // Tauri resources land directly in Contents/Resources/
            let sibling2 = dir.join("qwen3_tts.py");
            if sibling2.exists() {
                return Some(sibling2);
            }
            // Packaged .app: exe is in Contents/MacOS, tauri keeps the
            // resource's relative path under Contents/Resources/
            let sibling3 = dir.join("../Resources/scripts/qwen3_tts.py");
            if sibling3.exists() {
                return Some(sibling3);
            }
        }
    }
    let mut current = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    loop {
        let candidate = current
            .join("src-tauri")
            .join("scripts")
            .join("qwen3_tts.py");
        if candidate.exists() {
            return Some(candidate);
        }
        if !current.pop() {
            break;
        }
    }
    None
}

/// Pick the Python interpreter for qwen3_tts.py: explicit override, then the
/// dedicated venv (mlx-audio is not on the system python), then plain python3.
fn qwen3_python() -> String {
    if let Ok(p) = std::env::var("VOICEDESK_TTS_PYTHON") {
        return p;
    }
    if let Ok(home) = std::env::var("HOME") {
        let venv = format!("{}/.venvs/voicedesk-tts/bin/python", home);
        if std::path::Path::new(&venv).exists() {
            return venv;
        }
    }
    "python3".to_string()
}

/// Generate audio offline with Qwen3-TTS (mlx-audio Python subprocess).
/// British male voice ("Ryan" — per mlx-audio README, English speakers are
/// Ryan / Aiden). Exit code 3 = mlx_audio not installed → caller falls back
/// to the say/ffmpeg chain.
fn generate_qwen3_tts(text: &str, clip_id: usize) -> Result<GeneratedClip, String> {
    let script = find_qwen3_script().ok_or_else(|| "qwen3_tts.py script not found".to_string())?;
    let output_path = format!("/tmp/hermes_tts_qwen3_{}.wav", clip_id);

    let status = run_tracked(
        Command::new(qwen3_python())
            .arg(script)
            .arg("--text")
            .arg(text)
            .arg("--out")
            .arg(&output_path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped()),
    )
    .map_err(|e| format!("Failed to run python3: {}", e))?;

    if !status.success() {
        let _ = std::fs::remove_file(&output_path);
        if status.code() == Some(3) {
            return Err("mlx-audio not installed (pip3 install mlx-audio)".to_string());
        }
        return Err(format!("qwen3_tts.py exited with status: {}", status));
    }

    if !std::path::Path::new(&output_path).exists() {
        return Err("qwen3_tts.py produced no output file".to_string());
    }

    // Tag the clip so the frontend/logs know which engine produced it.
    log::warn!(
        "TTS engine: qwen3-tts (mlx-audio fallback) produced clip {}",
        clip_id
    );
    LAST_TTS_ENGINE.store(2, Ordering::SeqCst);

    Ok(GeneratedClip {
        path: output_path,
        is_temp: true,
    })
}

/// Last TTS engine used for the most recent utterance:
/// 0 = edge-tts, 1 = say/ffmpeg, 2 = qwen3-tts.
static LAST_TTS_ENGINE: AtomicI32 = AtomicI32::new(0);

/// Generate audio file using macOS say + ffmpeg JARVIS filter.
fn generate_jarvis_ffmpeg(
    text: &str,
    voice: &str,
    clip_id: usize,
) -> Result<GeneratedClip, String> {
    let raw_path = format!("/tmp/hermes_tts_raw_{}.aiff", clip_id);
    let processed_path = format!("/tmp/hermes_tts_jarvis_{}.aiff", clip_id);

    // Step 1: Generate raw audio with `say`
    let status = run_tracked(
        Command::new("say")
            .arg("-v")
            .arg(voice)
            .arg("-o")
            .arg(&raw_path)
            .arg(text)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null()),
    )
    .map_err(|e| {
        let _ = std::fs::remove_file(&raw_path);
        format!("Failed to run say: {}", e)
    })?;

    if !status.success() {
        let _ = std::fs::remove_file(&raw_path);
        return Err(format!("say exited with status: {}", status));
    }

    // Step 2: Apply JARVIS audio effects with ffmpeg
    let ffmpeg_status = run_tracked(
        Command::new("ffmpeg")
            .arg("-y")
            .arg("-i")
            .arg(&raw_path)
            .arg("-af")
            .arg(JARVIS_FILTER)
            .arg(&processed_path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null()),
    )
    .map_err(|e| {
        let _ = std::fs::remove_file(&raw_path);
        let _ = std::fs::remove_file(&processed_path);
        format!("Failed to run ffmpeg: {}", e)
    })?;

    if !ffmpeg_status.success() {
        // Fallback: use the raw file directly. Keep raw_path (it IS the clip
        // we return) but drop any partial processed output.
        let _ = std::fs::remove_file(&processed_path);
        return Ok(GeneratedClip {
            path: raw_path,
            is_temp: true,
        });
    }

    // Clean up raw file — only after the processed output is confirmed good.
    let _ = std::fs::remove_file(&raw_path);

    Ok(GeneratedClip {
        path: processed_path,
        is_temp: true,
    })
}

/// Generate audio file using macOS say directly (no processing).
fn generate_say_direct(text: &str, voice: &str, clip_id: usize) -> Result<GeneratedClip, String> {
    let output_path = format!("/tmp/hermes_tts_say_{}.aiff", clip_id);

    let status = run_tracked(
        Command::new("say")
            .arg("-v")
            .arg(voice)
            .arg("-o")
            .arg(&output_path)
            .arg(text)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null()),
    )
    .map_err(|e| {
        let _ = std::fs::remove_file(&output_path);
        format!("Failed to run say: {}", e)
    })?;

    if !status.success() {
        let _ = std::fs::remove_file(&output_path);
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
                    "Edge-TTS generation failed for clip {}: {}, falling back to Qwen3-TTS",
                    clip_id,
                    e
                );
                // Offline fallback #1: Qwen3-TTS via mlx-audio subprocess
                match generate_qwen3_tts(text, clip_id) {
                    Ok(clip) => return Ok(clip),
                    Err(qe) => {
                        log::warn!(
                            "Qwen3-TTS fallback failed for clip {}: {}, falling back to say",
                            clip_id,
                            qe
                        );
                    }
                }
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
    // Legacy and staged playback are mutually exclusive.
    if queue_is_active() {
        reset_tts_queue()?;
    }
    // 1. Kill previous utterance
    stop_child_process();

    // 2. Bump generation counter — invalidates pending callbacks
    let gen = TTS_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;

    // 3. Signal TTS-start and suspend mic
    capture::notify_tts_start();
    capture::suspend_mic();
    LAST_TTS_ENGINE.store(0, Ordering::SeqCst);

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
                    Err(edge_err) => {
                        log::warn!(
                            "Edge-TTS failed for clip {}: {} — trying Qwen3-TTS fallback",
                            clip_id,
                            edge_err
                        );
                        // Offline fallback #1: Qwen3-TTS via mlx-audio subprocess
                        match generate_qwen3_tts(&text_owned, clip_id) {
                            Ok(c) => Ok(c),
                            Err(qe) => {
                                log::warn!(
                                    "Qwen3-TTS fallback failed for clip {}: {} — falling back to say",
                                    clip_id,
                                    qe
                                );
                                let voice = get_voice_for_text(&text_owned);
                                if has_ffmpeg() {
                                    generate_jarvis_ffmpeg(&text_owned, &voice, clip_id)
                                } else {
                                    generate_say_direct(&text_owned, &voice, clip_id)
                                }
                            }
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
    // Legacy and staged playback are mutually exclusive.
    if queue_is_active() {
        reset_tts_queue()?;
    }

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
    LAST_TTS_ENGINE.store(0, Ordering::SeqCst);

    std::thread::spawn(move || {
        run_segment(texts, gen, &|| TTS_GENERATION.load(Ordering::SeqCst) == gen);
        finalize_tts(&app, gen);
    });

    Ok(())
}

/// Generate and play one segment. Returns true when a newer generation aborted it.
fn run_segment(texts: Vec<String>, gen: u64, is_current: &dyn Fn() -> bool) -> bool {
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
        let gen_epoch = GENERATION_EPOCH.load(Ordering::SeqCst);
        handles.push(std::thread::spawn(move || {
            set_generation_epoch(gen_epoch);
            let clip_result = if use_jarvis && has_et {
                match generate_edgetts(&text, clip_id) {
                    Ok(c) => Ok(c),
                    // Cancel barrier: if a reset killed this sub-process, do NOT
                    // spawn a fallback — the round is gone.
                    Err(e) if GENERATION_EPOCH.load(Ordering::SeqCst) != gen_epoch => {
                        Err(format!("cancelled by reset: {}", e))
                    }
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

    if !is_current() {
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
                if !is_current() {
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
            if !is_current() {
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

fn queue_is_active() -> bool {
    QUEUE
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|state| state.active)
}

struct QueueWorkerGuard {
    round_id: u64,
}

impl Drop for QueueWorkerGuard {
    fn drop(&mut self) {
        let mut queue = QUEUE.lock().unwrap();
        if let Some(state) = queue.as_mut() {
            if state.active && state.round_id == self.round_id {
                state.active = false;
                capture::notify_tts_end();
            }
        }
    }
}

/// Atomically enqueue a staged segment and optionally finish the streaming round.
pub fn speak_batch_queued(
    texts: Vec<String>,
    final_segment: bool,
    app: AppHandle,
) -> Result<(), String> {
    let (should_start, round_id) = {
        let mut queue = QUEUE.lock().unwrap();
        let state = queue.get_or_insert_with(|| QueueState {
            segments: VecDeque::new(),
            finished: false,
            active: false,
            round_id: 0,
        });
        let has_texts = !texts.is_empty();
        if has_texts {
            state.segments.push_back(texts);
        }
        // A new conversation round starts here: if a previous reset set
        // finished=true (to retire the old worker), the first enqueue of the
        // new round must clear it — otherwise the new worker sees an empty
        // queue + finished and immediately emits a spurious tts:complete.
        // Setting finished=false on a NON-empty enqueue is always safe: it can
        // only defer the finish until the real final_segment arrives.
        if has_texts {
            state.finished = false;
        }
        if final_segment {
            state.finished = true;
        }
        let should_start = !state.active;
        if should_start {
            state.active = true;
        }
        (should_start, state.round_id)
    };

    if should_start {
        capture::notify_tts_start();
        capture::suspend_mic();
        std::thread::spawn(move || queue_worker(app, round_id));
    }
    QUEUE_CV.notify_all();
    Ok(())
}

fn queue_worker(app: AppHandle, round_id: u64) {
    let _guard = QueueWorkerGuard { round_id };
    loop {
        let segment = {
            let mut queue = QUEUE.lock().unwrap();
            loop {
                let state = queue.as_mut().unwrap();
                if !state.active || state.round_id != round_id {
                    return;
                }
                if let Some(segment) = state.segments.pop_front() {
                    break Some(segment);
                }
                if state.finished {
                    break None;
                }
                queue = QUEUE_CV
                    .wait_timeout(queue, std::time::Duration::from_millis(200))
                    .unwrap()
                    .0;
            }
        };

        if let Some(segment) = segment {
            if run_segment(segment, round_id, &|| {
                QUEUE
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|state| state.active && state.round_id == round_id)
            }) {
                return;
            }
            continue;
        }

        let current = QUEUE.lock().unwrap().as_ref().is_some_and(|state| {
            state.active
                && state.round_id == round_id
                && state.finished
                && state.segments.is_empty()
        });
        if !current {
            return;
        }
        let _ = app.emit("tts:complete", serde_json::json!({}));
        let _ = app.emit("audio:state", serde_json::json!({"state": "idle"}));
        std::thread::sleep(std::time::Duration::from_millis(500));
        // Re-validate after the cooldown: if a reset (or a new round) started
        // during the sleep, this worker must NOT touch the mic — the new
        // round owns it now.
        let still_current = QUEUE
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|state| state.active && state.round_id == round_id);
        if still_current {
            capture::resume_mic();
        }
        let mut queue = QUEUE.lock().unwrap();
        if let Some(state) = queue.as_mut() {
            if state.active && state.round_id == round_id {
                state.active = false;
                capture::notify_tts_end();
            }
        }
        return;
    }
}

/// Clear all staged speech and invalidate the active queue worker.
pub fn reset_tts_queue() -> Result<(), String> {
    let was_active = {
        let mut queue = QUEUE.lock().unwrap();
        let state = queue.get_or_insert_with(|| QueueState {
            segments: VecDeque::new(),
            finished: false,
            active: false,
            round_id: 0,
        });
        let was_active = state.active;
        state.round_id = state.round_id.wrapping_add(1);
        state.segments.clear();
        state.finished = true;
        state.active = false;
        was_active
    };
    QUEUE_CV.notify_all();
    stop_child_process();
    // Cancel barrier FIRST: bump the epoch so in-flight generation threads
    // that get killed here do not fall back to spawning new sub-processes,
    // then kill the tracked ones.
    GENERATION_EPOCH.fetch_add(1, Ordering::SeqCst);
    kill_generation_processes();

    capture::resume_mic();
    if was_active {
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
        let engine = match LAST_TTS_ENGINE.load(Ordering::SeqCst) {
            2 => "qwen3-tts",
            1 => "say-ffmpeg",
            _ => "edge-tts",
        };
        let _ = app.emit("tts:complete", serde_json::json!({ "engine": engine }));
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

fn kill_generation_processes() {
    let pids = GENERATION_PIDS.lock().unwrap().clone();
    for pid in pids {
        let _ = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status();
    }
}

/// Stop current speech immediately.
/// Called when the user interrupts or a new response arrives.
pub fn stop() -> Result<(), String> {
    // `stop` also cancels legacy speak/speak_batch generations; queue resets do not.
    TTS_GENERATION.fetch_add(1, Ordering::SeqCst);
    reset_tts_queue()
}
