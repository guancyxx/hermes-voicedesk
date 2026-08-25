mod api;
mod audio;
mod session;
mod stt;

use std::sync::Mutex;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager,
};

/// Embedded JARVIS persona system prompt
const JARVIS_PERSONA: &str = include_str!("../resources/jarvis-persona.md");

/// Tracks whether JARVIS persona is active
static JARVIS_ACTIVE: Mutex<bool> = Mutex::new(false);

#[tauri::command]
async fn hermes_chat(message: String, session_id: Option<String>) -> Result<String, String> {
    api::hermes::chat(&message, session_id.as_deref()).await
}

#[tauri::command]
async fn hermes_chat_stream(
    message: String,
    session_id: Option<String>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    api::hermes::chat_stream(&message, session_id.as_deref(), app).await
}

#[tauri::command]
async fn check_hermes_api() -> Result<bool, String> {
    api::hermes::check_health().await
}

#[tauri::command]
async fn start_listening(app: tauri::AppHandle) -> Result<(), String> {
    audio::capture::start_mic_capture(app).await
}

#[tauri::command]
async fn stop_listening() -> Result<(), String> {
    audio::capture::stop_mic_capture().await
}

#[tauri::command]
fn save_chat_history(session_id: String, user_text: String, ai_text: String) -> Result<(), String> {
    let turn = session::store::new_turn(&session_id, &user_text, &ai_text);
    session::store::save_turn(&turn)
}

#[tauri::command]
fn load_chat_history(session_id: String) -> Result<Vec<session::store::ConversationTurn>, String> {
    session::store::load_history(&session_id)
}

#[tauri::command]
fn speak_text(text: String, app: tauri::AppHandle) -> Result<(), String> {
    audio::player::speak(&text, app)
}

/// Speak multiple sentences in parallel (generate all at once, play sequentially).
/// Called when the full response is ready for TTS playback.
#[tauri::command]
fn speak_batch(texts: Vec<String>, app: tauri::AppHandle) -> Result<(), String> {
    audio::player::speak_batch(texts, app)
}

#[tauri::command]
fn speak_batch_queued(
    texts: Vec<String>,
    final_segment: bool,
    app: tauri::AppHandle,
) -> Result<(), String> {
    audio::player::speak_batch_queued(texts, final_segment, app)
}

#[tauri::command]
fn reset_tts_queue() -> Result<(), String> {
    audio::player::reset_tts_queue()
}

#[tauri::command]
fn set_barge_in_enabled(enabled: bool) -> Result<(), String> {
    audio::capture::set_barge_in_enabled(enabled);
    Ok(())
}

#[tauri::command]
fn stop_speaking() -> Result<(), String> {
    audio::player::stop()
}

#[tauri::command]
fn set_jarvis_mode(enabled: bool) -> Result<(), String> {
    audio::player::set_jarvis_mode(enabled);
    Ok(())
}

#[tauri::command]
fn get_jarvis_mode() -> Result<bool, String> {
    Ok(audio::player::get_jarvis_mode())
}

#[tauri::command]
fn set_voice(voice: String) -> Result<(), String> {
    audio::player::set_voice(&voice);
    Ok(())
}

#[tauri::command]
fn get_voice() -> Result<String, String> {
    Ok(audio::player::get_voice_name())
}

/// Load the JARVIS persona system prompt.
/// Returns the full persona text. When `activate` is true, sends the persona
/// to Hermes Agent as a system message to set up the JARVIS personality.
#[tauri::command]
async fn load_jarvis_persona(activate: bool) -> Result<String, String> {
    let persona = JARVIS_PERSONA.to_string();

    if activate {
        // Set JARVIS as active
        *JARVIS_ACTIVE.lock().unwrap() = true;
        // Enable JARVIS voice mode
        audio::player::set_jarvis_mode(true);

        // Send the persona as a system prompt to Hermes
        let setup_message = format!(
            "[SYSTEM INSTRUCTION - JARVIS PERSONA ACTIVATED]\n\n{}\n\n---\n从现在开始，你将以 JARVIS 的身份和语气回复。确认激活。",
            persona
        );

        match api::hermes::chat(&setup_message, None).await {
            Ok(response) => {
                log::info!(
                    "JARVIS persona activated: {}",
                    &response[..response.len().min(120)]
                );
            }
            Err(e) => {
                log::warn!("Failed to activate JARVIS persona on Hermes API: {}", e);
                // Still return the persona, frontend can retry
            }
        }
    }

    Ok(persona)
}

