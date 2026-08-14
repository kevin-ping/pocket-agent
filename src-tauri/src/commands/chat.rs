use chrono;
use crate::api::client::{HermesClient, StreamEvent};
use crate::commands::config::{get_api_key, get_api_url, get_api_agent, build_voice_hint};
use crate::AppState;
use futures_util::StreamExt;
use std::process::Command;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

static CURRENT_AUDIO_SINK: std::sync::OnceLock<std::sync::Mutex<Option<std::sync::Arc<rodio::Sink>>>> = std::sync::OnceLock::new();
use tauri::{AppHandle, Emitter, State};

/// Maximum concurrent audio items in the pipeline.
const MAX_AUDIO_QUEUE: usize = 10;

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
        /// If true, emit chat-stream text events (used by push API).
        /// If false, only generate TTS and play (used by streaming TTS, text already displayed).
        show_text: bool,
        /// If true, this is a status announcement: skip all chat-audio-* events so
        /// the StatusPanel isn't cleared and the character isn't transitioned to "speaking".
        silent: bool,
    },
    Stop,
}

/// A prepared TTS audio file ready for playback.
struct PreparedAudio {
    tts_file: String,
    text: String,
    text_len: usize,
    generation: u64,
    show_text: bool,
    silent: bool,
    emotion: String,
    app: AppHandle,
}

static AUDIO_SENDER: std::sync::OnceLock<std::sync::Mutex<std::sync::mpsc::Sender<AudioCmd>>> = std::sync::OnceLock::new();


static AUDIO_GENERATION: AtomicU64 = AtomicU64::new(0);
static AUDIO_QUEUE_DEPTH: AtomicUsize = AtomicUsize::new(0);
static TURN_GENERATION: AtomicU64 = AtomicU64::new(0);
/// Non-silent audio jobs submitted-but-not-yet-finished for the current AUDIO_GENERATION.
/// Used to fire chat-audio-done exactly once, when the LAST sentence of a (possibly
/// multi-sentence) turn finishes playing — not after every sentence.
static PENDING_NON_SILENT: AtomicI64 = AtomicI64::new(0);
/// True once the submitter (run_hermes_turn / speak_text) knows no more non-silent
/// jobs will be submitted for the current generation.
static SUBMISSION_DONE: AtomicBool = AtomicBool::new(true);
/// Generation for which chat-audio-done has already been emitted (dedup guard); 0 = none yet.
static AUDIO_DONE_EMITTED_GEN: AtomicU64 = AtomicU64::new(0);

fn current_audio_sink() -> &'static Mutex<Option<std::sync::Arc<rodio::Sink>>> {
    CURRENT_AUDIO_SINK.get_or_init(|| Mutex::new(None))
}

fn audio_sender() -> &'static Mutex<std::sync::mpsc::Sender<AudioCmd>> {
    AUDIO_SENDER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<AudioCmd>();
        let (prep_tx, prep_rx) = std::sync::mpsc::channel::<PreparedAudio>();

        // === Thread 1: TTS Generator ===
        // Receives AudioCmd::Speak, generates TTS, sends PreparedAudio to player
        {
            let prep_tx = prep_tx.clone();
            std::thread::Builder::new()
                .name("tts-gen".to_string())
                .spawn(move || {
                    for cmd in rx {
                        match cmd {
                            AudioCmd::Stop => {}
                            AudioCmd::Speak { text, emotion, voice, format, app, generation, show_text, silent } => {
                                if generation != AUDIO_GENERATION.load(Ordering::SeqCst) {
                                    audio_queue_release(generation);
                                    continue;
                                }

                                let (rate, volume) = emotion_to_prosody(&emotion);
                                let tts_file = tts_path(&format);
                                let tts_ok = generate_tts_to(&text, &tts_file, &voice, rate, volume);
                                eprintln!("[TTS-GEN] gen={} chars={} ok={}", generation, text.chars().count(), tts_ok);

                                let text_len = text.chars().count();

                                let _ = prep_tx.send(PreparedAudio {
                                    tts_file,
                                    text,
                                    text_len,
                                    generation,
                                    show_text,
                                    silent,
                                    emotion,
                                    app,
                                });
                            }
                        }
                    }
                })
                .expect("failed to spawn tts-gen thread");
        }

        // === Thread 2: Audio Player ===
        // Takes pre-generated audio files and plays them immediately
        std::thread::Builder::new()
            .name("audio-player".to_string())
            .spawn(move || {
                for prep in prep_rx {
                    if prep.generation != AUDIO_GENERATION.load(Ordering::SeqCst) {
                        audio_queue_release(prep.generation);
                        continue;
                    }

                    // Emit text events for push API
                    if prep.show_text {
                        let _ = prep.app.emit("chat-speaking-start", TypewriterStartPayload {
                            emotion: prep.emotion.clone(),
                            total_chars: prep.text_len,
                            has_audio: true,
                        });
                        let _ = prep.app.emit("chat-stream", ChatStreamPayload {
                            delta: prep.text.clone(),
                        });
                        let _ = prep.app.emit("chat-stream-end", ());
                    }

                    let tts_ok = !prep.tts_file.is_empty() && Path::new(&prep.tts_file).exists();
                    eprintln!("[PLAYER] gen={} chars={} audio={}", prep.generation, prep.text_len, tts_ok);

                    if tts_ok {
                        match rodio::OutputStream::try_default() {
                            Ok((stream, stream_handle)) => {
                                match rodio::Sink::try_new(&stream_handle) {
                                    Ok(sink) => {
                                        let sink = std::sync::Arc::new(sink);
                                        *current_audio_sink().lock().unwrap() = Some(sink.clone());
                                        if let Ok(file) = std::fs::File::open(&prep.tts_file) {
                                            match rodio::Decoder::new(std::io::BufReader::new(file)) {
                                                Ok(source) => {
                                                    // Signal real audio start — frontend uses this
                                                    // (not chat-speaking-start) to switch to speaking animation.
                                                    // Skip for status announcements so the StatusPanel
                                                    // isn't cleared mid-turn.
                                                    if !prep.silent {
                                                        let _ = prep.app.emit("chat-audio-playing", ());
                                                    }
                                                    sink.append(source);
                                                    sink.sleep_until_end();
                                                }
                                                Err(e) => eprintln!("[AUDIO] decode: {}", e),
                                            }
                                        }
                                        *current_audio_sink().lock().unwrap() = None;
                                        drop(stream);
                                    }
                                    Err(e) => { eprintln!("[AUDIO] sink: {}", e); drop(stream); }
                                }
                            }
                            Err(e) => eprintln!("[AUDIO] no output: {}", e),
                        }
                    }

                    audio_queue_release(prep.generation);
                    let remaining = AUDIO_QUEUE_DEPTH.load(Ordering::SeqCst);
                    eprintln!("[AUDIO] done (gen={}, remaining={})", prep.generation, remaining);

                    // chat-audio-done fires once per turn, when every non-silent sentence
                    // submitted for this generation (see PENDING_NON_SILENT) has finished
                    // playing — not after each individual sentence.
                    if !prep.silent {
                        PENDING_NON_SILENT.fetch_sub(1, Ordering::SeqCst);
                        maybe_emit_audio_done(&prep.app, prep.generation);
                    }
                }
            })
            .expect("failed to spawn audio-player thread");
        Mutex::new(tx)
    })
}

