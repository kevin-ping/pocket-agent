use chrono;
use crate::api::client::HermesClient;
use crate::commands::config::{get_api_key, build_voice_hint};
use crate::AppState;
use futures_util::StreamExt;
use std::process::Command;
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};

/// Maximum concurrent audio items in the pipeline.
const MAX_AUDIO_QUEUE: usize = 3;

/// Jobs sent to the dedicated speak thread.
/// The thread processes them sequentially: TTS generate -> emit events -> play audio.
enum AudioCmd {
    Speak {
        text: String,
        emotion: String,
        voice: String,
        format: String,
        app: AppHandle,
        generation: u64,
    },
    Stop,
}

static AUDIO_SENDER: std::sync::OnceLock<std::sync::Mutex<std::sync::mpsc::Sender<AudioCmd>>> = std::sync::OnceLock::new();

static AUDIO_GENERATION: AtomicU64 = AtomicU64::new(0);
static AUDIO_QUEUE_DEPTH: AtomicUsize = AtomicUsize::new(0);

fn audio_sender() -> &'static Mutex<std::sync::mpsc::Sender<AudioCmd>> {
    AUDIO_SENDER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<AudioCmd>();
        std::thread::Builder::new()
            .name("speak-pipeline".to_string())
            .spawn(move || {
                for cmd in rx {
                    match cmd {
                        AudioCmd::Stop => {
                            eprintln!("[AUDIO] stop requested");
                            crate::commands::chat::audio_queue_reset();
                        }
                        AudioCmd::Speak { text, emotion, voice, format, app, generation } => {
                            let (rate, volume) = emotion_to_prosody(&emotion);
                            let char_count = text.chars().count();

                            // 1. Generate TTS first (takes time, don't start typewriter yet)
                            let tts_file = tts_path(&format);
                            let tts_ok = generate_tts_to(&text, &tts_file, &voice, rate, volume);

                            // 2. Emit text events AFTER TTS generation, RIGHT BEFORE playback
                            //    so typewriter and audio start nearly simultaneously
                            let _ = app.emit("chat-speaking-start", TypewriterStartPayload {
                                emotion: emotion.clone(),
                                total_chars: char_count,
                                has_audio: true,
                            });
                            let _ = app.emit("chat-stream", ChatStreamPayload { delta: text.clone() });
                            let _ = app.emit("chat-stream-end", ());
                            eprintln!("[SPEAK] emotion={} voice={} rate={} vol={} chars={} audio={}",
                                emotion, voice, rate, volume, char_count, tts_ok);

                            // 3. Play audio
                            if tts_ok {
                                match rodio::OutputStream::try_default() {
                                    Ok((stream, stream_handle)) => {
                                        match rodio::Sink::try_new(&stream_handle) {
                                            Ok(sink) => {
                                                if let Ok(file) = std::fs::File::open(&tts_file) {
                                                    match rodio::Decoder::new(std::io::BufReader::new(file)) {
                                                        Ok(source) => {
                                                            sink.append(source);
                                                            sink.sleep_until_end();
                                                        }
                                                        Err(e) => eprintln!("[AUDIO] decode: {}", e),
                                                    }
                                                }
                                                sink.detach();
                                                drop(stream); // release audio device
                                            }
                                            Err(e) => { eprintln!("[AUDIO] sink: {}", e); drop(stream); }
                                        }
                                    }
                                    Err(e) => eprintln!("[AUDIO] no output: {}", e),
                                }
                            }

                            // 4. Release queue slot
                            audio_queue_release();
                            eprintln!("[AUDIO] done (gen={})", generation);

                            // 5. Notify frontend
                            let _ = app.emit("chat-audio-done", ());
                        }
                    }
                }
            })
            .expect("failed to spawn speak-pipeline thread");
        Mutex::new(tx)
    })
}

/// Stop the pipeline and reset queue counter. Called on fn-key press.
pub fn stop_audio_queue() {
    let _ = audio_sender().lock().unwrap().send(AudioCmd::Stop);
}

/// Check if queue is full (read-only, for API layer).
pub fn is_queue_full() -> bool {
    AUDIO_QUEUE_DEPTH.load(Ordering::SeqCst) >= MAX_AUDIO_QUEUE
}

/// Reserve a queue slot (atomic CAS). Returns false if full.
fn audio_queue_reserve() -> bool {
    loop {
        let current = AUDIO_QUEUE_DEPTH.load(Ordering::SeqCst);
        if current >= MAX_AUDIO_QUEUE { return false; }
        if AUDIO_QUEUE_DEPTH.compare_exchange(current, current + 1, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            return true;
        }
    }
}

/// Release a slot after playback finishes.
fn audio_queue_release() {
    AUDIO_QUEUE_DEPTH.fetch_sub(1, Ordering::SeqCst);
}

