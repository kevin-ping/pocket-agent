use crate::api::client::HermesClient;
use crate::commands::config::{get_api_key, build_voice_hint};
use crate::AppState;
use futures_util::StreamExt;
use std::process::Command;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};


/// Commands sent to the dedicated audio thread.
enum AudioCmd {
    Play { path: String, app: AppHandle, generation: u64 },
    Stop,
}

static AUDIO_SENDER: std::sync::OnceLock<std::sync::Mutex<std::sync::mpsc::Sender<AudioCmd>>> = std::sync::OnceLock::new();

fn audio_sender() -> &'static Mutex<std::sync::mpsc::Sender<AudioCmd>> {
    AUDIO_SENDER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<AudioCmd>();
        std::thread::Builder::new()
            .name("audio-player".to_string())
            .spawn(move || {
                let mut sink: Option<rodio::Sink> = None;
                let mut _stream: Option<rodio::OutputStream> = None;
                let mut queued = 0usize;

                for cmd in rx {
                    match cmd {
                        AudioCmd::Stop => {
                            if let Some(s) = sink.take() {
                                s.stop();
                                s.detach();
                            }
                            _stream = None;
                            queued = 0;
                            crate::commands::chat::reset_speaking();
                            eprintln!("[AUDIO] queue cleared");
                        }
                        AudioCmd::Play { path, app, generation } => {

                            if sink.is_none() {
                                match rodio::OutputStream::try_default() {
                                    Ok((s, sh)) => {
                                        match rodio::Sink::try_new(&sh) {
                                            Ok(sk) => { _stream = Some(s); sink = Some(sk); }
                                            Err(e) => { eprintln!("[AUDIO] sink failed: {}", e); continue; }
                                        }
                                    }
                                    Err(e) => { eprintln!("[AUDIO] no output: {}", e); continue; }
                                }
                            }
                            let file = match std::fs::File::open(&path) {
                                Ok(f) => f,
                                Err(e) => { eprintln!("[AUDIO] open failed: {}", e); continue; }
                            };
                            match rodio::Decoder::new(std::io::BufReader::new(file)) {
                                Ok(source) => {
                                    if let Some(ref s) = sink {
                                        s.append(source);
                                        queued += 1;
                                        eprintln!("[AUDIO] queued");
                                    }
                                }
                                Err(e) => { eprintln!("[AUDIO] decode failed: {}", e); continue; }
                            }
                            // Wait for all audio to finish on this sink
                            if let Some(ref s) = sink {
                                s.sleep_until_end();
                            }
                            queued = queued.saturating_sub(1);
                            crate::commands::chat::release_speaking();
                            eprintln!("[AUDIO] done, queue: {}", queued);
                            if queued == 0 && AUDIO_GENERATION.load(Ordering::SeqCst) == generation {
                                let _ = app.emit("chat-audio-done", ());
                            }
                            // After all audio finishes, drop sink so next play gets fresh one
                            if queued == 0 {
                                if let Some(s) = sink.take() { s.detach(); }
                                _stream = None;
                            }
                        }
                    }
                }
            })
            .expect("failed to spawn audio-player thread");
        Mutex::new(tx)
    })
}

/// Stop and clear the audio queue. Called when user starts a new recording.
pub fn stop_audio_queue() {
    let _ = audio_sender().lock().unwrap().send(AudioCmd::Stop);
}

/// Check if currently speaking (read-only, no lock change).
pub fn is_speaking() -> bool {
    SPEAKING_LOCK.load(Ordering::SeqCst)
}

/// Try to acquire the speaking lock. Returns false if already speaking.
fn try_acquire_speaking() -> bool {
    SPEAKING_LOCK.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok()
}

/// Release the speaking lock when done.
fn release_speaking() {
    SPEAKING_LOCK.store(false, Ordering::SeqCst);
}

/// Force-release the speaking lock (on stop/cancel).
pub fn reset_speaking() {
    SPEAKING_LOCK.store(false, Ordering::SeqCst);
}


static AUDIO_GENERATION: AtomicU64 = AtomicU64::new(0);
static SPEAKING_LOCK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Get edge-tts binary path from env var, fallback to "edge-tts" (expect in PATH)
fn edge_tts_bin() -> String {
    std::env::var("EDGE_TTS_BIN").unwrap_or_else(|_| "edge-tts".to_string())
}

