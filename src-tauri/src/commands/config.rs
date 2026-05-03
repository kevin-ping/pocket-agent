use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_plugin_store::StoreExt;

// IMPORTANT: This is sent as a SYSTEM message to force plain text output.
// The model MUST obey this — no markdown, no bold, no code, no lists.
// Every reply will be read aloud via TTS. Keep it short (2-3 sentences max).
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
    let effective_lang = if !fixed_lang.is_empty() {
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

    // Build forced language instruction
    let forced_instruction = if !fixed_lang.is_empty() {
        format!("FORCED LANGUAGE MODE: Your TTS voice is locked to a specific language. ")
    } else {
        String::new()
    };

    let user_lang_name = match effective_lang.as_str() {
        "zh" => "Chinese (中文)",
        "ja" => "Japanese (日本語)",
        "ko" => "Korean (한국어)",
        "en" => "English",
        "fr" => "French (Français)",
        "de" => "German (Deutsch)",
        "es" => "Spanish (Español)",
        _ => primary_lang,
    };

    format!(r#"[SYSTEM INSTRUCTION - MANDATORY]
You are speaking to the user through a text-to-speech voice. Your response will be CONVERTED TO SPEECH and played aloud.

CRITICAL RULES (you MUST follow every time):
1. Respond in PLAIN TEXT ONLY. No markdown, no asterisks, no backticks, no code blocks, no bullet points, no numbered lists, no headers, no bold, no italic.
2. Keep your response CONCISE: 1-3 short sentences. Long text sounds terrible in TTS.
3. NEVER use symbols that don't speak well: # * ` [ ] {{ }} < > | \ /
4. If you need to mention code or technical terms, spell them out phonetically or describe them in plain words.

LANGUAGE RESTRICTION:
You have TTS voices installed for these languages: {lang_list}.
- You MUST ONLY respond in one of these languages.
- Default response language: {primary_lang}. Use this unless the user writes to you in another installed language.
- If the user writes in a language you do NOT have a voice for, respond in {primary_lang} and briefly explain you cannot speak that language.

{forced_instruction}You MUST respond in {user_lang_name}.
Do NOT switch to the user's actual language. Even if they write in Chinese, respond in {user_lang_name}. This is a hard TTS requirement.

VIOLATION OF ANY RULE ABOVE will cause the voice output to sound broken. Always obey.

LOCAL COMMANDS:
You can control the user's Mac by embedding command tags in your response.
Format: [CMD:shell_command]
Examples: [CMD:open -a "Google Chrome"], [CMD:open https://google.com], [CMD:open -a "Spotify"], [CMD:osascript -e 'tell application "Finder" to empty trash']
The command will be executed silently. Write your natural spoken response AROUND the tag.
Example response: 好的，帮你打开浏览器！[CMD:open -a "Google Chrome"]已经打开了哦。
You can use multiple [CMD:...] tags if needed. Available apps: Chrome, Safari, Spotify, Finder, Terminal, VS Code ("Visual Studio Code"), Notes, Calendar, Messages, Mail, etc."#,
        lang_list = lang_list,
        primary_lang = primary_lang,
        user_lang_name = user_lang_name,
        forced_instruction = forced_instruction,
    )
}

/// Load API key from environment variable.
/// Reads from .env file on startup, falls back to env var.
pub fn get_api_key() -> Option<String> {
    std::env::var("API_SERVER_KEY").ok().filter(|s| !s.is_empty())
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppConfig {
    pub api_url: String,
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
    pub fixed_lang: String,  // "", "primary", "aux1", "aux2"
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            api_url: "http://localhost:8642".to_string(),
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
            fixed_lang: String::new(),
        }
    }
}

pub fn load_config(app: &AppHandle) -> AppConfig {
    let Ok(store) = app.store("settings.json") else {
        return AppConfig::default();
    };
    let default = AppConfig::default();
    AppConfig {
        api_url: store.get("api_url").and_then(|v| v.as_str().map(String::from)).unwrap_or(default.api_url),
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
        fixed_lang: store.get("fixed_lang").and_then(|v| v.as_str().map(String::from)).unwrap_or_default(),
    }
}

#[tauri::command]
pub fn get_config(app: AppHandle) -> AppConfig {
    load_config(&app)
}

#[tauri::command]
pub async fn save_config(app: AppHandle, config: AppConfig) -> Result<(), String> {
    let store = app.store("settings.json").map_err(|e| e.to_string())?;
    store.set("api_url", serde_json::json!(config.api_url));
    store.set("volume", serde_json::json!(config.volume));
    store.set("character_skin", serde_json::json!(config.character_skin));
    store.set("dialog_style", serde_json::json!(config.dialog_style));
    store.set("tts_format", serde_json::json!(config.tts_format));
    store.set("tts_primary_voice", serde_json::json!(config.tts_primary_voice));
    store.set("tts_aux1_voice", serde_json::json!(config.tts_aux1_voice));
    store.set("tts_aux2_voice", serde_json::json!(config.tts_aux2_voice));
    if let Some(x) = config.window_x { store.set("window_x", serde_json::json!(x)); }
    if let Some(y) = config.window_y { store.set("window_y", serde_json::json!(y)); }
    if let Some(img) = &config.avatar_image { store.set("avatar_image", serde_json::json!(img)); }
    else { store.set("avatar_image", serde_json::json!(null)); }
    store.set("fixed_lang", serde_json::json!(config.fixed_lang));
    store.save().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}
