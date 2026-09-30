// sherpa_wake.rs — Streaming local KWS with optional owner voice verification
// Audio is sent as ordered 200 ms PCM blocks to the local Python KWS runtime.
// Keyword detection and selected-speaker verification must both pass when
// owner-only mode is enabled. Whisper is reserved for post-wake dictation.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use base64::Engine;

use crate::voice::record::{start_streaming_capture, stop_streaming_capture, OWNER_WAKE};

/// STT server URL for HTTP-based speaker embedding (bypasses macOS 12 ORT C API mismatch).
const STT_SERVER_URL: &str = "http://127.0.0.1:8651";

// ── Constants ────────────────────────────────────────────────────────────────

const TARGET_SR: u32 = 16000;
const READ_TIMEOUT_MS: u64 = 50;

// Cooldown after a detection (avoid re-trigger on same utterance)


// ── Paths ────────────────────────────────────────────────────────────────────


pub fn voiceprints_dir() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".pocket-agent")
        .join("voiceprints")
}

// ── Global state ─────────────────────────────────────────────────────────────

static WAKE_ACTIVE: AtomicBool = AtomicBool::new(false);
static WAKE_PAUSED: AtomicBool = AtomicBool::new(false);

struct WakeState {
    stop_flag: Arc<AtomicBool>,
    worker: JoinHandle<()>,
}

fn wake_state_slot() -> &'static Mutex<Option<WakeState>> {
    static SLOT: OnceLock<Mutex<Option<WakeState>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

pub fn is_wake_active() -> bool {
    WAKE_ACTIVE.load(Ordering::Acquire)
}

pub fn pause_wake() {
    WAKE_PAUSED.store(true, Ordering::Release);
}

pub fn resume_wake() {
    WAKE_PAUSED.store(false, Ordering::Release);
}


// ── Cosine similarity ────────────────────────────────────────────────────────

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a > 0.0 && norm_b > 0.0 {
        dot / (norm_a * norm_b)
    } else {
        0.0
    }
}

// ── RMS helper (mirrors conversation.rs) ──────────────────────────────────────────────

/// Downmix multi-channel samples to mono.
fn downmix_to_mono(samples: &[i16], channels: u16) -> Vec<i16> {
    if channels <= 1 {
        return samples.to_vec();
    }
    let ch = channels as usize;
    samples
        .chunks_exact(ch)
        .map(|c| {
            let sum: i32 = c.iter().map(|&s| s as i32).sum();
            (sum / ch as i32) as i16
        })
        .collect()
}

/// Resample mono samples to 16kHz via linear interpolation.
fn resample_to_16k(mono: &[i16], source_sr: u32) -> Vec<i16> {
    if source_sr == TARGET_SR || mono.is_empty() {
        return mono.to_vec();
    }
    let ratio = source_sr as f64 / TARGET_SR as f64;
    let out_len = ((mono.len() as f64) / ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = (i as f64) * ratio;
        let idx = pos.floor() as usize;
        let frac = (pos - pos.floor()) as f32;
        let v = if idx + 1 >= mono.len() {
            mono[mono.len() - 1] as f32
        } else {
            mono[idx] as f32 * (1.0 - frac) + mono[idx + 1] as f32 * frac
        };
        out.push(v.round().clamp(-32768.0, 32767.0) as i16);
    }
    out
}

// ── Start / Stop ─────────────────────────────────────────────────────────────

