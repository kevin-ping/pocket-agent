use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_store::StoreExt;

use super::settings_repository::{self, SaveConfigResponse};

// IMPORTANT: This is sent as a SYSTEM message to force spoken-style output.
// The model MUST obey this — no formatting, no visual-only content, no symbols.
// Every reply will be read aloud via TTS. Keep it short (2-3 sentences max).
// Absolutely NO emoji, NO tables, NO charts — describe everything in plain spoken words.
/// Map voice name prefix to human-readable language name for prompt injection.
fn voice_to_language(voice: &str) -> Option<&'static str> {
    let lang = voice.split('-').next().unwrap_or("");
    match lang {
        "zh" => Some("Chinese (中文)"),
        "ja" => Some("Japanese (日本語)"),
        "ko" => Some("Korean (한국어)"),
        "en" => Some("English"),
        "fr" => Some("French (Français)"),
        "de" => Some("German (Deutsch)"),
        "es" => Some("Spanish (Español)"),
        _ => None,
    }
}

/// Build the system hint dynamically based on configured TTS voices.
/// Tells the LLM which languages it's allowed to respond in.
pub fn build_voice_hint(primary_voice: &str, aux1_voice: &str, aux2_voice: &str, user_lang: &str, fixed_lang: &str) -> String {
    // Resolve fixed language: if set, override user_lang with the fixed voice's language
    let _effective_lang = if !fixed_lang.is_empty() {
        let voice = match fixed_lang {
            "aux1" if !aux1_voice.is_empty() => aux1_voice,
            "aux2" if !aux2_voice.is_empty() => aux2_voice,
            _ => primary_voice,  // "primary" or fallback
        };
        // Extract lang code from voice (e.g. "ko" from "ko-KR-SunHiNeural")
        voice.split('-').next().unwrap_or("zh").to_string()
    } else {
        user_lang.to_string()
    };

    // Collect unique languages from configured voices
    let mut langs: Vec<String> = Vec::new();
    for voice in &[primary_voice, aux1_voice, aux2_voice] {
        if voice.is_empty() { continue; }
        if let Some(lang_name) = voice_to_language(voice) {
            let s = lang_name.to_string();
            if !langs.contains(&s) {
                langs.push(s);
            }
        }
    }

    let lang_list = if langs.is_empty() {
        "Chinese (中文)".to_string()
    } else {
        langs.join(", ")
    };

    let primary_lang = voice_to_language(primary_voice)
        .unwrap_or("Chinese (中文)");

    let local_cmd_section = if std::env::var("ENABLE_LOCAL_COMMANDS").as_deref() == Ok("true") {
        r#"LOCAL COMMAND EXECUTION — MANDATORY:
When the user asks to open, launch, or control anything on their Mac (apps, files, URLs, projects), you MUST embed a [CMD:...] tag. NEVER say you cannot do it — you CAN via shell commands.

Triggers (中文 & English): "打开/启动/运行/开个/帮我开" "open/launch/start/go to" + any app, file, or URL name.

Format: [CMD:shell_command]
Common examples:
- 打开浏览器/Chrome → [CMD:open -a "Google Chrome"]
- 打开 Safari → [CMD:open -a Safari]
- 打开项目 → [CMD:code /path/to/project]
- 打开网页 → [CMD:open https://example.com]
- 打开 Spotify → [CMD:open -a "Spotify"]
- 打开终端 → [CMD:open -a Terminal]
- 打开 Finder/文件夹 → [CMD:open /path/to/folder]
- 清空废纸篽 → [CMD:osascript -e 'tell application "Finder" to empty trash']

RULES:
1. ALWAYS use [CMD:...] when the user asks to open/launch something. NEVER refuse or say you lack access.
2. Write your natural spoken response AROUND the tag. Example: 好的，帮你打开了！[CMD:open -a "Google Chrome"]已经打开了哦。
3. The command executes silently. Multiple [CMD:...] tags are allowed if the user asks for multiple things.
4. Available apps: Chrome ("Google Chrome"), Safari, Spotify, Finder, Terminal, VS Code ("Visual Studio Code"), Notes, Calendar, Messages, Mail, System Settings, App Store, etc.
5. Open a file/folder with default app: [CMD:open /path/to/file]. Open a project in VS Code: [CMD:code /path/to/project]."#
    } else {
        ""
    };

    format!(r#"[SYSTEM INSTRUCTION - MANDATORY]
You are speaking to the user through a text-to-speech voice. Your entire response will be CONVERTED TO SPEECH and read aloud — every word must be something a human can naturally say out loud.

CRITICAL RULES (you MUST follow every time):
1. Respond in PURE SPOKEN TEXT ONLY. No markdown, no asterisks, no backticks, no code blocks, no bullet points, no numbered lists, no headers, no bold, no italic, no inline code.
2. NO TABLES, NO GRAPHS, NO DIAGRAMS, NO CHARTS -- these cannot be read aloud. Describe relationships in plain spoken sentences instead.
3. Keep your response CONCISE: 1-3 short sentences. Long text sounds terrible in TTS.
4. NEVER use any symbols or special characters that don't read well: # * ` [ ] {{ }} < > | \ / -- and absolutely NO emoji or emoticons (:), ;), etc.). These will be read as garbled noise or cause TTS errors.
5. If you need to mention code or technical terms, spell them out phonetically or describe them in plain spoken words.
LANGUAGE RESTRICTION:
You have TTS voices installed for these languages: {lang_list}.
- You MUST ONLY respond in one of these languages.
- Default response language: {primary_lang}. Use this unless the user writes to you in another installed language.
- If the user writes in a language you do NOT have a voice for, respond in {primary_lang} and briefly explain you cannot speak that language.

VIOLATION OF ANY RULE ABOVE will cause the voice output to sound broken. Always obey.

{local_cmd_section}"#,
        lang_list = lang_list,
        primary_lang = primary_lang,
        local_cmd_section = local_cmd_section,
    )
}

/// Load API URL from environment variable.
/// Reads API_SERVER from .env file on startup, falls back to env var.
pub fn get_api_url() -> String {
    std::env::var("API_SERVER")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "http://localhost:8642".to_string())
}

