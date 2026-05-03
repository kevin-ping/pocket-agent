mod api;
mod commands;
mod voice;

use std::sync::Mutex;
use tauri::{Emitter, Manager, menu::{MenuBuilder, MenuItemBuilder}, tray::TrayIconBuilder};

pub struct AppState {
    pub session_id: Mutex<String>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::default().build())
        .manage(AppState {
            session_id: Mutex::new("pocket-agent-session".to_string()),
        })
        .manage(commands::voice::RecordingState::default())
        .setup(|app| {
            let handle = app.handle();

            let config = commands::config::load_config(handle);
            if let (Some(x), Some(y)) = (config.window_x, config.window_y) {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.set_position(tauri::LogicalPosition::new(x, y));
                }
            }

            voice::hotkey::check_accessibility(handle);
            voice::hotkey::spawn_hotkey_listener(handle.clone());
            voice::record::prewarm();

            // System tray menu (appears in macOS menu bar)
            let settings_item = MenuItemBuilder::with_id("settings", "Settings...").build(app)?;
            let quit_item = MenuItemBuilder::with_id("quit", "Quit Pocket Agent").build(app)?;
            let _menu = MenuBuilder::new(app)
                .items(&[&settings_item, &quit_item])
                .build()?;

            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().cloned().unwrap())
                .menu(&_menu)
                .tooltip("Pocket Agent")
                .on_menu_event(move |app, event| {
                    match event.id().as_ref() {
                        "settings" => {
                            let _ = app.emit("tray-open-settings", ());
                        }
                        "quit" => {
                            app.exit(0);
                        }
                        _ => {}
                    }
                })
                .build(app)?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::chat::send_message,
            commands::chat::speak,
            commands::config::get_config,
            commands::config::save_config,
            commands::config::quit_app,
            commands::voice::start_voice_recording,
            commands::voice::stop_voice_recording,
            commands::voice::cancel_voice_recording,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
