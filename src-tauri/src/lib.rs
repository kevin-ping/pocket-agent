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

            // Initialize chat history database
            if let Err(e) = commands::history::init_db() {
                eprintln!("[history] failed to init db: {}", e);
            }

            let config = commands::config::load_config(handle);

            // Apply double-click mode setting
            voice::hotkey::set_double_click_mode(config.double_click_to_record);

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
            voice::record::prewarm();

            // Start local API server for push notifications (port 8650)
            {
                let server_handle = handle.clone();
                tauri::async_runtime::spawn(async move {
                    crate::api::server::start_server(server_handle, 8650).await;
                });
            }
            // ── macOS top menu bar (app name next to Apple logo) ──
            use tauri::menu::{Menu, PredefinedMenuItem, Submenu};
            let about_item = PredefinedMenuItem::about(app, None, None)?;
            let sep = PredefinedMenuItem::separator(app)?;
            let app_settings = MenuItemBuilder::with_id("app-settings", "Settings…").build(app)?;
            let sep2 = PredefinedMenuItem::separator(app)?;
            let quit_item = PredefinedMenuItem::quit(app, None)?;
            let app_submenu = Submenu::with_items(app, "Pocket Agent", true, &[&about_item, &sep, &app_settings, &sep2, &quit_item])?;
            let menu = Menu::with_items(app, &[&app_submenu])?;
            app.set_menu(menu)?;

            app.on_menu_event(move |app, event| {
                if event.id().as_ref() == "app-settings" {
                    let _ = app.emit("tray-open-settings", ());
                }
            });

            // ── Tray icon (right side of menu bar) ──
            let tray_settings = MenuItemBuilder::with_id("tray-settings", "Settings…").build(app)?;
            let tray_history = MenuItemBuilder::with_id("tray-history", "Chat History").build(app)?;
            let tray_quit = MenuItemBuilder::with_id("tray-quit", "Quit Pocket Agent").build(app)?;
            let tray_menu = MenuBuilder::new(app)
                .items(&[&tray_settings, &tray_history, &tray_quit])
                .build()?;

            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().cloned().unwrap())
                .menu(&tray_menu)
                .tooltip("Pocket Agent")
                .on_menu_event(move |app, event| {
                    match event.id().as_ref() {
                        "tray-settings" => {
                            let _ = app.emit("tray-open-settings", ());
                        }
                        "tray-history" => {
                            let _ = app.emit("tray-open-history", ());
                        }
                        "tray-quit" => {
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
            commands::history::open_chat_history,
            commands::history::save_chat_message,

            commands::voice::start_voice_recording,
            commands::voice::stop_voice_recording,
            commands::voice::cancel_voice_recording,
            commands::voice::get_audio_level,
            voice::hotkey::start_capture,
            voice::hotkey::poll_capture,
            voice::hotkey::update_hotkey,
            voice::hotkey::set_double_click_mode,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