/// Load API key from environment variable.
pub fn get_api_key() -> Option<String> {
    std::env::var("API_SERVER_KEY").ok().filter(|s| !s.is_empty())
}

/// Load API agent from environment variable.
/// If set (e.g. "main"), PA connects to OpenClaw and routes to openclaw/{agent}.
/// If empty or unset, PA uses Hermes backend with model="default".
pub fn get_api_agent() -> Option<String> {
    std::env::var("API_AGENT").ok().filter(|s| !s.is_empty())
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppConfig {
    pub volume: f32,
    pub character_skin: String,
    pub dialog_style: String,
    pub tts_format: String,
    pub tts_primary_voice: String,
    pub tts_aux1_voice: String,
    pub tts_aux2_voice: String,
    pub window_x: Option<f64>,
    pub window_y: Option<f64>,
    pub avatar_image: Option<String>,
    pub avatar_gif: Option<String>,
    pub fixed_lang: String,
    #[serde(default = "default_ui_lang")]
    pub ui_lang: String,
    pub hotkey_code: i64,
    pub hotkey_name: String,
    pub tts_enabled: bool,
    pub double_click_to_record: bool,
    pub continuous_conversation: bool,
    pub silence_timeout_secs: u32,
    #[serde(default = "default_pause_tolerance_ms")]
    pub pause_tolerance_ms: u32,
    #[serde(default = "default_speech_rms_threshold")]
    pub speech_rms_threshold: f32,
    #[serde(default = "default_barge_in_rms_threshold")]
    pub barge_in_rms_threshold: f32,
    #[serde(default = "default_true")]
    pub barge_in_enabled: bool,
    pub skip_interrupt_confirmation: bool,
    #[serde(default)]
    pub wake_word_enabled: bool,
    #[serde(default = "default_wake_word_threshold")]
    pub wake_word_threshold: f32,
    #[serde(default)]
    pub speaker_verification_enabled: bool,
    #[serde(default)]
    pub last_enrolled_speaker: String,
}

fn default_pause_tolerance_ms() -> u32 {
    1500
}

fn default_speech_rms_threshold() -> f32 {
    0.015
}
fn default_barge_in_rms_threshold() -> f32 {
    0.04
}
fn default_true() -> bool {
    true
}
fn default_ui_lang() -> String {
    "en".to_string()
}

fn default_wake_word_threshold() -> f32 {
    0.5
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            volume: 0.8,
            character_skin: "default-css".to_string(),
            dialog_style: "bubble".to_string(),
            tts_format: "wav".to_string(),
            tts_primary_voice: "zh-CN-XiaoxiaoNeural".to_string(),
            tts_aux1_voice: String::new(),
            tts_aux2_voice: String::new(),
            window_x: None,
            window_y: None,
            avatar_image: None,
            avatar_gif: None,
            fixed_lang: String::new(),
            ui_lang: default_ui_lang(),
            hotkey_code: 60,
            hotkey_name: "RightShift".to_string(),
            tts_enabled: true,
            double_click_to_record: false,
            continuous_conversation: false,
            silence_timeout_secs: 5,
            pause_tolerance_ms: default_pause_tolerance_ms(),
            speech_rms_threshold: default_speech_rms_threshold(),
            barge_in_rms_threshold: default_barge_in_rms_threshold(),
            barge_in_enabled: true,
            skip_interrupt_confirmation: true,
            wake_word_enabled: false,
            wake_word_threshold: default_wake_word_threshold(),
            speaker_verification_enabled: false,
            last_enrolled_speaker: String::new(),
        }
    }
}