/// Reset counter on stop/cancel.
pub fn audio_queue_reset() {
    AUDIO_QUEUE_DEPTH.store(0, Ordering::SeqCst);
}

/// Get edge-tts binary path from env var, fallback to "edge-tts"
fn edge_tts_bin() -> String {
    std::env::var("EDGE_TTS_BIN").unwrap_or_else(|_| "edge-tts".to_string())
}

/// Detect language from text content using Unicode character ranges.
/// English is counted by word (space-separated), not by letter,
/// so "hello world" counts as en=2, not en=10.
fn detect_language(text: &str) -> &'static str {
    let mut ja = 0u32;
    let mut ko = 0u32;
    let mut zh = 0u32;
    let mut en = 0u32;

    for ch in text.chars() {
        match ch {
            '\u{3040}'..='\u{309F}' | '\u{30A0}'..='\u{30FF}' => ja += 1,
            '\u{AC00}'..='\u{D7AF}' => ko += 1,
            '\u{4E00}'..='\u{9FFF}' => zh += 1,
            _ => {}
        }
    }
    // Count English words (sequences of ASCII letters)
    for word in text.split_whitespace() {
        let ascii_letters: String = word.chars().filter(|c| c.is_ascii_alphabetic()).collect();
        if ascii_letters.len() >= 2 {
            en += 1;
        }
    }

    if ja > 0 { return "ja"; }
    if ko > 0 { return "ko"; }
    if zh > en && zh > 0 { return "zh"; }
    if en > 0 { return "en"; }
    "zh"
}

fn voice_lang(voice: &str) -> &str {
    voice.split('-').next().unwrap_or("zh")
}

fn select_voice(text: &str, primary: &str, aux1: &str, aux2: &str, fixed_lang: &str) -> String {
    if !fixed_lang.is_empty() {
        let forced_voice = match fixed_lang {
            "aux1" if !aux1.is_empty() => aux1,
            "aux2" if !aux2.is_empty() => aux2,
            _ => primary,
        };
        eprintln!("[TTS] fixed_lang={}, forcing voice: {}", fixed_lang, forced_voice);
        return forced_voice.to_string();
    }
    let detected = detect_language(text);
    eprintln!("[TTS] detected language: {}", detected);
    for v in &[primary, aux1, aux2] {
        if !v.is_empty() && voice_lang(v) == detected {
            return v.to_string();
        }
    }
    primary.to_string()
}

#[derive(serde::Serialize, Clone)]
struct ChatStreamPayload {
    delta: String,
}

#[derive(serde::Serialize, Clone)]
struct TypewriterStartPayload {
    emotion: String,
    total_chars: usize,
    has_audio: bool,
}

fn tts_path(format: &str) -> String {
    let id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    match format {
        "mp3" => format!("/tmp/pocket-agent-tts-{}.mp3", id),
        _ => format!("/tmp/pocket-agent-tts-{}.wav", id),
    }
}

fn emotion_to_prosody(emotion: &str) -> (&'static str, &'static str) {
    match emotion {
        "cheerful" => ("+15%", "+30%"),
        "sad"      => ("-20%", "-20%"),
        "angry"    => ("+20%", "+40%"),
        "calm"     => ("-10%", "-5%"),
        "excited"  => ("+15%", "+35%"),
        "whisper"  => ("-15%", "-30%"),
        "serious"  => ("-5%",  "+10%"),
        "friendly" => ("+5%",  "+10%"),
        _          => ("+0%",  "+0%"),
    }
}

fn generate_tts_to(text: &str, path: &str, voice: &str, rate: &str, volume: &str) -> bool {
    if text.trim().is_empty() { return false; }
    eprintln!("[TTS] generating for {} chars voice={} rate={} vol={}...", text.len(), voice, rate, volume);
    let rate_arg = format!("--rate={}", rate);
    let volume_arg = format!("--volume={}", volume);
    let result = Command::new(edge_tts_bin())
        .arg("--voice").arg(voice)
        .arg("--text").arg(text)
        .arg(&rate_arg)
        .arg(&volume_arg)
        .arg("--write-media").arg(path)
        .output();
    match result {
        Ok(output) => {
            if output.status.success() && Path::new(path).exists() {
                eprintln!("[TTS] OK, {} bytes", std::fs::metadata(path).map(|m| m.len()).unwrap_or(0));
                true
            } else {
                eprintln!("[TTS] failed: {}", String::from_utf8_lossy(&output.stderr));
                false
            }
        }
        Err(e) => { eprintln!("[TTS] error: {}", e); false }
    }
}