/// Detect language from text content using Unicode character ranges.
/// Returns ISO 639-1 code: "zh", "ja", "ko", "en", or fallback "zh".
fn detect_language(text: &str) -> &'static str {
    let mut ja = 0u32; // hiragana + katakana
    let mut ko = 0u32; // hangul
    let mut zh = 0u32; // CJK unified ideographs
    let mut en = 0u32; // latin letters

    for ch in text.chars() {
        match ch {
            '\u{3040}'..='\u{309F}' | // Hiragana
            '\u{30A0}'..='\u{30FF}' => ja += 1,  // Katakana
            '\u{AC00}'..='\u{D7AF}' => ko += 1,   // Hangul syllables
            '\u{4E00}'..='\u{9FFF}' => zh += 1,    // CJK Unified Ideographs
            '\u{FF00}'..='\u{FFEF}' => {}          // Fullwidth forms — skip
            'a'..='z' | 'A'..='Z' => en += 1,
            _ => {}
        }
    }

    // Japanese text always contains hiragana/katakana (particles, okurigana)
    if ja > 0 { return "ja"; }
    if ko > 0 { return "ko"; }
    if zh > en && zh > 0 { return "zh"; }
    if en > 0 { return "en"; }
    "zh" // fallback
}

/// Extract language prefix from voice name: "zh-CN-XiaoxiaoNeural" -> "zh"
fn voice_lang(voice: &str) -> &str {
    voice.split('-').next().unwrap_or("zh")
}

/// Pick the right voice for the detected language.
/// When fixed_lang is set (e.g. "aux1"), forces that voice regardless of detected language.
/// Falls back to primary if no auxiliary voice matches.
fn select_voice(text: &str, primary: &str, aux1: &str, aux2: &str, fixed_lang: &str) -> String {
    // If fixed language mode is set, force the corresponding voice
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

/// rate: "+0%" default, range "-50%" to "+100%"
/// Map emotion name to (rate, volume) prosody adjustments for Edge TTS.
/// Map emotion name to (rate, volume) prosody adjustments for Edge TTS.
/// Pitch is NOT changed — only rate and volume, as confirmed by testing.
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
    eprintln!("[TTS] generating {} for {} chars with voice {} rate={} volume={}...", "audio", text.len(), voice, rate, volume);
    let rate_arg = format!("--rate={}", rate);
    let volume_arg = format!("--volume={}", volume);
    let result = Command::new(edge_tts_bin())
        .arg("--voice").arg(voice)
        .arg("--text").arg(text)
        .arg(&rate_arg)
        .arg(&volume_arg)
        .arg("--write-media").arg(&path)
        .output();
    match result {
        Ok(output) => {
            if output.status.success() && Path::new(&path).exists() {
                eprintln!("[TTS] OK, {} bytes", std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0));
                true
            } else {
                eprintln!("[TTS] failed: {}", String::from_utf8_lossy(&output.stderr));
                false
            }
        }
        Err(e) => { eprintln!("[TTS] error: {}", e); false }
    }
}

/// Send audio to the dedicated player thread. Dropped if queue is full.
fn play_audio(path: String, app: AppHandle, generation: u64) {
    let _ = audio_sender().lock().unwrap().send(AudioCmd::Play {
        path,
        app,
        generation,
    });
}

/// Simple keyword-based emotion detection for display speed mapping.
/// Returns one of: friendly, cheerful, calm, serious, sad, whisper, excited, angry
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


/// Shared playback pipeline: TTS generation -> typewriter events -> audio playback.
/// Used by both send_message (voice chat) and speak_text (API push).
fn speak_internal(
    app: &AppHandle,
    text: &str,
    emotion: &str,
    voice: &str,
    format: &str,
    tts_enabled: bool,
    generation: u64,
) -> bool {
    // Acquire speaking lock — only one message at a time
    if !try_acquire_speaking() {
        eprintln!("[SPEAK] busy, dropping {} chars", text.chars().count());
        return false;
    }

    let (rate, volume) = emotion_to_prosody(emotion);
    let tts_file = tts_path(format);
    let tts_ok = if tts_enabled {
        generate_tts_to(text, &tts_file, voice, rate, volume)
    } else {
        eprintln!("[SPEAK] TTS skipped (tts_enabled=false)");
        false
    };

    let char_count = text.chars().count();
    eprintln!("[SPEAK] emotion={} voice={} rate={} vol={} chars={} audio={}",
        emotion, voice, rate, volume, char_count, tts_ok);

    let _ = app.emit("chat-speaking-start", TypewriterStartPayload {
        emotion: emotion.to_string(),
        total_chars: char_count,
        has_audio: tts_ok,
    });
    let _ = app.emit("chat-stream", ChatStreamPayload { delta: text.to_string() });
    let _ = app.emit("chat-stream-end", ());

    if tts_ok {
        play_audio(tts_file, app.clone(), generation);
    } else {
        // No audio — release the slot we reserved
        release_speaking();
        let _ = app.emit("chat-audio-done", ());
    }

    tts_ok
}