pub(crate) fn load_legacy_config(app: &AppHandle) -> AppConfig {
    let Ok(store) = app.store("settings.json") else {
        return AppConfig::default();
    };
    let default = AppConfig::default();
    AppConfig {
        volume: store.get("volume").and_then(|v| v.as_f64().map(|f| f as f32)).unwrap_or(default.volume),
        character_skin: store.get("character_skin").and_then(|v| v.as_str().map(String::from)).unwrap_or(default.character_skin),
        dialog_style: store.get("dialog_style").and_then(|v| v.as_str().map(String::from)).unwrap_or(default.dialog_style),
        tts_format: store.get("tts_format").and_then(|v| v.as_str().map(String::from)).unwrap_or(default.tts_format),
        tts_primary_voice: store.get("tts_primary_voice").and_then(|v| v.as_str().map(String::from)).unwrap_or(default.tts_primary_voice),
        tts_aux1_voice: store.get("tts_aux1_voice").and_then(|v| v.as_str().map(String::from)).unwrap_or(default.tts_aux1_voice),
        tts_aux2_voice: store.get("tts_aux2_voice").and_then(|v| v.as_str().map(String::from)).unwrap_or(default.tts_aux2_voice),
        window_x: store.get("window_x").and_then(|v| v.as_f64()),
        window_y: store.get("window_y").and_then(|v| v.as_f64()),
        avatar_image: store.get("avatar_image").and_then(|v| v.as_str().map(String::from)),
        avatar_gif: store.get("avatar_gif").and_then(|v| v.as_str().map(String::from)),
        fixed_lang: store.get("fixed_lang").and_then(|v| v.as_str().map(String::from)).unwrap_or_default(),
        ui_lang: store.get("ui_lang").and_then(|v| v.as_str().map(String::from)).unwrap_or_else(|| default_ui_lang()),
        hotkey_code: store.get("hotkey_code").and_then(|v| v.as_i64()).unwrap_or(60),
        hotkey_name: store.get("hotkey_name").and_then(|v| v.as_str().map(String::from)).unwrap_or_else(|| "RightShift".to_string()),
        tts_enabled: store.get("tts_enabled").and_then(|v| v.as_bool()).unwrap_or(true),
        double_click_to_record: store.get("double_click_to_record").and_then(|v| v.as_bool()).unwrap_or(false),
        continuous_conversation: store.get("continuous_conversation").and_then(|v| v.as_bool()).unwrap_or(default.continuous_conversation),
        silence_timeout_secs: store.get("silence_timeout_secs").and_then(|v| v.as_u64()).map(|n| n as u32).unwrap_or(default.silence_timeout_secs),
        pause_tolerance_ms: store.get("pause_tolerance_ms").and_then(|v| v.as_u64()).map(|n| n as u32).unwrap_or(default.pause_tolerance_ms),
        speech_rms_threshold: store.get("speech_rms_threshold").and_then(|v| v.as_f64()).map(|f| f as f32).unwrap_or(default.speech_rms_threshold),
        barge_in_rms_threshold: store.get("barge_in_rms_threshold").and_then(|v| v.as_f64()).map(|f| f as f32).unwrap_or(default.barge_in_rms_threshold),
        barge_in_enabled: store.get("barge_in_enabled").and_then(|v| v.as_bool()).unwrap_or(default.barge_in_enabled),
        skip_interrupt_confirmation: store.get("skip_interrupt_confirmation").and_then(|v| v.as_bool()).unwrap_or(default.skip_interrupt_confirmation),
        wake_word_enabled: store.get("wake_word_enabled").and_then(|v| v.as_bool()).unwrap_or(default.wake_word_enabled),
        wake_word_threshold: store.get("wake_word_threshold").and_then(|v| v.as_f64()).map(|f| f as f32).unwrap_or(default.wake_word_threshold),
        speaker_verification_enabled: store.get("speaker_verification_enabled").and_then(|v| v.as_bool()).unwrap_or(default.speaker_verification_enabled),
        last_enrolled_speaker: store.get("last_enrolled_speaker").and_then(|v| v.as_str().map(String::from)).unwrap_or_default(),
    }
}

pub fn load_config(_app: &AppHandle) -> AppConfig {
    settings_repository::load().unwrap_or_else(|error| {
        eprintln!("[settings] database read failed, using defaults: {error}");
        AppConfig::default()
    })
}

