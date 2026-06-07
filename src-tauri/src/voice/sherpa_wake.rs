// sherpa_wake.rs — Native wake-word detection via energy-VAD + speaker embedding
//
// Architecture:
//   mic capture → energy-threshold VAD (Rust, no model needed)
//   → speech detected → buffer up to 3s of audio
//   → HTTP to Python STT server for speaker embedding + keyword matching
//   → match → emit "fn-key-down" (≡ pressing fn key)
//
// The energy-VAD gate means silent periods produce zero HTTP requests,
// unlike the old blind-buffer approach that sent every ~2s regardless.
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

// ── Energy-VAD constants (mirrors conversation.rs) ─────────────────────────────────────

/// RMS threshold for speech detection. ~ -36 dBFS, tuned for laptop built-in mic.
const SPEECH_RMS_THRESHOLD: f32 = 0.015;
/// Noise-floor multiplier for adaptive threshold.
const NOISE_FLOOR_MARGIN: f32 = 1.6;
/// Hard cap on effective threshold so noisy rooms don't deafen the mic.
const EFFECTIVE_THRESHOLD_CAP: f32 = 0.03;
/// Hysteresis release: once in a speech burst, RMS must drop below this to exit.
const SPEECH_RELEASE_RMS_THRESHOLD: f32 = 0.003;
/// Release-threshold noise-floor companion.
const RELEASE_NOISE_FLOOR_MARGIN: f32 = 1.2;
/// Minimum continuous speech before we start buffering (filters clicks/coughs).
const MIN_SPEECH_BEFORE_BUFFER_MS: u64 = 200;
/// Pre-buffer lookback: keeps recent audio so the start of speech is not lost.
const LOOKBACK_MS: u64 = 500;

/// How long to buffer after speech starts (max capture window).
const MAX_BUFFER_S: f32 = 3.0;

/// Minimum audio duration to send for wake check (too short = Whisper returns garbage).
const MIN_SEND_S: f32 = 0.8;

/// If speech stops, wait this long before sending what we have.
const SPEECH_END_SILENCE_MS: u64 = 1500;

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

// ── RMS helper (mirrors conversation.rs) ──────────────────────────────────────────────