pub fn start_wake_listener(app: AppHandle, _threshold: f32, _speaker_name: Option<String>) -> Result<(), String> {
    if WAKE_ACTIVE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("wake listener already active".into());
    }

    let setup = (|| {
        let config = crate::commands::settings_repository::load()?;
        let config = kws_config(&config);
        let client = kws_client(5)?;
        let session = kws_open(&client, &config)?;
        Ok::<_, String>((config, client, session))
    })();
    let (config, client, session) = match setup {
        Ok(v) => v,
        Err(e) => { WAKE_ACTIVE.store(false, Ordering::Release); return Err(e); }
    };
    let (tx, rx) = mpsc::sync_channel::<Vec<i16>>(16);
    let overflow = Arc::new(AtomicBool::new(false));
    let overflow_capture = overflow.clone();
    let stream_handle = match start_streaming_capture(OWNER_WAKE, move |samples, _sr| {
        if tx.try_send(samples.to_vec()).is_err() { overflow_capture.store(true, Ordering::Release); }
    }) {
        Ok(h) => h,
        Err(e) => {
            let _ = client.delete(format!("{}/kws/session/{}", STT_SERVER_URL, session)).send();
            WAKE_ACTIVE.store(false, Ordering::Release);
            return Err(format!("mic capture: {}", e));
        }
    };

    let device_sr = stream_handle.sample_rate;
    let device_ch = stream_handle.channels;
    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_flag_worker = stop_flag.clone();
    let app_worker = app.clone();

    let spawn = std::thread::Builder::new()
        .name("wake-worker".into())
        .spawn(move || {
            let result = wake_http_worker_loop(
                &app_worker,
                &rx,
                device_sr,
                device_ch,
                &stop_flag_worker,
                config, client, session, overflow,
            );
            stop_streaming_capture(stream_handle);
            WAKE_ACTIVE.store(false, Ordering::Release);
            WAKE_PAUSED.store(false, Ordering::Release);
            match result {
                Ok(()) => {
                    eprintln!("[wake] listener exited normally");
                    let _ = app_worker.emit("wake-listener-stopped", ());
                }
                Err(e) => {
                    eprintln!("[wake] listener error: {}", e);
                    let _ = app_worker
                        .emit("wake-listener-error", serde_json::json!({ "error": e }));
                }
            }
        });

    let worker = match spawn {
        Ok(h) => h,
        Err(e) => {
            WAKE_ACTIVE.store(false, Ordering::Release);
            return Err(format!("worker spawn: {}", e));
        }
    };

    {
        let mut slot = wake_state_slot().lock().unwrap_or_else(|p| p.into_inner());
        *slot = Some(WakeState { stop_flag, worker });
    }

    let _ = app.emit("wake-listener-started", ());
    eprintln!("[wake] local KWS listener started");
    Ok(())
}