fn detect_emotion(text: &str) -> String {
    let t = text.to_lowercase();
    let t_chars: Vec<char> = t.chars().collect();
    let has_exclamation = t_chars.iter().any(|&c| c == '!' || c == '！');
    let has_question = t_chars.iter().any(|&c| c == '?' || c == '？');

    let excited_count = t_chars.iter().filter(|&&c| c == '!' || c == '！').count();
    if excited_count >= 2 || t.contains("太棒") || t.contains("搞定") || t.contains("厉害") {
        return "excited".to_string();
    }
    if t.contains("⚠") || t.contains("警告") || t.contains("危险") || t.contains("注意") {
        return "serious".to_string();
    }
    if t.contains("吗") && has_question && !has_exclamation {
        return "calm".to_string();
    }
    if t.contains("难过") || t.contains("遗憾") || t.contains("抱歉") {
        return "sad".to_string();
    }
    if has_exclamation {
        return "cheerful".to_string();
    }
    if t.len() < 20 {
        return "friendly".to_string();
    }
    "friendly".to_string()
}

/// Shared entry point: reserve queue slot + send to pipeline.
/// The speak-pipeline thread handles TTS generation, events, and playback in order.
fn speak_internal(
    app: &AppHandle,
    text: &str,
    emotion: &str,
    voice: &str,
    format: &str,
    generation: u64,
) -> bool {
    if !audio_queue_reserve() {
        eprintln!("[SPEAK] queue full, dropping {} chars", text.chars().count());
        return false;
    }
    let _ = audio_sender().lock().unwrap().send(AudioCmd::Speak {
        text: text.to_string(),
        emotion: emotion.to_string(),
        voice: voice.to_string(),
        format: format.to_string(),
        app: app.clone(),
        generation,
    });
    true
}

/// POST to internal /push API so voice chat goes through the same pipeline.
fn push_to_self(text: &str, emotion: &str, voice: &str) {
    let api_key = std::env::var("API_SERVER_KEY").unwrap_or_default();
    let port = std::env::var("PA_PORT").unwrap_or_else(|_| "8650".to_string());
    let url = format!("http://127.0.0.1:{}/push", port);
    let body = format!(r#"{{"text":{},"emotion":{},"voice":{}}}"#,
        serde_json::to_string(text).unwrap_or_default(),
        serde_json::to_string(emotion).unwrap_or_default(),
        serde_json::to_string(voice).unwrap_or_default(),
    );
    std::thread::spawn(move || {
        let client = reqwest::blocking::Client::new();
        let resp = client.post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .body(body)
            .send();
        match resp {
            Ok(r) => eprintln!("[SSE] push_to_self: {}", r.status()),
            Err(e) => eprintln!("[SSE] push_to_self failed: {}", e),
        }
    });
}

#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    state: State<'_, AppState>,
    text: String,
    api_url: String,
    _tts_format: Option<String>,
    tts_primary_voice: Option<String>,
    tts_aux1_voice: Option<String>,
    tts_aux2_voice: Option<String>,
    user_language: Option<String>,
    fixed_lang: Option<String>,
    _tts_enabled: Option<bool>,
) -> Result<(), String> {
    let primary = tts_primary_voice.unwrap_or_else(|| "zh-CN-XiaoxiaoNeural".to_string());
    let aux1 = tts_aux1_voice.unwrap_or_default();
    let aux2 = tts_aux2_voice.unwrap_or_default();
    let api_key = get_api_key();
    let client = HermesClient::new(&api_url, api_key);

    let user_lang = user_language.unwrap_or_else(|| "zh".to_string());
    let fixed = fixed_lang.unwrap_or_default();
    let mut hint = build_voice_hint(&primary, &aux1, &aux2, &user_lang, &fixed);
    // In auto mode (no forced language), append language-follow instruction
    // so LLM does not drift to a different language from recent context
    if fixed.is_empty() {
        hint.push_str("\n\nIMPORTANT: You MUST respond in the SAME language the user writes in. If the user writes in Chinese, respond in Chinese. If the user writes in English, respond in English. Never switch languages based on previous conversation context.");
    }

    app.emit("chat-thinking-start", ()).map_err(|e| e.to_string())?;

    // Dynamic session_id: base-{yyyy-mm-dd} — auto-rotates daily
    let base_id = state.session_id.lock().unwrap().clone();
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let session_id = format!("{}-{}", base_id, today);

    // Read yesterday's daily summary for context continuity
    let yesterday = (chrono::Local::now() - chrono::TimeDelta::days(1)).format("%Y-%m-%d").to_string();
    let summary_path = format!("{}/.hermes/pa-summaries/{}.md",
        std::env::var("HOME").unwrap_or_default(), yesterday);
    let daily_summary = std::fs::read_to_string(&summary_path).ok();
    if daily_summary.is_some() {
        eprintln!("[SSE] loaded daily summary from {}", summary_path);
    }
    let mut full_response = String::new();
    let max_retries = 2;

    for attempt in 0..=max_retries {
        if attempt > 0 {
            eprintln!("[SSE] retry {}/{}", attempt, max_retries);
            std::thread::sleep(std::time::Duration::from_secs(2));
        }

        eprintln!("[SSE] >>> sending to LLM [{}] (session: {})", chrono::Local::now().format("%H:%M:%S%.3f"), session_id);
        let mut stream = match client.chat_stream(&text, Some(&hint), daily_summary.as_deref(), Some(&session_id)).await {
            Ok(s) => s,
            Err(e) => {
                if attempt < max_retries { continue; }
                return Err(e);
            }
        };
        let mut received_data = false;

        let stream_ended_normally;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(delta) => {
                    if !received_data {
                        eprintln!("[SSE] <<< first token from LLM [{}]", chrono::Local::now().format("%H:%M:%S%.3f"));
                    }
                    received_data = true;
                    full_response.push_str(&delta);
                }
                Err(e) => {
                    if !received_data && attempt < max_retries {
                        eprintln!("[SSE] no data received, will retry");
                        break;
                    }
                    app.emit("chat-stream-error", e.clone()).map_err(|e| e.to_string())?;
                    return Err(e);
                }
            }
        }
        stream_ended_normally = true;
        eprintln!("[SSE] <<< stream complete [{}] ({} chars)", chrono::Local::now().format("%H:%M:%S%.3f"), full_response.len());

        if received_data || stream_ended_normally { break; }
    }

    let full_response = if std::env::var("ENABLE_LOCAL_COMMANDS").as_deref() == Ok("true") {
        execute_commands(&full_response)
    } else {
        strip_cmd_tags(&full_response)
    };

    if full_response.trim().is_empty() {
        let _ = app.emit("chat-stream-end", ());
        return Ok(());
    }

    let emotion = detect_emotion(&full_response);
    let voice = select_voice(&full_response, &primary, &aux1, &aux2, &fixed);
    push_to_self(&full_response, &emotion, &voice);

    Ok(())
}