/// Stop the pipeline and reset queue counter. Called on fn-key press and barge-in.
pub fn stop_audio_queue() {
    eprintln!("[AUDIO] stop requested");
    AUDIO_GENERATION.fetch_add(1, Ordering::SeqCst);
    // Supersede any in-flight SSE turn so its sentence callback stops
    // reserving queue slots that will never be released (leak → queue
    // full → new turn's TTS silently dropped).
    TURN_GENERATION.fetch_add(1, Ordering::SeqCst);
    audio_queue_reset();
    let sink = current_audio_sink().lock().unwrap().take();
    if let Some(sink) = sink {
        sink.stop();
    }
    let _ = audio_sender().lock().unwrap().send(AudioCmd::Stop);
    schedule_wake_resume_after_grace();
}

/// FEAT-B4: schedule wake-listener resume after a short grace so the TTS tail
/// fading through speakers→mic doesn't immediately re-trigger detection. If a
/// new TTS chunk reserves a slot during the grace window, the resume is a
/// no-op (queue depth > 0 short-circuits).
fn schedule_wake_resume_after_grace() {
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(300));
        if AUDIO_QUEUE_DEPTH.load(Ordering::SeqCst) == 0 {
            crate::voice::sherpa_wake::resume_wake();
        }
    });
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
            // FEAT-B4: first TTS chunk → suppress wake-word detection so the
            // assistant's own audio fed through speakers→mic can't self-trigger.
            if current == 0 {
                crate::voice::sherpa_wake::pause_wake();
            }
            return true;
        }
    }
}

/// Release a slot after playback finishes.
fn audio_queue_release(generation: u64) {
    if generation != AUDIO_GENERATION.load(Ordering::SeqCst) {
        return;
    }
    loop {
        let current = AUDIO_QUEUE_DEPTH.load(Ordering::SeqCst);
        if current == 0 {
            return;
        }
        if AUDIO_QUEUE_DEPTH
            .compare_exchange(current, current - 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            // FEAT-B4: last TTS chunk just finished → schedule wake resume
            // after a 300 ms grace window so the speaker tail doesn't retrigger.
            if current == 1 {
                schedule_wake_resume_after_grace();
            }
            return;
        }
    }
}

/// Reset counter on stop/cancel.
pub fn audio_queue_reset() {
    AUDIO_QUEUE_DEPTH.store(0, Ordering::SeqCst);
}

/// Reset chat-audio-done tracking for a freshly allocated speak generation
/// (called once per turn, before any sentences are submitted).
fn begin_audio_submission() {
    PENDING_NON_SILENT.store(0, Ordering::SeqCst);
    SUBMISSION_DONE.store(false, Ordering::SeqCst);
    AUDIO_DONE_EMITTED_GEN.store(0, Ordering::SeqCst);
}