#[tauri::command]
pub async fn get_config(_app: AppHandle) -> Result<AppConfig, String> {
    // SQLite and avatar decoding are blocking operations. Keep them off the
    // Tauri command thread so a large avatar cannot stall the settings webview.
    tauri::async_runtime::spawn_blocking(settings_repository::load)
        .await
        .map_err(|e| format!("settings read task failed: {e}"))?
}

#[tauri::command]
pub async fn save_settings_page_config(
    app: AppHandle,
    mut config: AppConfig,
) -> Result<SaveConfigResponse, String> {
    // Assets and the PA widget position are owned by separate flows. Preserve
    // them here so the settings page only transfers the small scalar config.
    let current = tauri::async_runtime::spawn_blocking(settings_repository::load)
        .await
        .map_err(|e| format!("settings read task failed: {e}"))??;
    config.avatar_image = current.avatar_image;
    config.avatar_gif = current.avatar_gif;
    config.window_x = current.window_x;
    config.window_y = current.window_y;
    let mut saved = save_config(app, config).await?;
    saved.config.avatar_image = None;
    saved.config.avatar_gif = None;
    Ok(saved)
}

#[tauri::command]
pub async fn save_config(app: AppHandle, config: AppConfig) -> Result<SaveConfigResponse, String> {
    let old = settings_repository::load()?;
    let saved = settings_repository::save(&config)?;

    let apply_result = (|| -> Result<(), String> {
        if old.hotkey_code != saved.config.hotkey_code {
            crate::voice::hotkey::update_hotkey(saved.config.hotkey_code);
        }
        if old.double_click_to_record != saved.config.double_click_to_record {
            crate::voice::hotkey::set_double_click_mode(saved.config.double_click_to_record);
        }
        let wake_changed = old.wake_word_enabled != saved.config.wake_word_enabled
            || (old.wake_word_threshold - saved.config.wake_word_threshold).abs() > f32::EPSILON
            || old.last_enrolled_speaker != saved.config.last_enrolled_speaker;
        if wake_changed {
            crate::commands::voice::stop_wake_word_listening()?;
            if saved.config.wake_word_enabled {
                crate::commands::voice::start_wake_word_listening(
                    app.clone(),
                    Some(saved.config.wake_word_threshold),
                    Some(saved.config.last_enrolled_speaker.clone()),
                )?;
            }
        }
        Ok(())
    })();

    if let Err(error) = apply_result {
        let _ = settings_repository::save(&old);
        crate::voice::hotkey::update_hotkey(old.hotkey_code);
        crate::voice::hotkey::set_double_click_mode(old.double_click_to_record);
        let _ = crate::commands::voice::stop_wake_word_listening();
        if old.wake_word_enabled {
            let _ = crate::commands::voice::start_wake_word_listening(
                app.clone(),
                Some(old.wake_word_threshold),
                Some(old.last_enrolled_speaker.clone()),
            );
        }
        return Err(format!("apply settings failed; changes rolled back: {error}"));
    }

    app.emit("settings-changed", serde_json::json!({ "revision": saved.revision }))
        .map_err(|e| e.to_string())?;
    Ok(saved)
}

#[tauri::command]
pub fn open_settings_window(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("settings") {
        window.show().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }
    // Only inject the small scalar settings snapshot. Avatar blobs are fetched
    // independently after the form is visible.
    let bootstrap = match settings_repository::load() {
        Ok(mut config) => {
            config.avatar_image = None;
            config.avatar_gif = None;
            serde_json::json!({ "config": config })
        }
        Err(error) => serde_json::json!({ "error": error }),
    };
    let bootstrap_json = serde_json::to_string(&bootstrap).map_err(|e| e.to_string())?;
    WebviewWindowBuilder::new(&app, "settings", WebviewUrl::App("settings.html".into()))
        .initialization_script(format!(
            "window.__PA_SETTINGS_BOOTSTRAP__ = {bootstrap_json};"
        ))
        .title("Pocket Agent Settings")
        .inner_size(820.0, 650.0)
        .min_inner_size(720.0, 560.0)
        .resizable(true)
        .decorations(true)
        .center()
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn get_setting_asset(key: String) -> Result<Option<String>, String> {
    settings_repository::get_asset(&key)
}

#[tauri::command]
pub fn save_setting_asset(app: AppHandle, key: String, data_uri: String) -> Result<(), String> {
    let revision = settings_repository::save_asset(&key, &data_uri)?;
    app.emit("settings-changed", serde_json::json!({ "revision": revision }))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_setting_asset(app: AppHandle, key: String) -> Result<(), String> {
    let revision = settings_repository::delete_asset(&key)?;
    app.emit("settings-changed", serde_json::json!({ "revision": revision }))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_window_position(x: f64, y: f64) -> Result<(), String> {
    settings_repository::save_window_position(x, y)
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}