/// Check if JARVIS persona is currently active.
#[tauri::command]
fn is_jarvis_persona_active() -> Result<bool, String> {
    Ok(*JARVIS_ACTIVE.lock().unwrap())
}

/// Deactivate JARVIS persona and reset to default mode.
#[tauri::command]
async fn deactivate_jarvis_persona() -> Result<(), String> {
    *JARVIS_ACTIVE.lock().unwrap() = false;
    audio::player::set_jarvis_mode(false);

    // Send a reset message to Hermes
    let reset_msg = "[SYSTEM INSTRUCTION - JARVIS PERSONA DEACTIVATED]\n\n请恢复到默认的 Hermes Agent 人格。确认解除 JARVIS 模式。";
    match api::hermes::chat(reset_msg, None).await {
        Ok(_) => log::info!("JARVIS persona deactivated"),
        Err(e) => log::warn!("Failed to deactivate JARVIS persona on Hermes API: {}", e),
    }

    Ok(())
}

#[tauri::command]
fn get_available_voices() -> Result<Vec<String>, String> {
    use std::process::Command;
    let output = Command::new("say")
        .arg("-v")
        .arg("?")
        .output()
        .map_err(|e| format!("Failed to list voices: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let voices: Vec<String> = stdout
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                Some(parts[0].to_string())
            } else {
                None
            }
        })
        .collect();

    Ok(voices)
}

#[tauri::command]
fn notify_tts_start() {
    audio::capture::notify_tts_start();
}

#[tauri::command]
fn notify_tts_end() {
    audio::capture::notify_tts_end();
}

#[tauri::command]
fn start_wake_word(
    app: tauri::AppHandle,
    access_key: Option<String>,
    keyword: Option<String>,
) -> Result<(), String> {
    audio::wake::start_wake_word(app, access_key, keyword);
    Ok(())
}

#[tauri::command]
fn stop_wake_word() -> Result<(), String> {
    audio::wake::stop_wake_word();
    Ok(())
}

#[tauri::command]
fn get_wake_status() -> Result<bool, String> {
    Ok(audio::wake::is_wake_detected())
}

#[tauri::command]
fn transcribe_audio(path: String) -> Result<String, String> {
    stt::whisper::transcribe_file(&path, "base")
}

#[tauri::command]
fn transcribe_audio_native(path: String) -> Result<String, String> {
    stt::apple::transcribe_file(&path)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_fs::init())
        .setup(|app| {
            // Enable logging in all modes — needed for wake word debugging
            {
                let level = if cfg!(debug_assertions) {
                    log::LevelFilter::Debug
                } else {
                    log::LevelFilter::Info
                };
                app.handle()
                    .plugin(tauri_plugin_log::Builder::default().level(level).build())?;
            }

            // ---- Tray Icon ----
            let show = MenuItem::with_id(app, "show", "Show VoiceDesk", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;

            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("Hermes VoiceDesk")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app)?;

            // ---- Global Hotkey (requires Accessibility permission) ----
            // Only register if the plugin loads successfully.
            match setup_global_shortcut(app) {
                Ok(()) => log::info!("Global shortcut registered: Option+Space"),
                Err(e) => log::warn!("Global shortcut unavailable: {}", e),
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            hermes_chat,
            hermes_chat_stream,
            check_hermes_api,
            start_listening,
            stop_listening,
            save_chat_history,
            load_chat_history,
            speak_text,
            speak_batch,
            speak_batch_queued,
            reset_tts_queue,
            set_barge_in_enabled,
            stop_speaking,
            notify_tts_start,
            notify_tts_end,
            start_wake_word,
            stop_wake_word,
            get_wake_status,
            transcribe_audio,
            transcribe_audio_native,
            set_jarvis_mode,
            get_jarvis_mode,
            set_voice,
            get_voice,
            get_available_voices,
            load_jarvis_persona,
            is_jarvis_persona_active,
            deactivate_jarvis_persona,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Hermes VoiceDesk");
}

fn setup_global_shortcut(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    use tauri_plugin_global_shortcut::{
        Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState,
    };

    let shortcut = Shortcut::new(Some(Modifiers::ALT), Code::Space);
    let app_handle = app.handle().clone();

    app_handle.plugin(
        tauri_plugin_global_shortcut::Builder::new()
            .with_handler(move |app, _shortcut, event| {
                if event.state == ShortcutState::Pressed {
                    if let Some(window) = app.get_webview_window("main") {
                        match window.is_visible() {
                            Ok(true) => {
                                let _ = window.hide();
                            }
                            _ => {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                    }
                }
            })
            .build(),
    )?;

    app_handle.global_shortcut().register(shortcut)?;
    Ok(())
}