/// Emit chat-audio-done exactly once for `generation`, once all of its non-silent
/// sentences have been submitted (SUBMISSION_DONE) and finished playing (PENDING_NON_SILENT <= 0).
fn maybe_emit_audio_done(app: &AppHandle, generation: u64) {
    if generation != AUDIO_GENERATION.load(Ordering::SeqCst) {
        return;
    }
    if !SUBMISSION_DONE.load(Ordering::SeqCst) {
        return;
    }
    if PENDING_NON_SILENT.load(Ordering::SeqCst) > 0 {
        return;
    }
    if AUDIO_DONE_EMITTED_GEN
        .compare_exchange(0, generation, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        eprintln!("[AUDIO] emitting chat-audio-done (gen={})", generation);
        let _ = app.emit("chat-audio-done", ());
    }
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

fn select_voice(text: &str, primary: &str, aux1: &str, aux2: &str, fixed_lang: &str, _user_lang: &str) -> String {
    // 忽略 user_lang 参数，始终根据回复内容的语言来选择 TTS 语音
    // 这是为了支持：用户用中文问，但 LLM 返回日语/英语等情况
    if !fixed_lang.is_empty() {
        let forced_voice = match fixed_lang {
            "aux1" if !aux1.is_empty() => aux1,
            "aux2" if !aux2.is_empty() => aux2,
            _ => primary,
        };
        // Graceful degradation: if response language doesn't match forced voice,
        // fall back to auto-detection to avoid edge-tts NoAudioReceived error
        let forced_lang = voice_lang(forced_voice);
        let detected = detect_language(text);
        if forced_lang == detected {
            eprintln!("[TTS] fixed_lang={}, forcing voice: {} (matches detected: {})", fixed_lang, forced_voice, detected);
            return forced_voice.to_string();
        } else {
            eprintln!("[TTS] fixed_lang={} but detected={} — falling back to auto voice to avoid TTS failure", fixed_lang, detected);
            // Fall through to auto-detection below
        }
    }
    // 根据回复内容检测语言并选择对应的 TTS 语音
    let lang = detect_language(text);
    eprintln!("[TTS] response lang detected: {}", lang);
    for v in &[primary, aux1, aux2] {
        if !v.is_empty() && voice_lang(v) == lang {
            return v.to_string();
        }
    }
    // Fallback: try primary voice if no match
    primary.to_string()
}

pub fn build_bridge_session_key(source: &str, session_id: &str) -> String {
    format!("bridge:{}:{}", source.trim(), session_id.trim())
}

fn build_ui_turn_text(text: &str, primary: &str, aux1: &str, aux2: &str, fixed: &str) -> String {
    let forced_suffix = if !fixed.is_empty() {
        let voice = match fixed {
            "aux1" if !aux1.is_empty() => aux1,
            "aux2" if !aux2.is_empty() => aux2,
            _ => primary,
        };
        let lang_code = voice.split('-').next().unwrap_or("zh");
        let lang_name = match lang_code {
            "zh" => "Chinese",
            "ja" => "Japanese",
            "ko" => "Korean",
            "en" => "English",
            "fr" => "French",
            "de" => "German",
            "es" => "Spanish",
            _ => "Chinese",
        };
        format!(" Please reply in {}.", lang_name)
    } else {
        let detected = detect_language(text);
        let lang_name = match detected {
            "en" => "English",
            "ja" => "Japanese",
            "ko" => "Korean",
            _ => "Chinese",
        };
        format!(" Please reply in {}.", lang_name)
    };
    format!("{}{}", text, forced_suffix)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HermesTurnMode {
    Ui,
    Bridge,
}

struct HermesTurnRequest {
    text: String,
    session_id: String,
    voice_hint: Option<String>,
    context: Option<String>,
    mode: HermesTurnMode,
    /// Optional callback for streaming TTS: called with each complete sentence as LLM streams
    on_sentence: Option<std::sync::Arc<dyn Fn(&str) + Send + Sync>>,
    /// Generation captured at turn start. The streaming loop breaks out cleanly
    /// when `TURN_GENERATION` advances past this value.
    turn_gen: u64,
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
        "calm"     => ("+0%", "-5%"),
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


/// Split text into sentences for streaming TTS.
/// Returns complete sentences + remaining buffer.
/// Sentences end at: 。！？!? and also \n
/// The ASCII period '.' is only a sentence boundary when NOT part of a number (e.g. "2.54").
fn split_sentences(buffer: &str) -> (Vec<String>, String) {
    let mut sentences = Vec::new();
    let chars: Vec<char> = buffer.chars().collect();
    let mut last_split = 0;

    // Track whether we are inside a [CMD:...] tag so we don't split mid-tag.
    // URL periods inside CMD tags would otherwise be treated as sentence ends.
    let mut in_cmd_tag = false;

    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];

        // Detect [CMD: tag start and ] tag end
        if !in_cmd_tag {
            // Check for "[CMD:" prefix
            let remaining_str: String = chars[i..].iter().collect();
            if remaining_str.starts_with("[CMD:") {
                in_cmd_tag = true;
                // Skip past "[CMD:"
                i += 5;
                continue;
            }
        } else {
            if ch == ']' {
                in_cmd_tag = false;
            }
            i += 1;
            continue;
        }

        // Check if this is a sentence-ending char
        let is_end = matches!(ch, '。' | '！' | '？' | '!' | '?' | '\n');

        // Special handling for ASCII period: skip if it's a decimal point (digit.digit)
        let is_period = ch == '.';
        let is_decimal = if is_period {
            let prev_digit = i > 0 && chars[i - 1].is_ascii_digit();
            let next_digit = i + 1 < chars.len() && chars[i + 1].is_ascii_digit();
            prev_digit && next_digit
        } else {
            false
        };

        if (is_end || (is_period && !is_decimal)) && (i - last_split + 1) >= 2 {
            let end = i + 1;
            let sentence: String = chars[last_split..end].iter().collect();
            let trimmed = sentence.trim();
            if !trimmed.is_empty() {
                sentences.push(trimmed.to_string());
            }
            last_split = end;
        }

        i += 1;
    }

    let remaining: String = chars[last_split..].iter().collect();
    (sentences, remaining)
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
    show_text: bool,
) -> bool {
    // Wait for a queue slot (blocking) instead of dropping sentences
    let max_wait = std::time::Duration::from_secs(30);
    let start = std::time::Instant::now();
    while !audio_queue_reserve() {
        if start.elapsed() > max_wait {
            eprintln!("[SPEAK] queue wait timeout, dropping {} chars", text.chars().count());
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let _ = audio_sender().lock().unwrap().send(AudioCmd::Speak {
        text: text.to_string(),
        emotion: emotion.to_string(),
        voice: voice.to_string(),
        format: format.to_string(),
        app: app.clone(),
        generation,
        show_text,
        silent: false,
    });
    if generation == AUDIO_GENERATION.load(Ordering::SeqCst) {
        PENDING_NON_SILENT.fetch_add(1, Ordering::SeqCst);
    }
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

fn emit_text_without_tts(app: &AppHandle, full_response: &str, emotion: &str) {
    let cleaned = strip_all_cmd_tags(full_response);
    if cleaned.is_empty() { return; }

    // Text-only fallback: no audio plays, but UI still wants the "speaking" animation
    // while text streams. Fire chat-audio-playing alongside speaking-start.
    let _ = app.emit("chat-audio-playing", ());
    let _ = app.emit("chat-speaking-start", TypewriterStartPayload {
        emotion: emotion.to_string(),
        total_chars: cleaned.chars().count(),
        has_audio: false,
    });
    for ch in cleaned.chars() {
        let _ = app.emit("chat-stream", ChatStreamPayload { delta: ch.to_string() });
    }
    let _ = app.emit("chat-stream-end", ());
}

async fn run_hermes_turn(
    app: &AppHandle,
    client: &HermesClient,
    request: &HermesTurnRequest,
) -> Result<String, String> {
    let mut full_response = String::new();
    let mut sentence_buffer = String::new();
    let max_retries = 2;
    // Event name prefix: bridge mode uses "bridge-" prefix, UI mode uses "chat-"
    let evp = |suffix: &str| -> String {
        match request.mode {
            HermesTurnMode::Bridge => format!("bridge-{}", suffix),
            HermesTurnMode::Ui => format!("chat-{}", suffix),
        }
    };

    for attempt in 0..=max_retries {
        if attempt > 0 {
            eprintln!("[SSE] retry {}/{}", attempt, max_retries);
            std::thread::sleep(std::time::Duration::from_secs(2));
        }

        eprintln!(
            "[SSE] >>> sending to LLM [{}] (mode: {:?}, session: {})",
            chrono::Local::now().format("%H:%M:%S%.3f"),
            request.mode,
            request.session_id,
        );
        let mut stream = match client
            .chat_stream(
                &request.text,
                request.voice_hint.as_deref(),
                request.context.as_deref(),
                Some(&request.session_id),
            )
            .await
        {
            Ok(s) => s,
            Err(e) => {
                if attempt < max_retries {
                    continue;
                }
                return Err(e);
            }
        };
        let mut received_data = false;
        let mut reasoning_buffer = String::new();

        while let Some(chunk) = stream.next().await {
            if request.turn_gen != TURN_GENERATION.load(Ordering::SeqCst) {
                eprintln!("[SSE] turn superseded (gen={}, current={}) — exiting loop",
                    request.turn_gen, TURN_GENERATION.load(Ordering::SeqCst));
                return Ok(full_response);
            }
            match chunk {
                Ok(StreamEvent::Content(delta)) => {
                    if !reasoning_buffer.is_empty() {
                        let _ = app.emit(&evp("thinking"), reasoning_buffer.clone());
                        reasoning_buffer.clear();
                    }
                    if !received_data {
                        eprintln!("[SSE] <<< first token from LLM [{}]", chrono::Local::now().format("%H:%M:%S%.3f"));
                    }
                    received_data = true;
                    full_response.push_str(&delta);

                    // UI mode: emit chat-stream delta so frontend can display text
                    // in real-time while TTS plays in parallel via on_sentence.
                    // Bridge mode skips this — it uses bridge-* events and push_to_self.
                    if request.mode == HermesTurnMode::Ui {
                        let _ = app.emit("chat-stream", ChatStreamPayload { delta: delta.clone() });
                    }

                    // Stream TTS: split into sentences as they arrive
                    if let Some(ref cb) = request.on_sentence {
                        sentence_buffer.push_str(&delta);
                        let (sentences, leftover) = split_sentences(&sentence_buffer);
                        for s in &sentences {
                            cb(s);
                        }
                        sentence_buffer = leftover;
                    }
                }
                Ok(StreamEvent::Reasoning(text)) => {
                    if !received_data {
                        eprintln!("[SSE] <<< first reasoning from LLM [{}]: {}...", chrono::Local::now().format("%H:%M:%S%.3f"), &text[..text.len().min(60)]);
                    }
                    received_data = true;
                    reasoning_buffer.push_str(&text);
                    if reasoning_buffer.contains('\n') || reasoning_buffer.len() >= 60 {
                        let _ = app.emit(&evp("thinking"), reasoning_buffer.clone());
                        reasoning_buffer.clear();
                    }
                }
                Ok(StreamEvent::ToolCallStart { id, name }) => {
                    if !reasoning_buffer.is_empty() {
                        let _ = app.emit(&evp("thinking"), reasoning_buffer.clone());
                        reasoning_buffer.clear();
                    }
                    if !received_data {
                        eprintln!("[SSE] <<< first tool call from LLM [{}]: {}", chrono::Local::now().format("%H:%M:%S%.3f"), name);
                    }
                    received_data = true;
                    let _ = app.emit(&evp("tool-call"), serde_json::json!({
                        "id": id,
                        "name": name
                    }).to_string());
                }
                Err(e) => {
                    if !received_data && attempt < max_retries {
                        eprintln!("[SSE] no data received, will retry");
                        break;
                    }
                    return Err(e);
                }
            }
        }

        // Flush remaining sentence_buffer
        if let Some(ref cb) = request.on_sentence {
            let trimmed = sentence_buffer.trim();
            if !trimmed.is_empty() {
                eprintln!("[SSE] flushing last sentence ({} chars)", trimmed.len());
                cb(trimmed);
            }
            // No more sentences will be submitted for this generation — chat-audio-done
            // can now fire once the already-submitted sentences finish playing (or
            // immediately below, if they already have).
            SUBMISSION_DONE.store(true, Ordering::SeqCst);
            maybe_emit_audio_done(app, AUDIO_GENERATION.load(Ordering::SeqCst));
        }

        eprintln!("[SSE] <<< stream complete [{}] ({} chars)", chrono::Local::now().format("%H:%M:%S%.3f"), full_response.len());
        break;
    }

    let full_response = if std::env::var("ENABLE_LOCAL_COMMANDS").as_deref() == Ok("true") {
        execute_commands(&full_response)
    } else {
        strip_cmd_tags(&full_response)
    };

    Ok(full_response)
}

pub async fn dispatch_bridge_message(
    app: AppHandle,
    source: String,
    session_id: String,
    text: String,
    context: Option<String>,
    show_thinking: bool,
) -> Result<(), String> {
    let api_key = get_api_key();
    let api_agent = get_api_agent();
    let client = HermesClient::new(&get_api_url(), api_key, api_agent);
    let bridge_session = build_bridge_session_key(&source, &session_id);

    if show_thinking {
        app.emit("bridge-thinking-start", ()).map_err(|e| e.to_string())?;
    }

    let my_turn_gen = TURN_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;

    let request = HermesTurnRequest {
        text,
        session_id: bridge_session,
        voice_hint: None,
        context,
        mode: HermesTurnMode::Bridge,
        on_sentence: None,
        turn_gen: my_turn_gen,
    };

    match run_hermes_turn(&app, &client, &request).await {
        Ok(full_response) => {
            let superseded = my_turn_gen != TURN_GENERATION.load(Ordering::SeqCst);
            if superseded {
                // A newer turn (or discard_pending_turn) has bumped past us.
                // The new turn will emit its own terminal events — stay silent.
                return Ok(());
            }
            if full_response.trim().is_empty() {
                let _ = app.emit("bridge-turn-finished", ());
                return Ok(());
            }
            // Push response to internal /push endpoint to trigger TTS + display
            let cleaned = strip_all_cmd_tags(&full_response);
            if cleaned.is_empty() {
                let _ = app.emit("bridge-turn-finished", ());
                return Ok(());
            }
            let emotion = detect_emotion(&cleaned);
            push_to_self(&cleaned, &emotion, "zh-CN-XiaoxiaoNeural");
            Ok(())
        }
        Err(e) => {
            if my_turn_gen != TURN_GENERATION.load(Ordering::SeqCst) {
                return Ok(());
            }
            let _ = app.emit("bridge-turn-error", e.clone());
            Err(e)
        }
    }
}

#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    state: State<'_, AppState>,
    text: String,
    _tts_format: Option<String>,
    tts_primary_voice: Option<String>,
    tts_aux1_voice: Option<String>,
    tts_aux2_voice: Option<String>,
    user_language: Option<String>,
    fixed_lang: Option<String>,
    tts_enabled: Option<bool>,
) -> Result<(), String> {
    let primary = tts_primary_voice.unwrap_or_else(|| "zh-CN-XiaoxiaoNeural".to_string());
    let aux1 = tts_aux1_voice.unwrap_or_default();
    let aux2 = tts_aux2_voice.unwrap_or_default();
    let api_key = get_api_key();
    let api_agent = get_api_agent();
    let client = HermesClient::new(&get_api_url(), api_key, api_agent);

    let user_lang = user_language.unwrap_or_else(|| "zh".to_string());
    let fixed = fixed_lang.unwrap_or_default();
    let mut hint = build_voice_hint(&primary, &aux1, &aux2, &user_lang, &fixed);
    if fixed.is_empty() {
        hint.push_str("

IMPORTANT: You MUST respond in the SAME language the user writes in. If the user writes in Chinese, respond in Chinese. If the user writes in English, respond in English. Never switch languages based on previous conversation context.");
    }

    let text = build_ui_turn_text(&text, &primary, &aux1, &aux2, &fixed);

    let my_turn_gen = TURN_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;

    app.emit("chat-thinking-start", ()).map_err(|e| e.to_string())?;

    let base_id = state.session_id.lock().unwrap().clone();
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let session_id = format!("{}-{}", base_id, today);

    let yesterday = (chrono::Local::now() - chrono::TimeDelta::days(1)).format("%Y-%m-%d").to_string();
    let summary_path = format!("{}/.hermes/pa-summaries/{}.md",
        std::env::var("HOME").unwrap_or_default(), yesterday);
    let daily_summary = std::fs::read_to_string(&summary_path).ok();
    if daily_summary.is_some() {
        eprintln!("[SSE] loaded daily summary from {}", summary_path);
    }

    // Streaming TTS: sentence-level callback
    let tts_on = tts_enabled.unwrap_or(true);

    // When TTS is on, emit speaking-start early so frontend initializes typewriter
    // and stream state BEFORE chat-stream deltas arrive from run_hermes_turn.
    // When TTS is off, emit_text_without_tts handles the full lifecycle below.
    if tts_on {
        let _ = app.emit("chat-speaking-start", TypewriterStartPayload {
            emotion: "friendly".to_string(),
            total_chars: 0,
            has_audio: true,
        });
    }

    let speak_generation = AUDIO_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    begin_audio_submission();
    let speak_app = app.clone();
    let speak_format = "wav".to_string();

    // Track the first sentence's voice so subsequent sentences use the same voice.
    // This ensures consistency: if sentence 1 is Chinese, sentence 2 (even if English URL)
    // will still be read with the Chinese voice.
    let sentence_voice: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

    let on_sentence: Option<std::sync::Arc<dyn Fn(&str) + Send + Sync>> = if tts_on {
        Some(std::sync::Arc::new(move |sentence: &str| {
            // Strip [CMD:...] tags — they won't appear in chat box, so don't read them.
            let clean_sentence = strip_all_cmd_tags(sentence);
            if clean_sentence.is_empty() { return; }

            // Use first sentence's voice for all subsequent sentences.
            // Only detect voice on the first non-empty sentence.
            let voice = {
                let mut guard = sentence_voice.lock().unwrap();
                if guard.is_none() {
                    *guard = Some(select_voice(&clean_sentence, &primary, &aux1, &aux2, &fixed, &user_lang));
                }
                guard.clone().unwrap()
            };

            let emotion = detect_emotion(&clean_sentence);
            speak_internal(&speak_app, &clean_sentence, &emotion, &voice, &speak_format, speak_generation, false);
        }))
    } else {
        None
    };

    let request = HermesTurnRequest {
        text,
        session_id,
        voice_hint: Some(hint),
        context: daily_summary,
        mode: HermesTurnMode::Ui,
        on_sentence,
        turn_gen: my_turn_gen,
    };

    let full_response = match run_hermes_turn(&app, &client, &request).await {
        Ok(response) => response,
        Err(e) => {
            // If a newer turn superseded us, swallow the error — the new turn owns the UI.
            if my_turn_gen != TURN_GENERATION.load(Ordering::SeqCst) {
                return Ok(());
            }
            app.emit("chat-stream-error", e.clone()).map_err(|emit_err| emit_err.to_string())?;
            return Err(e);
        }
    };

    let was_interrupted = my_turn_gen != TURN_GENERATION.load(Ordering::SeqCst);

    if was_interrupted {
        // The new turn will emit its own chat-stream-end / chat-stream-error.
        // Save the partial response (if any) with the interrupted marker so history stays coherent.
        let trimmed = full_response.trim();
        let content = if trimmed.is_empty() {
            "[已被用户打断，无回复]".to_string()
        } else {
            format!("{} [已被用户打断]", trimmed)
        };
        if let Err(e) = super::history::save_message("assistant", &content) {
            eprintln!("[history] failed to save interrupted message: {}", e);
        }
        return Ok(());
    }

    if full_response.trim().is_empty() {
        let _ = app.emit("chat-stream-end", ());
        return Ok(());
    }

    if let Err(e) = super::history::save_message("assistant", &full_response) {
        eprintln!("[history] failed to save assistant message: {}", e);
    }

    // TTS on: chat-stream deltas were emitted during run_hermes_turn, now signal end.
    // TTS off: emit_text_without_tts handles speaking-start + chat-stream + chat-stream-end.
    if tts_on {
        let _ = app.emit("chat-stream-end", ());
    } else {
        let emotion = detect_emotion(&full_response);
        emit_text_without_tts(&app, &full_response, &emotion);
    }

    Ok(())
}

#[tauri::command]
pub fn discard_pending_turn() {
    TURN_GENERATION.fetch_add(1, Ordering::SeqCst);
    AUDIO_GENERATION.fetch_add(1, Ordering::SeqCst);
}

/// Reset the hotkey state machine's press/release toggle. Called by the frontend
/// when the user cancels the break-confirmation popup, so the next hotkey press is
/// interpreted as "start" instead of "stop".
#[tauri::command]
pub fn reset_hotkey_active_state() {
    crate::voice::hotkey::reset_active_state();
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

    let tts_enabled = tts_enabled.unwrap_or(true);
    if !tts_enabled {
        // TTS disabled: show typewriter effect and speaking animation, but don't generate audio
        let emotion_str = emotion.unwrap_or_else(|| detect_emotion(&text));
        let _ = app.emit("chat-speaking-start", TypewriterStartPayload {
            emotion: emotion_str,
            total_chars: text.chars().count(),
            has_audio: false,
        });
        let _ = app.emit("chat-stream", ChatStreamPayload { delta: text.clone() });
        let _ = app.emit("chat-stream-end", ());
        return Ok(());
    }

    let format = tts_format.unwrap_or_else(|| "wav".to_string());
    let primary = tts_primary_voice.unwrap_or_else(|| "zh-CN-XiaoxiaoNeural".to_string());
    let aux1 = tts_aux1_voice.unwrap_or_default();
    let aux2 = tts_aux2_voice.unwrap_or_default();
    let generation = AUDIO_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    begin_audio_submission();

    let voice = override_voice.unwrap_or_else(|| select_voice(&text, &primary, &aux1, &aux2, "", ""));
    let emotion_str = emotion.unwrap_or_else(|| detect_emotion(&text));

    speak_internal(&app, &text, &emotion_str, &voice, &format, generation, true);
    // Single-shot: the one (and only) sentence has already been submitted above.
    SUBMISSION_DONE.store(true, Ordering::SeqCst);
    maybe_emit_audio_done(&app, generation);

    Ok(())
}

/// Play a voice sample from the settings window without emitting chat events
/// or changing the saved configuration.
#[tauri::command]
pub async fn preview_voice(
    voice: String,
    tts_format: Option<String>,
    volume: Option<f32>,
) -> Result<(), String> {
    if voice.is_empty()
        || !voice.ends_with("Neural")
        || !voice.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err("invalid preview voice".to_string());
    }
    let format = tts_format.unwrap_or_else(|| "wav".to_string());
    if !matches!(format.as_str(), "wav" | "mp3") {
        return Err("invalid preview audio format".to_string());
    }
    let playback_volume = volume.unwrap_or(0.8).clamp(0.0, 1.0);

    tauri::async_runtime::spawn_blocking(move || {
        let text = if voice.starts_with("zh-HK-") {
            "你好，我係你嘅語音智能助手，好高興可以同你一齊交流。"
        } else if voice.starts_with("zh-TW-") {
            "你好，我是你的語音智慧助理，很高興能和你一起交流。"
        } else if voice.starts_with("zh-") {
            "你好，我是你的语音智能助手，很高兴能和你一起交流。"
        } else if voice.starts_with("ja-") {
            "こんにちは、私はあなたの音声AIアシスタントです。あなたとお話しできてうれしいです。"
        } else if voice.starts_with("ko-") {
            "안녕하세요. 저는 당신의 음성 인공지능 비서입니다. 함께 이야기하게 되어 기쁩니다."
        } else if voice.starts_with("fr-") {
            "Bonjour, je suis votre assistant vocal intelligent. Je suis ravi de pouvoir échanger avec vous."
        } else if voice.starts_with("de-") {
            "Hallo, ich bin Ihr intelligenter Sprachassistent. Ich freue mich, mit Ihnen zu sprechen."
        } else if voice.starts_with("es-") {
            "Hola, soy tu asistente de voz inteligente. Me alegra mucho poder hablar contigo."
        } else {
            "Hello, I'm your intelligent voice assistant. I'm very happy to talk with you."
        };
        let path = tts_path(&format);
        if !generate_tts_to(text, &path, &voice, "+0%", "+0%") {
            return Err("voice preview generation failed".to_string());
        }

        let (stream, stream_handle) =
            rodio::OutputStream::try_default().map_err(|e| format!("audio output: {e}"))?;
        let sink = rodio::Sink::try_new(&stream_handle)
            .map_err(|e| format!("audio preview sink: {e}"))?;
        sink.set_volume(playback_volume);
        let file = std::fs::File::open(&path)
            .map_err(|e| format!("open voice preview: {e}"))?;
        let source = rodio::Decoder::new(std::io::BufReader::new(file))
            .map_err(|e| format!("decode voice preview: {e}"))?;
        sink.append(source);
        sink.sleep_until_end();
        drop(stream);
        let _ = std::fs::remove_file(path);
        Ok(())
    })
    .await
    .map_err(|e| format!("voice preview task failed: {e}"))?
}

/// Speak a short status announcement (e.g. "正在思考", "查询 weather").
/// Differs from `speak_text`:
///   - Does NOT emit chat-speaking-start/chat-stream/chat-stream-end, so the
///     text never appears in the chat box (status stays in the StatusPanel only).
///   - Does NOT bump AUDIO_GENERATION; uses the current one so the in-flight
///     response audio captured by `send_message` is not invalidated.
///   - Non-blocking: drops the announcement if the queue is full.
#[tauri::command]
pub async fn speak_status(
    app: AppHandle,
    text: String,
    override_voice: Option<String>,
    tts_format: Option<String>,
    tts_primary_voice: Option<String>,
    tts_aux1_voice: Option<String>,
    tts_aux2_voice: Option<String>,
    tts_enabled: Option<bool>,
) -> Result<(), String> {
    if text.trim().is_empty() { return Ok(()); }
    if !tts_enabled.unwrap_or(true) { return Ok(()); }

    let format = tts_format.unwrap_or_else(|| "wav".to_string());
    let primary = tts_primary_voice.unwrap_or_else(|| "zh-CN-XiaoxiaoNeural".to_string());
    let aux1 = tts_aux1_voice.unwrap_or_default();
    let aux2 = tts_aux2_voice.unwrap_or_default();

    let generation = AUDIO_GENERATION.load(Ordering::SeqCst);

    if !audio_queue_reserve() {
        eprintln!("[STATUS-TTS] queue full, skipping {} chars", text.chars().count());
        return Ok(());
    }

    let voice = override_voice
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| select_voice(&text, &primary, &aux1, &aux2, "", ""));
    let _ = audio_sender().lock().unwrap().send(AudioCmd::Speak {
        text: text.clone(),
        emotion: "neutral".to_string(),
        voice,
        format,
        app: app.clone(),
        generation,
        show_text: false,
        silent: true,
    });

    Ok(())
}

/// Strip [CMD:...] tags from text without executing them.
fn strip_cmd_tags(text: &str) -> String {
    let re = regex::Regex::new(r#"\[CMD:[^\]]+\]"#).unwrap();
    let clean = re.replace_all(text, "").to_string();
    let space_re = regex::Regex::new(r"  +").unwrap();
    space_re.replace_all(&clean.trim(), " ").to_string()
}

/// Comprehensive CMD filter for streaming deltas and TTS input.
/// Covers: [LOCAL_CMD ...], [CMD:...], residual "CMD]...", and
/// orphaned lines starting with executing:/OK/ERROR/FAILED.
/// Returns cleaned text with multi-space collapsed.
fn strip_all_cmd_tags(text: &str) -> String {
    let full_re = regex::Regex::new(r#"\[LOCAL_CMD[\s\S]*?\]"#).unwrap();
    let cmd_re = regex::Regex::new(r#"\[CMD:[^\]]*\]"#).unwrap();
    let resid_re = regex::Regex::new(r#"CMD\][^\n]*"#).unwrap();
    let kw_re = regex::Regex::new(r#"(?m)^(executing:|OK|ERROR|FAILED)\s[^\n]*\n?"#).unwrap();

    let s = full_re.replace_all(text, "").to_string();
    let s = cmd_re.replace_all(&s, "").to_string();
    let s = resid_re.replace_all(&s, "").to_string();
    let s = kw_re.replace_all(&s, "").to_string();

    let space_re = regex::Regex::new(r"  +").unwrap();
    let blank_re = regex::Regex::new(r"\n{3,}").unwrap();
    let s = space_re.replace_all(&s, " ").to_string();
    let s = blank_re.replace_all(&s, "\n\n").to_string();
    s.trim().to_string()
}

/// Extract [CMD:...] tags from text, execute them, return text with tags removed.
///
/// Commands are executed in detached threads to avoid blocking the async runtime.
/// std::process::Command::output() waits for stdout/stderr pipes to close - GUI apps
/// like Chrome inherit the pipe write-end and never close it, which blocks the tokio
/// worker thread and freezes the UI (chat-stream-end never emits, PA stuck in SPEAKING).
fn execute_commands(text: &str) -> String {
    let re = regex::Regex::new(r#"\[CMD:([^\]]+)\]"#).unwrap();

    for cap in re.captures_iter(text) {
        let cmd_str = cap[1].to_string();
        eprintln!("[LOCAL_CMD] dispatching: {}", cmd_str);

        // Spawn a detached thread so .output() cannot block the async runtime.
        if let Err(e) = std::thread::Builder::new()
            .name("local-cmd".to_string())
            .spawn(move || {
                let result = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(&cmd_str)
                    .output();

                match result {
                    Ok(output) => {
                        if output.status.success() {
                            eprintln!("[LOCAL_CMD] OK: {}", cmd_str);
                        } else {
                            eprintln!("[LOCAL_CMD] exit={}: {}", output.status, String::from_utf8_lossy(&output.stderr));
                        }
                    }
                    Err(e) => eprintln!("[LOCAL_CMD] error: {}", e),
                }
            })
        {
            eprintln!("[LOCAL_CMD] thread spawn failed: {}", e);
        }
    }

    let clean = re.replace_all(text, "").to_string();
    let space_re = regex::Regex::new(r"  +").unwrap();
    space_re.replace_all(&clean.trim(), " ").to_string()
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_session_keys_are_namespaced() {
        assert_eq!(build_bridge_session_key("chess-app", "game-001"), "bridge:chess-app:game-001");
    }

    #[test]
    fn ui_turn_text_appends_detected_language_instruction() {
        let text = build_ui_turn_text(
            "hello world",
            "zh-CN-XiaoxiaoNeural",
            "",
            "",
            "",
        );
        assert_eq!(text, "hello world Please reply in English.");
    }
}
