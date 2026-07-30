mod audio;
mod stt;
mod api;
mod session;

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager,
};

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
fn speak_text(text: String) -> Result<(), String> {
    audio::player::speak(&text)
}

#[tauri::command]
fn stop_speaking() -> Result<(), String> {
    audio::player::stop()
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
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
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
            speak_text,
            stop_speaking,
            transcribe_audio,
            transcribe_audio_native,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Hermes VoiceDesk");
}

fn setup_global_shortcut(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

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