pub fn stop_wake_listener() {
    if !WAKE_ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let state = {
        let mut slot = wake_state_slot().lock().unwrap_or_else(|p| p.into_inner());
        slot.take()
    };
    if let Some(state) = state {
        state.stop_flag.store(true, Ordering::Release);
        let _ = state.worker.join();
        // The audio-streaming thread is detached — it releases CAPTURE_OWNER
        // asynchronously after the stop signal.  Spin-wait briefly so callers
        // (e.g. start_enroll_recording) can immediately re-acquire capture.
        use crate::voice::record::{current_owner, OWNER_NONE};
        for _ in 0..100 {
            if current_owner() == OWNER_NONE {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

/// A persistent HTTP client feeds ordered PCM blocks into a dedicated KWS stream.
fn wake_http_worker_loop(
    app: &AppHandle,
    rx: &mpsc::Receiver<Vec<i16>>,
    device_sr: u32,
    device_ch: u16,
    stop_flag: &AtomicBool,
    config: serde_json::Value,
    client: reqwest::blocking::Client,
    mut session: String,
    overflow: Arc<AtomicBool>,
) -> Result<(), String> {
    let result = (|| {
        let mut pcm = Vec::new();
        let mut sequence = 0u64;
        let mut was_paused = false;
        let mut last_sent = std::time::Instant::now();
        while !stop_flag.load(Ordering::Acquire) {
            let paused = WAKE_PAUSED.load(Ordering::Acquire);
            let gap = overflow.swap(false, Ordering::AcqRel)
                || last_sent.elapsed() > Duration::from_secs(10);
            if paused {
                was_paused = true;
                pcm.clear();
                let _ = rx.recv_timeout(Duration::from_millis(READ_TIMEOUT_MS));
                continue;
            }
            if was_paused || gap {
                while rx.try_recv().is_ok() {}
                let _ = client.delete(format!("{}/kws/session/{}", STT_SERVER_URL, session)).send();
                session = kws_open(&client, &config)?;
                sequence = 0;
                pcm.clear();
                was_paused = false;
                last_sent = std::time::Instant::now();
            }
            match rx.recv_timeout(Duration::from_millis(READ_TIMEOUT_MS)) {
                Ok(raw) => pcm.extend(resample_to_16k(&downmix_to_mono(&raw, device_ch), device_sr)),
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return Err("mic channel disconnected".into()),
            }
            while pcm.len() >= 3200 {
                let block: Vec<i16> = pcm.drain(..3200).collect();
                let bytes: Vec<u8> = block.iter().flat_map(|s| s.to_le_bytes()).collect();
                let v = kws_response(client.post(format!("{}/kws/audio/{}?sequence={}", STT_SERVER_URL, session, sequence))
                    .header("content-type", "application/octet-stream").body(bytes).send())?;
                sequence += 1;
                last_sent = std::time::Instant::now();
                // Never apply results from audio submitted before a stop/pause.
                if stop_flag.load(Ordering::Acquire) || WAKE_PAUSED.load(Ordering::Acquire) || overflow.load(Ordering::Acquire) {
                    break;
                }
                if v["keyword_match"].as_bool() == Some(true) && v["speaker_match"].as_bool() == Some(true) {
                    // Suspend until the conversation releases the microphone.
                    WAKE_PAUSED.store(true, Ordering::Release);
                    crate::voice::hotkey::set_active_state(true);
                    let _ = app.emit("fn-key-down", ());
                    pcm.clear();
                    break;
                }
            }
        }
        Ok(())
    })();
    let _ = client.delete(format!("{}/kws/session/{}", STT_SERVER_URL, session)).send();
    result
}

pub fn kws_client(timeout: u64) -> Result<reqwest::blocking::Client, String> {
    let token = std::fs::read_to_string(voiceprints_dir().parent().unwrap().join("server.token"))
        .map_err(|_| "Voice service is not ready. Please wait and retry.".to_string())?;
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("authorization", format!("Bearer {}", token.trim()).parse().map_err(|_| "Invalid voice service token")?);
    headers.insert("origin", "tauri://localhost".parse().unwrap());
    reqwest::blocking::Client::builder().default_headers(headers)
        .timeout(Duration::from_secs(timeout)).build().map_err(|e| e.to_string())
}

fn kws_response(response: Result<reqwest::blocking::Response, reqwest::Error>) -> Result<serde_json::Value, String> {
    let response = response.map_err(|e| format!("Wake service: {e}"))?;
    let status = response.status();
    let text = response.text().map_err(|e| e.to_string())?;
    if !status.is_success() { return Err(format!("Wake service {status}: {text}")); }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

pub fn kws_config(config: &crate::commands::config::AppConfig) -> serde_json::Value {
    serde_json::json!({"phrase": config.wake_phrase, "kws_threshold": config.wake_kws_threshold,
        "owner_only": config.wake_owner_only, "speaker": config.last_enrolled_speaker,
        "speaker_threshold": config.wake_word_threshold})
}

fn kws_open(client: &reqwest::blocking::Client, config: &serde_json::Value) -> Result<String, String> {
    let v = kws_response(client.post(format!("{}/kws/session", STT_SERVER_URL)).json(config).send())?;
    v["session"].as_str().map(String::from).ok_or_else(|| "Missing wake session".into())
}

pub fn kws_model_action(install: bool) -> Result<serde_json::Value, String> {
    let client = kws_client(if install { 180 } else { 5 })?;
    if install {
        kws_response(client.post(format!("{}/kws/install", STT_SERVER_URL)).body("").send())
    } else {
        kws_response(client.get(format!("{}/kws/status", STT_SERVER_URL)).send())
    }
}

pub fn test_wake_audio(path: &str, config: &crate::commands::config::AppConfig) -> Result<serde_json::Value, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut reader = hound::WavReader::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    let samples: Vec<i16> = reader.samples::<i16>().collect::<Result<_, _>>().map_err(|e| e.to_string())?;
    let samples = resample_to_16k(&downmix_to_mono(&samples, spec.channels), spec.sample_rate);
    kws_check_samples(&samples, &kws_config(config))
}

fn kws_check_samples(samples: &[i16], config: &serde_json::Value) -> Result<serde_json::Value, String> {
    let part = reqwest::blocking::multipart::Part::bytes(i16_to_wav(samples, TARGET_SR))
        .file_name("wake.wav").mime_str("audio/wav").map_err(|e| e.to_string())?;
    let form = reqwest::blocking::multipart::Form::new().part("file", part).text("config", config.to_string());
    kws_response(kws_client(10)?.post(format!("{}/kws/check", STT_SERVER_URL)).multipart(form).send())
}

#[derive(Debug)]
struct WakeCheckResult {
    speaker_match: bool,
    keyword_match: bool,
    keyword_text: String,
}

/// Reuse enrolled keyword AND speaker verification during playback. Errors fail
/// closed: generic speech, noise or an unavailable service cannot interrupt.
/// Speaker verification reduces speaker echo false positives; it is not AEC.
pub fn check_interrupt_wake(samples: &[i16], sr: u32, threshold: f32) -> Result<bool, String> {
    let result = wake_http_check(samples, sr, 1, threshold)?;
    Ok(result.speaker_match && result.keyword_match && !result.keyword_text.trim().is_empty())
}

/// Send audio buffer to Python /wake/check endpoint.
fn wake_http_check(
    raw_samples: &[i16],
    device_sr: u32,
    device_ch: u16,
    threshold: f32,
) -> Result<WakeCheckResult, String> {
    let config = crate::commands::settings_repository::load()?;
    let mono = downmix_to_mono(raw_samples, device_ch);
    let samples = resample_to_16k(&mono, device_sr);
    let mut config = kws_config(&config);
    config["speaker_threshold"] = serde_json::json!(threshold);
    let v = kws_check_samples(&samples, &config)?;
    Ok(WakeCheckResult {
        speaker_match: v["speaker_match"].as_bool().unwrap_or(false),
        keyword_match: v["keyword_match"].as_bool().unwrap_or(false),
        keyword_text: v["keyword_text"].as_str().unwrap_or("").to_string(),
    })
}

/// Build minimal WAV bytes from mono i16 samples.
fn i16_to_wav(samples: &[i16], sample_rate: u32) -> Vec<u8> {
    let data_len = samples.len() * 2;
    let mut wav = Vec::with_capacity(44 + data_len);
    // RIFF header
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    // fmt chunk
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // chunk size
    wav.extend_from_slice(&1u16.to_le_bytes());  // PCM
    wav.extend_from_slice(&1u16.to_le_bytes());  // mono
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    let byte_rate = sample_rate * 2; // 16-bit mono
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());  // block align
    wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    // data chunk
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data_len as u32).to_le_bytes());
    for &s in samples {
        wav.extend_from_slice(&s.to_le_bytes());
    }
    wav
}

//

//

//

// ── Speaker enrollment / verification ────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct EnrollResult {
    pub ok: bool,
    pub speaker_id: String,
    pub duration_s: f32,
    pub wake_text: String,
}