#[tauri::command]
pub async fn speak(_text: String) -> Result<(), String> { Ok(()) }

/// Direct TTS: speak given text without calling LLM.
/// Used by the API push endpoint to play pushed messages.
#[tauri::command]
pub async fn speak_text(
    app: AppHandle,
    text: String,
    emotion: Option<String>,
    override_voice: Option<String>,
    tts_format: Option<String>,
    tts_primary_voice: Option<String>,
    tts_aux1_voice: Option<String>,
    tts_aux2_voice: Option<String>,
    _tts_enabled: Option<bool>,
) -> Result<(), String> {
    if text.trim().is_empty() { return Ok(()); }

    let format = tts_format.unwrap_or_else(|| "wav".to_string());
    let primary = tts_primary_voice.unwrap_or_else(|| "zh-CN-XiaoxiaoNeural".to_string());
    let aux1 = tts_aux1_voice.unwrap_or_default();
    let aux2 = tts_aux2_voice.unwrap_or_default();
    let generation = AUDIO_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;

    let voice = override_voice.unwrap_or_else(|| select_voice(&text, &primary, &aux1, &aux2, ""));
    let emotion_str = emotion.unwrap_or_else(|| detect_emotion(&text));

    speak_internal(&app, &text, &emotion_str, &voice, &format, generation);

    Ok(())
}

/// Strip [CMD:...] tags from text without executing them.
fn strip_cmd_tags(text: &str) -> String {
    let re = regex::Regex::new(r#"\[CMD:[^\]]+\]"#).unwrap();
    let clean = re.replace_all(text, "").to_string();
    let space_re = regex::Regex::new(r"  +").unwrap();
    space_re.replace_all(&clean.trim(), " ").to_string()
}

/// Extract [CMD:...] tags from text, execute them, return text with tags removed.
fn execute_commands(text: &str) -> String {
    let re = regex::Regex::new(r#"\[CMD:([^\]]+)\]"#).unwrap();

    for cap in re.captures_iter(text) {
        let cmd_str = &cap[1];
        eprintln!("[LOCAL_CMD] executing: {}", cmd_str);

        let result = std::process::Command::new("sh")
            .arg("-c")
            .arg(cmd_str)
            .output();

        match result {
            Ok(output) => {
                if output.status.success() {
                    eprintln!("[LOCAL_CMD] OK");
                } else {
                    eprintln!("[LOCAL_CMD] exit={}: {}", output.status, String::from_utf8_lossy(&output.stderr));
                }
            }
            Err(e) => eprintln!("[LOCAL_CMD] error: {}", e),
        }
    }

    let clean = re.replace_all(text, "").to_string();
    let space_re = regex::Regex::new(r"  +").unwrap();
    space_re.replace_all(&clean.trim(), " ").to_string()
}
