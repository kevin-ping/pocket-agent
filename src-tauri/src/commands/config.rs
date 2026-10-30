use std::collections::BTreeMap;

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

    format!(r#"[SYSTEM INSTRUCTION - MANDATORY]
You are speaking to the user through a text-to-speech voice. Your entire response will be CONVERTED TO SPEECH and read aloud — every word must be something a human can naturally say out loud.

CRITICAL RULES (you MUST follow every time):
1. Respond in PURE SPOKEN TEXT ONLY. No markdown, no asterisks, no backticks, no code blocks, no bullet points, no numbered lists, no headers, no bold, no italic, no inline code.
2. NO TABLES, NO GRAPHS, NO DIAGRAMS, NO CHARTS -- these cannot be read aloud. Describe relationships in plain spoken sentences instead.
3. Talk like a friend chatting face to face, not like someone filing a report. By default answer in 1-2 short sentences, with the conclusion in the first sentence. No opening filler ("Of course", "Great question", "Hope this helps"), do not restate the question, and do not narrate your process ("first I will..., then..., finally..."). Give a detailed explanation ONLY when the user explicitly asks for detail, steps, examples, the reason why or an in-depth explanation; even then keep it flowing spoken prose. If blocked or not finished, say plainly what failed, what approval is needed or what is missing; never claim an unverified action succeeded.
4. NEVER use any symbols or special characters that don't read well: # * ` [ ] {{ }} < > | \ / -- and absolutely NO emoji or emoticons (:), ;), etc.). These will be read as garbled noise or cause TTS errors.
5. If you need to mention code or technical terms, spell them out phonetically or describe them in plain spoken words.

VOICE OVERRIDE (takes priority over any other instruction about written formatting or structure, including style hints attached to the user's message):
This is a voice channel. Even when you are allowed to go into detail or the length limit is lifted, only the content expands, never the format: keep it connected spoken prose with no lists, no headings and no structure. Do not read commands, file paths, code or IDs out verbatim; describe in one sentence what they are. Never use bracket labels such as 【建议】 or [blocked].

LANGUAGE RESTRICTION:
You have TTS voices installed for these languages: {lang_list}.
- You MUST ONLY respond in one of these languages.
- Default response language: {primary_lang}. Use this unless the user writes to you in another installed language.
- If the user writes in a language you do NOT have a voice for, respond in {primary_lang} and briefly explain you cannot speak that language.

VIOLATION OF ANY RULE ABOVE will cause the voice output to sound broken. Always obey."#,
        lang_list = lang_list,
        primary_lang = primary_lang,
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
    pub wake_phrase: String,
    #[serde(default = "default_kws_threshold")]
    pub wake_kws_threshold: f32,
    #[serde(default = "default_true")]
    pub wake_owner_only: bool,
    #[serde(default)]
    pub speaker_verification_enabled: bool,
    #[serde(default)]
    pub conversation_owner_only: bool,
    #[serde(default)]
    pub last_enrolled_speaker: String,
    #[serde(default = "default_true")]
    pub mesh_avatar_enabled: bool,
    #[serde(default = "default_mesh_avatar_scale")]
    pub mesh_avatar_scale: f32,
    #[serde(default)]
    pub mesh_avatar_offset_x: i32,
    #[serde(default)]
    pub mesh_avatar_offset_y: i32,
    /// Character state → mesh-avatar expression.
    #[serde(default = "default_mesh_avatar_expressions")]
    pub mesh_avatar_expressions: BTreeMap<String, String>,
    /// PA reply emotion → mesh-avatar emotion.
    #[serde(default = "default_mesh_avatar_emotions")]
    pub mesh_avatar_emotions: BTreeMap<String, String>,
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

fn default_kws_threshold() -> f32 { 0.25 }

fn default_mesh_avatar_scale() -> f32 {
    1.0
}

fn string_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

pub(crate) fn default_mesh_avatar_expressions() -> BTreeMap<String, String> {
    string_map(&[
        ("idle", "neutral"),
        ("listening", "surprise"),
        ("thinking", "halfLidded"),
        ("speaking", "neutral"),
    ])
}

pub(crate) fn default_mesh_avatar_emotions() -> BTreeMap<String, String> {
    string_map(&[
        ("cheerful", "happy"),
        ("excited", "happy"),
        ("friendly", "happy"),
        ("sad", "sad"),
        ("angry", "angry"),
        ("calm", "relaxed"),
        ("whisper", "relaxed"),
        ("serious", "neutral"),
    ])
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
            wake_phrase: String::new(),
            wake_kws_threshold: default_kws_threshold(),
            wake_owner_only: true,
            speaker_verification_enabled: false,
            conversation_owner_only: false,
            last_enrolled_speaker: String::new(),
            mesh_avatar_enabled: true,
            mesh_avatar_scale: default_mesh_avatar_scale(),
            mesh_avatar_offset_x: 0,
            mesh_avatar_offset_y: 0,
            mesh_avatar_expressions: default_mesh_avatar_expressions(),
            mesh_avatar_emotions: default_mesh_avatar_emotions(),
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
        wake_phrase: String::new(),
        wake_kws_threshold: default_kws_threshold(),
        wake_owner_only: true,
        speaker_verification_enabled: store.get("speaker_verification_enabled").and_then(|v| v.as_bool()).unwrap_or(default.speaker_verification_enabled),
        conversation_owner_only: default.conversation_owner_only,
        last_enrolled_speaker: store.get("last_enrolled_speaker").and_then(|v| v.as_str().map(String::from)).unwrap_or_default(),
        mesh_avatar_enabled: default.mesh_avatar_enabled,
        mesh_avatar_scale: default.mesh_avatar_scale,
        mesh_avatar_offset_x: default.mesh_avatar_offset_x,
        mesh_avatar_offset_y: default.mesh_avatar_offset_y,
        mesh_avatar_expressions: default.mesh_avatar_expressions,
        mesh_avatar_emotions: default.mesh_avatar_emotions,
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
    tauri::async_runtime::spawn_blocking(move || save_config_blocking(app, config))
        .await.map_err(|e| format!("settings apply task failed: {e}"))?
}

fn save_config_blocking(app: AppHandle, config: AppConfig) -> Result<SaveConfigResponse, String> {
    static APPLY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = APPLY_LOCK.lock().map_err(|_| "settings apply lock poisoned")?;
    let old = settings_repository::load()?;
    let saved = save_then_apply(
        &old,
        &config,
        settings_repository::save,
        |new| {
            if old.hotkey_code != new.hotkey_code {
                crate::voice::hotkey::update_hotkey(new.hotkey_code);
            }
            if old.double_click_to_record != new.double_click_to_record {
                crate::voice::hotkey::set_double_click_mode(new.double_click_to_record);
            }
            Ok(())
        },
        || {
            crate::voice::hotkey::update_hotkey(old.hotkey_code);
            crate::voice::hotkey::set_double_click_mode(old.double_click_to_record);
        },
        |new| {
            crate::commands::voice::stop_wake_word_listening()?;
            if new.wake_word_enabled {
                crate::voice::sherpa_wake::start_wake_listener(
                    app.clone(),
                    new.wake_word_threshold,
                    Some(new.last_enrolled_speaker.clone()),
                )?;
            }
            Ok(())
        },
    )?;

    app.emit("settings-changed", serde_json::json!({ "revision": saved.revision }))
        .map_err(|e| e.to_string())?;
    Ok(saved)
}

fn wake_changed(old: &AppConfig, new: &AppConfig) -> bool {
    old.wake_word_enabled != new.wake_word_enabled
        || (old.wake_word_threshold - new.wake_word_threshold).abs() > f32::EPSILON
        || old.last_enrolled_speaker != new.last_enrolled_speaker
        || old.wake_phrase != new.wake_phrase
        || old.wake_kws_threshold != new.wake_kws_threshold
        || old.wake_owner_only != new.wake_owner_only
}

/// Persist `config`, then apply it. A failed non-wake step restores `old` and
/// fails the save. A failed wake restart does not: everything stays saved (wake
/// settings as entered, listener off) and the reason is returned in
/// `wake_error`, so wake problems never block unrelated settings.
fn save_then_apply(
    old: &AppConfig,
    config: &AppConfig,
    save: impl Fn(&AppConfig) -> Result<SaveConfigResponse, String>,
    apply_other: impl FnOnce(&AppConfig) -> Result<(), String>,
    rollback_other: impl FnOnce(),
    restart_wake: impl FnOnce(&AppConfig) -> Result<(), String>,
) -> Result<SaveConfigResponse, String> {
    let mut saved = save(config)?;
    if let Err(error) = apply_other(&saved.config) {
        let _ = save(old);
        rollback_other();
        return Err(format!("apply settings failed; changes rolled back: {error}"));
    }
    if wake_changed(old, &saved.config) {
        if let Err(error) = restart_wake(&saved.config) {
            eprintln!("[settings] saved; wake listener not started: {error}");
            saved.wake_error = Some(error);
        }
    }
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
pub async fn get_setting_asset(key: String) -> Result<Option<String>, String> {
    // Assets can be up to 10 MB and the main window reads the mesh avatar on
    // every settings change; keep the read and base64 encoding off the main thread.
    tauri::async_runtime::spawn_blocking(move || settings_repository::get_asset(&key))
        .await
        .map_err(|e| format!("settings asset read task failed: {e}"))?
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_assistant_is_brief_by_default_but_can_explain_on_request() {
        let hint = build_voice_hint("zh-CN-XiaoxiaoNeural", "", "", "zh", "");
        assert!(hint.contains("By default answer in 1-2 short sentences"));
        assert!(hint.contains("ONLY when the user explicitly asks for detail"));
        // How to carry out computer actions is left to Hermes.
        assert!(!hint.contains("For an authorized computer action"));
        assert!(hint.contains("never claim an unverified action succeeded"));
    }

    #[test]
    fn voice_hint_sounds_like_chat_and_keeps_format_spoken_when_expanding() {
        let hint = build_voice_hint("zh-CN-XiaoxiaoNeural", "", "", "zh", "");
        assert!(hint.contains("friend chatting face to face"));
        assert!(hint.contains("conclusion in the first sentence"));
        assert!(hint.contains("No opening filler"));
        assert!(hint.contains("do not narrate your process"));
        assert!(hint.contains("VOICE OVERRIDE"));
        assert!(hint.contains("only the content expands, never the format"));
        assert!(hint.contains("Do not read commands, file paths, code or IDs out verbatim"));
        assert!(hint.contains("Never use bracket labels"));
        assert!(hint.contains("takes priority over any other instruction about written formatting"));
    }

    #[test]
    fn voice_hint_never_teaches_local_command_tags() {
        // 桌面能力唯一入口是 Hermes computer_use，提示词不得再提 [CMD:] 通道
        std::env::set_var("ENABLE_LOCAL_COMMANDS", "true");
        let hint = build_voice_hint("zh-CN-XiaoxiaoNeural", "", "", "zh", "");
        assert!(!hint.contains("[CMD:"));
        assert!(!hint.contains("LOCAL COMMAND EXECUTION"));
    }

    /// Runs `save_then_apply` against an in-memory store; returns the result and
    /// what ended up stored.
    fn run_save(
        old: &AppConfig,
        new: &AppConfig,
        apply_other: Result<(), String>,
        restart_wake: Result<(), String>,
    ) -> (Result<SaveConfigResponse, String>, AppConfig, bool, bool) {
        let stored = std::cell::RefCell::new(old.clone());
        let (mut rolled_back, mut wake_called) = (false, false);
        let result = save_then_apply(
            old,
            new,
            |c| {
                *stored.borrow_mut() = c.clone();
                Ok(SaveConfigResponse { config: c.clone(), revision: 1, wake_error: None })
            },
            |_| apply_other,
            || rolled_back = true,
            |_| {
                wake_called = true;
                restart_wake
            },
        );
        (result, stored.into_inner(), rolled_back, wake_called)
    }

    fn wake_on(phrase: &str) -> AppConfig {
        AppConfig { wake_word_enabled: true, wake_phrase: phrase.into(), ..AppConfig::default() }
    }

    #[test]
    fn wake_failure_keeps_unrelated_settings_saved() {
        let old = wake_on("hey there");
        let mut new = wake_on("hey mimikyu");
        new.mesh_avatar_enabled = !old.mesh_avatar_enabled;
        for code in ["wake_phrase_unpronounceable", "wake_engine_unsupported_os"] {
            let err = crate::voice::sherpa_wake::wake_error(code, serde_json::json!({}));
            let (result, stored, rolled_back, _) = run_save(&old, &new, Ok(()), Err(err.clone()));
            let saved = result.expect("save must succeed");
            assert_eq!(saved.wake_error.as_deref(), Some(err.as_str()));
            assert_eq!(stored.mesh_avatar_enabled, new.mesh_avatar_enabled);
            // Wake settings stay as entered and are not switched off.
            assert!(stored.wake_word_enabled);
            assert_eq!(stored.wake_phrase, "hey mimikyu");
            assert!(!rolled_back);
        }
    }

    #[test]
    fn non_wake_apply_failure_still_rolls_back_everything() {
        let old = wake_on("hey there");
        let mut new = old.clone();
        new.hotkey_code = old.hotkey_code + 1;
        new.wake_phrase = "hey mimikyu".into();
        let (result, stored, rolled_back, wake_called) =
            run_save(&old, &new, Err("hotkey".into()), Ok(()));
        assert!(result.unwrap_err().contains("rolled back"));
        assert_eq!(stored.hotkey_code, old.hotkey_code);
        assert_eq!(stored.wake_phrase, old.wake_phrase);
        assert!(rolled_back && !wake_called);
    }

    #[test]
    fn unchanged_wake_settings_do_not_restart_wake() {
        let old = wake_on("hey there");
        let mut new = old.clone();
        new.mesh_avatar_enabled = !old.mesh_avatar_enabled;
        let (result, _, _, wake_called) = run_save(&old, &new, Ok(()), Err("unused".into()));
        assert!(result.unwrap().wake_error.is_none());
        assert!(!wake_called);
    }
}