#[derive(Debug, Serialize)]
pub struct VerifyResult {
    pub verified: bool,
    pub speaker: Option<String>,
    pub confidence: f32,
}

#[derive(Debug, Serialize)]
pub struct SpeakerInfo {
    pub name: String,
    pub enrolled_at: String,
}



/// Read WAV duration from header without decoding the whole file.
fn wav_duration_from_header(path: &str) -> f32 {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else { return 0.0 };
    let mut header = [0u8; 44];
    if f.read_exact(&mut header).is_err() { return 0.0; }
    let channels = u16::from_le_bytes([header[22], header[23]]) as u32;
    let sample_rate = u32::from_le_bytes([header[24], header[25], header[26], header[27]]);
    let bits_per_sample = u16::from_le_bytes([header[34], header[35]]) as u32;
    let data_size = u32::from_le_bytes([header[40], header[41], header[42], header[43]]);
    let bytes_per_frame = channels * (bits_per_sample / 8);
    if sample_rate == 0 || bytes_per_frame == 0 { return 0.0; }
    data_size as f32 / (sample_rate as f32 * bytes_per_frame as f32)
}

/// Extract speaker embedding via HTTP to the Python STT server.
/// Bypasses the native sherpa-onnx C API which has an ORT version mismatch on macOS 12.
fn extract_embedding_http(wav_path: &str) -> Result<Vec<f32>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let file_bytes = std::fs::read(wav_path)
        .map_err(|e| format!("读取 WAV 失败: {}", e))?;

    let file_name = std::path::PathBuf::from(wav_path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let part = reqwest::blocking::multipart::Part::bytes(file_bytes)
        .file_name(file_name)
        .mime_str("audio/wav")
        .map_err(|e| format!("mime error: {}", e))?;

    let form = reqwest::blocking::multipart::Form::new().part("file", part);

    let resp = client
        .post(format!("{}/speaker/embed", STT_SERVER_URL))
        .multipart(form)
        .send()
        .map_err(|e| format!("embedding HTTP 请求失败: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(format!("stt-server /speaker/embed 返回 {}: {}", status, body));
    }

    let v: serde_json::Value = resp.json()
        .map_err(|e| format!("解析 JSON 失败: {}", e))?;

    let emb_b64 = v["embedding"].as_str().ok_or("missing embedding field")?;
    let emb_bytes = base64::engine::general_purpose::STANDARD
        .decode(emb_b64)
        .map_err(|e| format!("base64 decode failed: {}", e))?;

    let embedding: Vec<f32> = emb_bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();

    Ok(embedding)
}

/// Enroll voice identity; explicit KWS settings define the wake phrase.
pub fn enroll_speaker(name: &str, wav_path: &str) -> Result<EnrollResult, String> {
    validate_speaker_name(name)?;
    let duration_s = wav_duration_from_header(wav_path);

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("enroll HTTP client: {}", e))?;

    let file_bytes = std::fs::read(wav_path)
        .map_err(|e| format!("read WAV: {}", e))?;

    let file_name = std::path::PathBuf::from(wav_path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let file_part = reqwest::blocking::multipart::Part::bytes(file_bytes)
        .file_name(file_name)
        .mime_str("audio/wav")
        .map_err(|e| format!("mime: {}", e))?;

    let form = reqwest::blocking::multipart::Form::new()
        .part("file", file_part)
        .text("name", name.to_string());

    let resp = client
        .post(format!("{}/speaker/enroll", STT_SERVER_URL))
        .multipart(form)
        .send()
        .map_err(|e| format!("enroll HTTP: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(format!("enroll server returned {}: {}", status, body));
    }

    let _v: serde_json::Value = resp.json()
        .map_err(|e| format!("enroll JSON: {}", e))?;

    eprintln!(
        "[sherpa] enrolled '{}' (dur={:.1}s) — voice identity saved",
        name, duration_s
    );

    Ok(EnrollResult {
        ok: true,
        speaker_id: name.to_string(),
        duration_s,
        wake_text: String::new(),
    })
}


/// Training mode: append wake keyword variant via Python STT server.
/// Unlike enroll (which resets variants), this only appends a new variant.
pub fn train_speaker(name: &str, wav_path: &str) -> Result<EnrollResult, String> {
    validate_speaker_name(name)?;
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("train HTTP client: {}", e))?;

    let file_bytes = std::fs::read(wav_path)
        .map_err(|e| format!("read WAV: {}", e))?;

    let file_name = std::path::PathBuf::from(wav_path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let file_part = reqwest::blocking::multipart::Part::bytes(file_bytes)
        .file_name(file_name)
        .mime_str("audio/wav")
        .map_err(|e| format!("mime: {}", e))?;

    let form = reqwest::blocking::multipart::Form::new()
        .part("file", file_part)
        .text("name", name.to_string());

    let resp = client
        .post(format!("{}/speaker/train", STT_SERVER_URL))
        .multipart(form)
        .send()
        .map_err(|e| format!("train HTTP: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(format!("train server returned {}: {}", status, body));
    }

    let v: serde_json::Value = resp.json()
        .map_err(|e| format!("train JSON: {}", e))?;
    let wake_text = v["wake_text"].as_str().unwrap_or_default().trim();
    if v["ok"].as_bool() != Some(true)
        || wake_text.is_empty()
        || v["variant_count"].as_u64().unwrap_or(0) == 0
    {
        return Err("No usable wake phrase recognized. Please record again.".to_string());
    }

    eprintln!("[sherpa] train saved variant for '{}': '{}'", name, wake_text);

    Ok(EnrollResult {
        ok: true,
        speaker_id: name.to_string(),
        duration_s: 0.0,
        wake_text: wake_text.to_string(),
    })
}


/// Return the number of wake keyword variants for a speaker.
pub fn get_wake_variant_count(name: &str) -> usize {
    let vp_dir = voiceprints_dir();
    let wake_txt = vp_dir.join(format!("{}.wake.txt", name));
    if !wake_txt.exists() {
        return 0;
    }
    let Ok(data) = std::fs::read_to_string(&wake_txt) else { return 0 };
    if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(&data) {
        arr.len()
    } else {
        0
    }
}

/// Return the wake keyword strings for a speaker.
pub fn get_wake_words(name: &str) -> Vec<String> {
    let vp_dir = voiceprints_dir();
    let wake_txt = vp_dir.join(format!("{}.wake.txt", name));
    if !wake_txt.exists() {
        return Vec::new();
    }
    let Ok(data) = std::fs::read_to_string(&wake_txt) else { return Vec::new() };
    serde_json::from_str::<Vec<String>>(&data).unwrap_or_default()
}

/// Remove one wake keyword for a speaker; returns the remaining list.
pub fn remove_wake_word(name: &str, word: &str) -> Vec<String> {
    let wake_txt = voiceprints_dir().join(format!("{}.wake.txt", name));
    let mut words = get_wake_words(name);
    words.retain(|w| w != word);
    if let Ok(data) = serde_json::to_string(&words) {
        let _ = std::fs::write(&wake_txt, data);
    }
    words
}

/// Verify: extract embedding from WAV, compare with all enrolled.
pub fn verify_speaker(wav_path: &str, threshold: Option<f32>) -> Result<VerifyResult, String> {
    let thr = threshold.unwrap_or(0.7);

    // Extract embedding via Python STT server (bypasses macOS 12 ORT C API mismatch)
    let probe = extract_embedding_http(wav_path)?;

    // Compare with all enrolled
    let vp_dir = voiceprints_dir();
    if !vp_dir.exists() {
        return Ok(VerifyResult {
            verified: false,
            speaker: None,
            confidence: 0.0,
        });
    }

    let mut best_name = String::new();
    let mut best_score: f32 = 0.0;

    for entry in std::fs::read_dir(&vp_dir).map_err(|e| format!("readdir: {}", e))? {
        let entry = entry.map_err(|e| format!("dirent: {}", e))?;
        let path = entry.path();
        if path.extension().map_or(true, |e| e != "bin") {
            continue;
        }
        let name = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let ref_emb: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();

        let score = cosine_similarity(&probe, &ref_emb);
        if score > best_score {
            best_score = score;
            best_name = name;
        }
    }

    let verified = best_score >= thr && !best_name.is_empty();
    Ok(VerifyResult {
        verified,
        speaker: if verified { Some(best_name) } else { None },
        confidence: best_score,
    })
}

/// List enrolled speakers.
pub fn list_speakers() -> Result<Vec<SpeakerInfo>, String> {
    let vp_dir = voiceprints_dir();
    if !vp_dir.exists() {
        return Ok(vec![]);
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&vp_dir).map_err(|e| format!("readdir: {}", e))? {
        let entry = entry.map_err(|e| format!("dirent: {}", e))?;
        let path = entry.path();
        if path.extension().map_or(true, |e| e != "bin") {
            continue;
        }
        let name = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let mtime = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .map(|t| {
                let dt: chrono::DateTime<chrono::Utc> = t.into();
                dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()
            })
            .unwrap_or_default();
        out.push(SpeakerInfo {
            name,
            enrolled_at: mtime,
        });
    }
    Ok(out)
}

/// Validate a speaker name: reject path-traversal and non-identifier chars.
fn validate_speaker_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 64 {
        return Err("name must be 1-64 chars".into());
    }
    if name.contains('.') || name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err("invalid name: path separators or dots not allowed".into());
    }
    if !name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
        return Err("name must be alphanumeric, underscore, or hyphen".into());
    }
    Ok(())
}

/// Remove an enrolled speaker.
pub fn remove_speaker(name: &str) -> Result<(), String> {
    validate_speaker_name(name)?;
    let base = voiceprints_dir();
    let path = base.join(format!("{}.bin", name));
    // Canonicalize both sides to ensure we stay inside voiceprints/
    let canon = std::fs::canonicalize(&path).map_err(|e| format!("resolve: {}", e))?;
    let base_canon = std::fs::canonicalize(&base).unwrap_or_else(|_| base.clone());
    if !canon.starts_with(&base_canon) {
        return Err("path escapes voiceprints directory".into());
    }
    std::fs::remove_file(&canon).map_err(|e| format!("remove: {}", e))
}
