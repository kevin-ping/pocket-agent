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
    // Load .env file:
    // 1. Current working directory (works for `tauri dev`)
    // 2. ~/.pocket-agent/.env (works for packaged .app)
    let _ = dotenvy::dotenv();
    if let Ok(home) = std::env::var("HOME") {
        let env_path = std::path::Path::new(&home).join(".pocket-agent").join(".env");
        let _ = dotenvy::from_path(&env_path);
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::default().build())
        .manage(AppState {
            session_id: Mutex::new("pocket-agent".to_string()),
        })
        .manage(commands::voice::RecordingState::default())
        .setup(|app| {
            let handle = app.handle();

            let config = commands::config::load_config(handle);
            // Apply env config from saved settings (overrides .env file)
            if !config.api_server.is_empty() { std::env::set_var("API_SERVER", &config.api_server); }
            if !config.api_agent.is_empty() { std::env::set_var("API_AGENT", &config.api_agent); }
            if !config.api_server_key.is_empty() { std::env::set_var("API_SERVER_KEY", &config.api_server_key); }
            if !config.enable_local_commands.is_empty() { std::env::set_var("ENABLE_LOCAL_COMMANDS", &config.enable_local_commands); }
            if let (Some(x), Some(y)) = (config.window_x, config.window_y) {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.set_position(tauri::LogicalPosition::new(x, y));
                }
            }

            // Log session info at startup for debugging
            {
                let app_state: tauri::State<'_, AppState> = app.state::<AppState>();
                let sid = app_state.session_id.lock().unwrap();
                let today = chrono::Local::now().format("%Y-%m-%d");
                eprintln!("[session] using {}-{}", *sid, today);
            }

            voice::hotkey::check_accessibility(handle);
            voice::hotkey::spawn_hotkey_listener(handle.clone(), config.hotkey_code);
            voice::hotkey::prewarm_capture();
            voice::record::prewarm();

            // Start local API server for push notifications (port 8650)
            {
                let server_handle = handle.clone();
                tauri::async_runtime::spawn(async move {
                    crate::api::server::start_server(server_handle, 8650).await;
                });
            }
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
            commands::chat::speak_text,
            commands::config::get_config,
            commands::config::save_config,
            commands::config::quit_app,

            commands::voice::start_voice_recording,
            commands::voice::stop_voice_recording,
            commands::voice::cancel_voice_recording,
            voice::hotkey::capture_hotkey,
            voice::hotkey::update_hotkey,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
