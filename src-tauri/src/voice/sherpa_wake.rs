// sherpa_wake.rs — Native wake-word detection via VAD + speaker embedding
//
// Architecture:
//   mic capture → buffer 1.5s → HTTP to Python STT server for VAD + speaker embedding
//   → match → emit "wake-word-detected" with pre_verified=true (≡ pressing fn key)
//
// Public API:
//   start_wake_listener(app, threshold)
//   stop_wake_listener()
//   is_wake_active()
//   pause_wake() / resume_wake()
//   enroll_speaker(name, wav_path)
//   verify_speaker(wav_path, threshold)
//   list_speakers() / remove_speaker(name)

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
const DETECTION_COOLDOWN_MS: u64 = 1500;

// ── Paths ────────────────────────────────────────────────────────────────────


fn voiceprints_dir() -> std::path::PathBuf {
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

/// Load enrolled embedding from disk. Returns None if not found.
fn load_enrolled_embedding() -> Option<Vec<f32>> {
    let path = voiceprints_dir().join("Me.bin");
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() % 4 != 0 {
        return None;
    }
    Some(
        bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
    )
}

// ── Start / Stop ─────────────────────────────────────────────────────────────

pub fn start_wake_listener(app: AppHandle, threshold: f32) -> Result<(), String> {
    if WAKE_ACTIVE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("wake listener already active".into());
    }

    // Check that an enrolled voiceprint exists
    if load_enrolled_embedding().is_none() {
        WAKE_ACTIVE.store(false, Ordering::Release);
        return Err("No enrolled voiceprint. Record a wake word first.".into());
    }

    // Start mic capture (no native sherpa-onnx needed — VAD + embedding via HTTP)
    let (tx, rx) = mpsc::channel::<Vec<i16>>();
    let stream_handle = match start_streaming_capture(OWNER_WAKE, move |samples, _sr| {
        let _ = tx.send(samples.to_vec());
    }) {
        Ok(h) => h,
        Err(e) => {
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
                threshold,
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
    eprintln!("[wake] VAD+embedding listener started");
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

// ── Worker loop ──────────────────────────────────────────────────────────────
//
// Flow:
//   mic → resample 16kHz f32 → feed VAD chunk-by-chunk
//   VAD detects speech segment → buffer speech samples
//   On speech-end (VAD goes silent) → extract embedding → cosine sim → trigger?
//
/// HTTP-based wake worker: buffers ~1.5s of mic audio, sends to Python
/// server for VAD + speaker matching. Avoids native sherpa-onnx C API entirely.
fn wake_http_worker_loop(
    app: &AppHandle,
    rx: &mpsc::Receiver<Vec<i16>>,
    device_sr: u32,
    device_ch: u16,
    stop_flag: &AtomicBool,
    threshold: f32,
) -> Result<(), String> {
    // Wait for STT server to be healthy before processing audio
    {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .map_err(|e| format!("health client: {}", e))?;
        for i in 0..60 {
            if stop_flag.load(Ordering::Acquire) { return Ok(()); }
            if client.get(format!("{}/health", STT_SERVER_URL)).send().is_ok() {
                eprintln!("[wake] STT server ready after {}s", i);
                break;
            }
            if i == 59 {
                return Err("STT server not ready after 60s".into());
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }

    // Buffer ~3s of 16kHz mono audio for each HTTP check (must match enrollment duration)
    let buf_target_samples: usize = (TARGET_SR as usize) * 3; // 48000 samples = 3s
    let mut audio_buf: Vec<i16> = Vec::with_capacity(buf_target_samples * 2);
    let mut last_detection_time: std::time::Instant = std::time::Instant::now()
        .checked_sub(Duration::from_millis(DETECTION_COOLDOWN_MS))
        .unwrap_or(std::time::Instant::now());

    while !stop_flag.load(Ordering::Acquire) {
        if WAKE_PAUSED.load(Ordering::Acquire) {
            let _ = rx.recv_timeout(Duration::from_millis(READ_TIMEOUT_MS));
            audio_buf.clear();
            continue;
        }

        match rx.recv_timeout(Duration::from_millis(READ_TIMEOUT_MS)) {
            Ok(raw_samples) => {
                audio_buf.extend_from_slice(&raw_samples);

                if audio_buf.len() >= buf_target_samples * (device_ch.max(1) as usize) {
                    // Sliding window: send last 3s, keep 1s overlap for next check
                    let keep_samples = (TARGET_SR as usize) * 1 * (device_ch.max(1) as usize);
                    let drain_end = audio_buf.len().saturating_sub(keep_samples);
                    let samples_to_send: Vec<i16> = audio_buf.drain(..drain_end).collect();
                    match wake_http_check(&samples_to_send, device_sr, device_ch, threshold) {
                        Ok(result) => {
                            if result.score > 0.0 { eprintln!("[wake] check: match={} score={:.3}", result.speaker_match, result.score); }
                            if result.speaker_match && result.keyword_match {
                                let now = std::time::Instant::now();
                                if now.duration_since(last_detection_time)
                                    >= Duration::from_millis(DETECTION_COOLDOWN_MS)
                                {
                                    last_detection_time = now;
                                    eprintln!("[wake] HTTP MATCH! score={:.3} keyword={}", result.score, result.keyword_match);
                                    // Wake = fn-key-down: reuse all hotkey logic
                                    let _ = app.emit("fn-key-down", ());
                                    eprintln!("[wake] emitted fn-key-down, worker will continue until stop_wake_listener is called");
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("[wake] HTTP check failed: {}", e);
                        }
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                // On timeout, if we have significant buffered audio, flush it
                if !audio_buf.is_empty() {
                    let samples_to_send: Vec<i16> = audio_buf.drain(..).collect();
                    match wake_http_check(&samples_to_send, device_sr, device_ch, threshold) {
                        Ok(result) => {
                            if result.score > 0.0 { eprintln!("[wake] flush check: speaker={} keyword={} score={:.3}", result.speaker_match, result.keyword_match, result.score); }
                            if result.speaker_match && result.keyword_match {
                                let now = std::time::Instant::now();
                                if now.duration_since(last_detection_time)
                                    >= Duration::from_millis(DETECTION_COOLDOWN_MS)
                                {
                                    last_detection_time = now;
                                    eprintln!("[wake] HTTP MATCH (flush)! score={:.3}", result.score);
                                    // Wake = fn-key-down: reuse all hotkey logic
                                    let _ = app.emit("fn-key-down", ());
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("[wake] flush HTTP check failed: {}", e);
                        }
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err("mic channel disconnected".into());
            }
        }
    }
    Ok(())
}

#[derive(Debug)]
struct WakeCheckResult {
    speaker_match: bool,
    keyword_match: bool,
    score: f32,
}

/// Send audio buffer to Python /wake/check endpoint.
fn wake_http_check(
    raw_samples: &[i16],
    device_sr: u32,
    device_ch: u16,
    threshold: f32,
) -> Result<WakeCheckResult, String> {
    // Convert to mono f32
    let mono_f32: Vec<f32> = if device_ch > 1 {
        let ch = device_ch as usize;
        raw_samples
            .chunks_exact(ch)
            .map(|c| {
                let sum: f32 = c.iter().map(|&s| s as f32).sum();
                sum / (ch as f32) / 32768.0
            })
            .collect()
    } else {
        raw_samples.iter().map(|&s| s as f32 / 32768.0).collect()
    };

    // Resample to 16kHz (linear interpolation)
    let resampled = if device_sr != TARGET_SR && !mono_f32.is_empty() {
        let ratio = device_sr as f64 / TARGET_SR as f64;
        let out_len = ((mono_f32.len() as f64) / ratio).floor() as usize;
        let mut out = Vec::with_capacity(out_len);
        for i in 0..out_len {
            let pos = (i as f64) * ratio;
            let idx = pos.floor() as usize;
            let frac = (pos - pos.floor()) as f32;
            let v = if idx + 1 >= mono_f32.len() {
                mono_f32[mono_f32.len() - 1]
            } else {
                mono_f32[idx] * (1.0 - frac) + mono_f32[idx + 1] * frac
            };
            out.push((v * 32767.0).round().clamp(-32768.0, 32767.0) as i16);
        }
        out
    } else {
        mono_f32.iter().map(|&s| (s * 32767.0).round().clamp(-32768.0, 32767.0) as i16).collect()
    };

    // Build WAV bytes (mono, 16-bit, 16kHz)
    let wav = i16_to_wav(&resampled, TARGET_SR);

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("wake HTTP client: {}", e))?;

    let part = reqwest::blocking::multipart::Part::bytes(wav)
        .file_name("wake.wav")
        .mime_str("audio/wav")
        .map_err(|e| format!("mime: {}", e))?;

    let form = reqwest::blocking::multipart::Form::new().part("file", part);

    let resp = client
        .post(format!("{}/wake/check?threshold={:.2}", STT_SERVER_URL, threshold))
        .multipart(form)
        .send()
        .map_err(|e| format!("wake HTTP: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("wake/check HTTP {}", resp.status()));
    }

    let v: serde_json::Value = resp.json().map_err(|e| format!("wake JSON: {}", e))?;

    Ok(WakeCheckResult {
        speaker_match: v["speaker_match"].as_bool().unwrap_or(false),
        keyword_match: v["keyword_match"].as_bool().unwrap_or(false),
        score: v["score"].as_f64().unwrap_or(0.0) as f32,
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

/// Enroll: extract embedding + wake fingerprint via Python STT server.
/// Saves both voiceprints/{name}.bin (speaker embedding) and
/// voiceprints/{name}.wake.npy (Mel-spectrogram fingerprint for wake phrase).
pub fn enroll_speaker(name: &str, wav_path: &str) -> Result<EnrollResult, String> {
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
        "[sherpa] enrolled '{}' (dur={:.1}s) — embedding + wake fingerprint saved",
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

    let _v: serde_json::Value = resp.json()
        .map_err(|e| format!("train JSON: {}", e))?;

    eprintln!("[sherpa] train appended variant for '{}'", name);

    Ok(EnrollResult {
        ok: true,
        speaker_id: name.to_string(),
        duration_s: 0.0,
        wake_text: String::new(),
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

/// Remove an enrolled speaker.
pub fn remove_speaker(name: &str) -> Result<(), String> {
    let path = voiceprints_dir().join(format!("{}.bin", name));
    if !path.exists() {
        return Err("speaker not found".into());
    }
    std::fs::remove_file(&path).map_err(|e| format!("remove: {}", e))
}