fn rms_i16(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples
        .iter()
        .map(|&s| {
            let f = s as f64 / i16::MAX as f64;
            f * f
        })
        .sum();
    (sum_sq / samples.len() as f64).sqrt() as f32
}

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
    eprintln!("[wake] energy-VAD + embedding listener started");
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
// Flow (event-driven, mirrors conversation.rs):
//   mic → downmix mono → compute RMS
//   → speech detected? start buffering
//   → buffer reaches 3s OR speech ends (silence after speech) → send HTTP
//   → Python does speaker embedding + keyword matching → trigger?
//
/// HTTP-based wake worker: energy-VAD gate, only sends HTTP when speech is present.
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

    // Buffer state
    let max_buffer_samples: usize = (TARGET_SR as usize) * (MAX_BUFFER_S as usize);
    let mut audio_buf: Vec<i16> = Vec::with_capacity(max_buffer_samples * 2);
    let mut last_detection_time: std::time::Instant = std::time::Instant::now()
        .checked_sub(Duration::from_millis(DETECTION_COOLDOWN_MS))
        .unwrap_or(std::time::Instant::now());

    // Energy-VAD state (mirrors conversation.rs)
    let mut noise_floor: f32 = 0.0;
    let mut in_speech_burst: bool = false;
    let mut is_buffering: bool = false;
    let mut speech_run_ms: u64 = 0;
    let mut silence_run_ms: u64 = 0;

    // Pre-buffer (ring): always stores the last ~500ms of resampled 16kHz audio.
    // When speech is confirmed, its contents are prepended to audio_buf
    // so the start of the utterance ("一" in "一二三四") is not lost.
    let lookback_samples = (TARGET_SR as usize * LOOKBACK_MS as usize) / 1000;
    let mut pre_buf: std::collections::VecDeque<i16> = std::collections::VecDeque::with_capacity(lookback_samples);

    while !stop_flag.load(Ordering::Acquire) {
        if WAKE_PAUSED.load(Ordering::Acquire) {
            let _ = rx.recv_timeout(Duration::from_millis(READ_TIMEOUT_MS));
            audio_buf.clear();
                    pre_buf.clear();
            is_buffering = false;
            in_speech_burst = false;
            speech_run_ms = 0;
            silence_run_ms = 0;
            continue;
        }

        match rx.recv_timeout(Duration::from_millis(READ_TIMEOUT_MS)) {
            Ok(raw_samples) => {
                let mono = downmix_to_mono(&raw_samples, device_ch);
                let chunk_ms = (mono.len() as u64 * 1000) / device_sr.max(1) as u64;
                let rms = rms_i16(&mono);

                // Adaptive threshold (mirrors conversation.rs)
                let effective_threshold = SPEECH_RMS_THRESHOLD
                    .max(noise_floor * NOISE_FLOOR_MARGIN)
                    .min(EFFECTIVE_THRESHOLD_CAP);
                let effective_release = SPEECH_RELEASE_RMS_THRESHOLD
                    .max(noise_floor * RELEASE_NOISE_FLOOR_MARGIN);

                // Hysteresis
                if rms > effective_threshold {
                    in_speech_burst = true;
                } else if rms < effective_release {
                    in_speech_burst = false;
                }

                if in_speech_burst {
                    speech_run_ms = speech_run_ms.saturating_add(chunk_ms);
                    silence_run_ms = 0;

                    // Only start buffering after MIN_SPEECH_BEFORE_BUFFER_MS
                    if speech_run_ms >= MIN_SPEECH_BEFORE_BUFFER_MS && !is_buffering {
                        is_buffering = true;
                        // Flush lookback buffer: prepend recent audio so speech onset is captured.
                        if !pre_buf.is_empty() {
                            audio_buf.extend(pre_buf.iter().copied());
                        }
                        eprintln!(
                            "[wake] speech detected: rms={:.4} thresh={:.4} floor={:.4}",
                            rms, effective_threshold, noise_floor
                        );
                    }

                } else {
                    if is_buffering {
                        silence_run_ms = silence_run_ms.saturating_add(chunk_ms);
                    } else {
                        // Update noise floor from quiet chunks
                        if rms < SPEECH_RMS_THRESHOLD * 0.4 {
                            noise_floor = 0.95 * noise_floor + 0.05 * rms;
                        }
                    }
                    speech_run_ms = 0;
                }

                // Always feed pre_buf (ring buffer) so we have lookback audio.
                {
                    let resampled = resample_to_16k(&mono, device_sr);
                    for &s in &resampled {
                        if pre_buf.len() >= lookback_samples {
                            pre_buf.pop_front();
                        }
                        pre_buf.push_back(s);
                    }
                    // Once buffering is active, also append to audio_buf.
                    if is_buffering {
                        audio_buf.extend_from_slice(&resampled);
                    }
                }
                // Send if buffer full (3s) OR speech ended (silence after buffering)
                let buffer_full = audio_buf.len() >= max_buffer_samples;
                let speech_ended = is_buffering
                    && !in_speech_burst
                    && silence_run_ms >= SPEECH_END_SILENCE_MS;

                let min_send_samples = (TARGET_SR as f32 * MIN_SEND_S) as usize;
                let enough_audio = audio_buf.len() >= min_send_samples;
                if (buffer_full || speech_ended) && enough_audio {
                    if !audio_buf.is_empty() {
                        let samples_to_send: Vec<i16> = audio_buf.drain(..).collect();
                        eprintln!(
                            "[wake] sending {} samples ({:.1}s) buffer_full={} speech_ended={}",
                            samples_to_send.len(),
                            samples_to_send.len() as f32 / TARGET_SR as f32,
                            buffer_full,
                            speech_ended,
                        );
                        match wake_http_check(&samples_to_send, TARGET_SR, 1, threshold) {
                            Ok(result) => {
                                if true {
                                    eprintln!("[wake] check: speaker={} keyword={} score={:.3} text=\"{}\"", result.speaker_match, result.keyword_match, result.score, result.keyword_text);
                                }
                                if result.speaker_match && result.keyword_match {
                                    let now = std::time::Instant::now();
                                    if now.duration_since(last_detection_time)
                                        >= Duration::from_millis(DETECTION_COOLDOWN_MS)
                                    {
                                        last_detection_time = now;
                                        eprintln!("[wake] MATCH! score={:.3} keyword={}", result.score, result.keyword_match);
                                        let _ = app.emit("fn-key-down", ());
                                        eprintln!("[wake] emitted fn-key-down");
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("[wake] HTTP check failed: {}", e);
                            }
                        }
                    }
                    is_buffering = false;
                    silence_run_ms = 0;
                    audio_buf.clear();
                    pre_buf.clear();
                } else if speech_ended {
                    eprintln!("[wake] discarding short buffer: {} samples ({:.1}s) < {:.1}s min",
                        audio_buf.len(), audio_buf.len() as f32 / TARGET_SR as f32, MIN_SEND_S);
                    is_buffering = false;
                    silence_run_ms = 0;
                    audio_buf.clear();
                    pre_buf.clear();
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                // If buffering and mic goes silent, flush what we have
                if is_buffering && !audio_buf.is_empty() {
                    let samples_to_send: Vec<i16> = audio_buf.drain(..).collect();
                    eprintln!(
                        "[wake] timeout flush: {} samples ({:.1}s)",
                        samples_to_send.len(),
                        samples_to_send.len() as f32 / TARGET_SR as f32,
                    );
                    match wake_http_check(&samples_to_send, TARGET_SR, 1, threshold) {
                        Ok(result) => {
                            if true {
                                eprintln!("[wake] flush: speaker={} keyword={} score={:.3} text=\"{}\"", result.speaker_match, result.keyword_match, result.score, result.keyword_text);
                            }
                            if result.speaker_match && result.keyword_match {
                                let now = std::time::Instant::now();
                                if now.duration_since(last_detection_time)
                                    >= Duration::from_millis(DETECTION_COOLDOWN_MS)
                                {
                                    last_detection_time = now;
                                    eprintln!("[wake] MATCH (flush)! score={:.3}", result.score);
                                    let _ = app.emit("fn-key-down", ());
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("[wake] flush HTTP check failed: {}", e);
                        }
                    }
                }
                is_buffering = false;
                silence_run_ms = 0;
                speech_run_ms = 0;
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
    keyword_text: String,
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