#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    state: State<'_, AppState>,
    text: String,
    api_url: String,
    tts_format: Option<String>,
    tts_primary_voice: Option<String>,
    tts_aux1_voice: Option<String>,
    tts_aux2_voice: Option<String>,
    user_language: Option<String>,
    fixed_lang: Option<String>,
    tts_enabled: Option<bool>,
) -> Result<(), String> {
    let format = tts_format.unwrap_or_else(|| "wav".to_string());
    let primary = tts_primary_voice.unwrap_or_else(|| "zh-CN-XiaoxiaoNeural".to_string());
    let aux1 = tts_aux1_voice.unwrap_or_default();
    let aux2 = tts_aux2_voice.unwrap_or_default();
    let generation = AUDIO_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let api_key = get_api_key();
    let client = HermesClient::new(&api_url, api_key);

    let user_lang = user_language.unwrap_or_else(|| "zh".to_string());
    let fixed = fixed_lang.unwrap_or_default();
    let hint = build_voice_hint(&primary, &aux1, &aux2, &user_lang, &fixed);

    app.emit("chat-thinking-start", ()).map_err(|e| e.to_string())?;

    let session_id = state.session_id.lock().unwrap().clone();
    let mut full_response = String::new();
    let max_retries = 2;

    for attempt in 0..=max_retries {
        if attempt > 0 {
            eprintln!("[SSE] retry {}/{}", attempt, max_retries);
            std::thread::sleep(std::time::Duration::from_secs(2));
        }

        let mut stream = match client.chat_stream(&text, Some(&hint), Some(&session_id)).await {
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

        // If stream ended normally (not via error break), don't retry even if no data
        // (LLM may have used tools — tool events are filtered out but stream completes fine)
        if received_data || stream_ended_normally { break; }
    }

    // Extract and execute [CMD:...] tags from response (only if explicitly enabled)
    let full_response = if std::env::var("ENABLE_LOCAL_COMMANDS").as_deref() == Ok("true") {
        execute_commands(&full_response)
    } else {
        // Strip [CMD:...] tags silently so TTS doesn't read them
        strip_cmd_tags(&full_response)
    };

    if full_response.trim().is_empty() {
        let _ = app.emit("chat-stream-end", ());
        return Ok(());
    }

    let emotion = detect_emotion(&full_response);
    let voice = select_voice(&full_response, &primary, &aux1, &aux2, &fixed);
    let use_tts = tts_enabled.unwrap_or(true);

    speak_internal(&app, &full_response, &emotion, &voice, &format, use_tts, generation);

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
    tts_enabled: Option<bool>,
) -> Result<(), String> {
    if text.trim().is_empty() { return Ok(()); }

    let format = tts_format.unwrap_or_else(|| "wav".to_string());
    let primary = tts_primary_voice.unwrap_or_else(|| "zh-CN-XiaoxiaoNeural".to_string());
    let aux1 = tts_aux1_voice.unwrap_or_default();
    let aux2 = tts_aux2_voice.unwrap_or_default();
    let generation = AUDIO_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;

    let voice = override_voice.unwrap_or_else(|| select_voice(&text, &primary, &aux1, &aux2, ""));
    let emotion_str = emotion.unwrap_or_else(|| detect_emotion(&text));
    let use_tts = tts_enabled.unwrap_or(true);

    speak_internal(&app, &text, &emotion_str, &voice, &format, use_tts, generation);

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

        // Use sh -c for all commands to handle quotes and complex args
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

    // Remove all [CMD:...] tags from text (so TTS doesn't read them)
    let clean = re.replace_all(text, "").to_string();
    // Clean up extra whitespace left by removed tags
    let space_re = regex::Regex::new(r"  +").unwrap();
    space_re.replace_all(&clean.trim(), " ").to_string()
}
