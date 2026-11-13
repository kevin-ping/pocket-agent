use chrono;
use crate::voice::record::{
    current_owner, lock_capture_gate, release_capture, start_recording, stop_recording_into, stop_recording_no_handle,
    take_pre_started, try_acquire_capture, RecordingHandle,
    OWNER_CONVERSATION, OWNER_NONE, OWNER_SINGLE_SHOT, OWNER_VOICE_CLIP, OWNER_WAKE,
};
use crate::voice::sherpa_wake::VoiceSource;
use crate::voice::stt::transcribe;
use std::sync::Mutex;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, State};

const MAX_RECORDING_SECS: u64 = 30;
const MIN_RECORDING_SECS: f32 = 1.5;

/// Compute RMS dBFS of a 16-bit PCM mono/stereo WAV — used purely for logging
/// so we can diagnose the server's `too_quiet` (-30 dBFS) rejections.
fn wav_rms_dbfs(path: &str) -> f32 {
    let Ok(bytes) = std::fs::read(path) else { return -120.0 };
    if bytes.len() < 44 { return -120.0 }
    let pcm = &bytes[44..];
    if pcm.len() < 2 { return -120.0 }
    let mut sum_sq: f64 = 0.0;
    let mut n: u64 = 0;
    for pair in pcm.chunks_exact(2) {
        let s = i16::from_le_bytes([pair[0], pair[1]]) as f64 / 32768.0;
        sum_sq += s * s;
        n += 1;
    }
    if n == 0 { return -120.0 }
    let rms = (sum_sq / n as f64).sqrt();
    if rms <= 0.0 { -120.0 } else { (20.0 * rms.log10()) as f32 }
}

/// Read WAV duration from file header (no decoding needed)
fn wav_duration_secs(path: &str) -> Result<f32, String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| format!("打开WAV失败: {}", e))?;
    let mut header = [0u8; 44];
    f.read_exact(&mut header).map_err(|e| format!("读取WAV头失败: {}", e))?;
    // WAV header layout: NumChannels @ 22 (u16), SampleRate @ 24 (u32),
    // BitsPerSample @ 34 (u16), data_chunk_size @ 40 (u32) — all LE.
    let channels = u16::from_le_bytes([header[22], header[23]]) as u32;
    let sample_rate = u32::from_le_bytes([header[24], header[25], header[26], header[27]]);
    let bits_per_sample = u16::from_le_bytes([header[34], header[35]]) as u32;
    let data_size = u32::from_le_bytes([header[40], header[41], header[42], header[43]]);
    let bytes_per_frame = channels * (bits_per_sample / 8);
    if sample_rate == 0 || bytes_per_frame == 0 {
        return Ok(0.0);
    }
    Ok(data_size as f32 / (sample_rate as f32 * bytes_per_frame as f32))
}

pub struct RecordingState {
    handle: Mutex<Option<RecordingHandle>>,
    timeout_active: Arc<AtomicBool>,
}

impl Default for RecordingState {
    fn default() -> Self {
        Self {
            handle: Mutex::new(None),
            timeout_active: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// RAII guard that releases the SINGLE_SHOT capture owner on drop unless
/// explicitly committed. Used so any early-return from start_voice_recording
/// (mutex error, daemon start failure, thread spawn failure) reliably releases
/// the slot.
struct ReservationGuard {
    committed: bool,
}
impl ReservationGuard {
    fn commit(mut self) {
        self.committed = true;
    }
}
impl Drop for ReservationGuard {
    fn drop(&mut self) {
        if !self.committed {
            release_capture(OWNER_SINGLE_SHOT);
        }
    }
}

#[tauri::command]
pub fn start_voice_recording(
    app: AppHandle,
    state: State<'_, RecordingState>,
) -> Result<(), String> {
    // Hold the capture gate across the *entire* setup: peer-flag check, slot reservation,
    // daemon warmup, and final handle commit. Concurrent callers serialize behind it and
    // observe state.handle == Some on their first check, avoiding the daemon-warmup
    // window where neither SINGLE_SHOT_RESERVED nor start_ack reliably reflects the truth.
    // Brief blocking (~10-200ms) on the rare second hotkey press is acceptable.
    let _gate = lock_capture_gate();

    // Owner transition matrix (record.rs file-header doc):
    //   CONVERSATION → SINGLE_SHOT  ✗ refused (different active flow)
    //   WAKE         → SINGLE_SHOT  ✓ pre-empt: stop wake, frontend re-arms post-stop
    //   NONE         → SINGLE_SHOT  ✓
    //   SINGLE_SHOT  → SINGLE_SHOT  ✓ no-op
    match current_owner() {
        OWNER_CONVERSATION => {
            return Err("连续对话进行中，无法启动单次录音".into());
        }
        OWNER_WAKE => {
            crate::voice::sherpa_wake::stop_wake_listener();
            // Spin until the audio-streaming thread releases the capture device.
            // stop_wake_listener joins the worker, but the streaming thread
            // releases asynchronously via Drop — same race as conversation.rs.
            for _ in 0..50 {
                if current_owner() == OWNER_NONE {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        _ => {}
    }

    {
        let guard = state
            .handle
            .lock()
            .map_err(|_| "录音状态锁定失败".to_string())?;
        if guard.is_some() {
            return Ok(());
        }
    }

    // Acquire the single-shot slot synchronously so a peer streaming entry can
    // observe ownership immediately (before daemon start_ack).
    try_acquire_capture(OWNER_SINGLE_SHOT)?;
    let reservation = ReservationGuard { committed: false };

    // Destructive actions deferred from the hotkey thread so the break-confirmation
    // popup can intercept them: stop any in-progress TTS, then warm up the mic.
    crate::commands::chat::stop_audio_queue("single-shot start");
    crate::voice::record::pre_start();

    let handle = match take_pre_started() {
        Some(h) => h,
        None => start_recording()?, // reservation released by ReservationGuard::drop
    };

    {
        let mut guard = state
            .handle
            .lock()
            .map_err(|_| "录音状态锁定失败".to_string())?;
        // Under the held gate, this should never see Some — but keep the check as a defensive belt.
        if guard.is_some() {
            return Ok(());
        }
        *guard = Some(handle);
    }

    // Mark timeout as active
    state.timeout_active.store(true, Ordering::SeqCst);
    let timeout_flag = state.timeout_active.clone();
    let app_clone = app.clone();

    // Spawn VAD + auto-timeout thread.
    // Uses two-stage detection:
    //   1. Quick RMS energy check (every 100ms) to gate expensive HTTP calls
    //   2. Silero VAD via Python STT server (every 500ms when energy detected)
    //      to distinguish human speech from noise (birds, music, etc.)
    // Falls back to MAX_RECORDING_SECS hard timeout.
    std::thread::Builder::new()
        .name("recording-vad".to_string())
        .spawn(move || {
            let tick_ms: u64 = 100;
            let vad_check_ms: u64 = 500;       // Silero VAD check interval
            let rms_gate: f32 = 0.01;           // RMS gate: skip VAD HTTP if below
            let min_speech_ms: u64 = 500;       // minimum speech before silence triggers stop
            let silence_stop_ms: u64 = 1500;    // 1.5s confirmed no-speech → stop
            let mut heard_speech_ms: u64 = 0;
            let mut silence_after_speech_ms: u64 = 0;
            let mut last_vad_check_ms: u64 = 0;
                        let max_ticks = MAX_RECORDING_SECS * 1000 / tick_ms;

            for _ in 0..max_ticks {
                if timeout_flag.load(Ordering::SeqCst) == false { return; }
                std::thread::sleep(std::time::Duration::from_millis(tick_ms));
                                last_vad_check_ms += tick_ms;

                let level = crate::voice::record::AUDIO_LEVEL.load(Ordering::Relaxed) as f32 / 1000.0;

                // Quick gate: if RMS is very low, definitely no speech
                if level < rms_gate {
                    if heard_speech_ms >= min_speech_ms {
                        silence_after_speech_ms += tick_ms;
                    }
                    if heard_speech_ms >= min_speech_ms && silence_after_speech_ms >= silence_stop_ms {
                        if timeout_flag.compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                            eprintln!("[voice] silence after {}ms speech, stopping", heard_speech_ms);
                            let _ = app_clone.emit("fn-key-up", ());
                            return;
                        }
                    }
                    continue;
                }

                // RMS above gate — could be speech or noise.
                // Use Silero VAD to distinguish, but not too frequently.
                if last_vad_check_ms >= vad_check_ms {
                    last_vad_check_ms = 0;
                    let has_speech = check_silero_vad();
                    if has_speech {
                        heard_speech_ms += vad_check_ms;
                        silence_after_speech_ms = 0;
                    } else if heard_speech_ms >= min_speech_ms {
                        // Silero says no human speech (birds/noise only)
                        silence_after_speech_ms += vad_check_ms;
                    }

                    if heard_speech_ms >= min_speech_ms && silence_after_speech_ms >= silence_stop_ms {
                        if timeout_flag.compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                            eprintln!("[voice] Silero VAD: no speech for {}ms after {}ms speech, stopping",
                                silence_after_speech_ms, heard_speech_ms);
                            let _ = app_clone.emit("fn-key-up", ());
                            return;
                        }
                    }
                }
            }
            // Hard timeout
            if timeout_flag.compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                eprintln!("[voice] auto-timeout after {}s", MAX_RECORDING_SECS);
                let _ = app_clone.emit("fn-key-up", ());
            }
        })
        .map_err(|e| format!("启动VAD线程失败: {}", e))?;

    // Recording is live; ownership of the reservation has transferred to stop_recording_internal.
    reservation.commit();
    eprintln!("[voice] recording started (max {}s, VAD auto-stop)", MAX_RECORDING_SECS);
    Ok(())
}

#[tauri::command]
pub async fn stop_voice_recording(
    app: AppHandle,
    state: State<'_, RecordingState>,
) -> Result<(), String> {
    // Cancel the auto-timeout
    state.timeout_active.store(false, Ordering::SeqCst);

    // Clear handle (for housekeeping)
    {
        let mut guard = state
            .handle
            .lock()
            .map_err(|_| "录音状态锁定失败".to_string())?;
        guard.take();
    }

    eprintln!("[voice] recording stopped, starting STT... [{}]", chrono::Local::now().format("%H:%M:%S%.3f"));
    let wav_path = tokio::task::spawn_blocking(move || stop_recording_no_handle())
        .await
        .map_err(|e| format!("停止录音失败: {}", e))??;

    // Check WAV duration — reject recordings shorter than 1.5s to prevent Whisper hallucination
    let path_clone = wav_path.clone();
    let duration_secs = tokio::task::spawn_blocking(move || {
        wav_duration_secs(&path_clone)
    }).await
        .map_err(|e| format!("时长检查失败: {}", e))??;

    if duration_secs < MIN_RECORDING_SECS {
        eprintln!("[voice] recording too short ({:.1}s), skipping STT", duration_secs);
        app.emit("stt-error", serde_json::json!({ "error": "录音时间太短，请长按说话" }))
            .map_err(|e| e.to_string())?;
        return Ok(());
    }

    let result = tokio::task::spawn_blocking(move || transcribe(&wav_path))
        .await
        .map_err(|e| format!("STT 任务失败: {}", e))?;

    match result {
        Ok(result) => {
            eprintln!("[voice] stt: {:?} (lang: {}) [{}]", result.text, result.language, chrono::Local::now().format("%H:%M:%S%.3f"));
            eprintln!("[voice] >>> sending to LLM...");
            app.emit("stt-result", serde_json::json!({ "text": result.text, "language": result.language }))
                .map_err(|e| e.to_string())?;
        }
        Err(e) => {
            eprintln!("[voice] stt error: {}", e);
            app.emit("stt-error", serde_json::json!({ "error": e }))
                .map_err(|e| e.to_string())?;
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn cancel_voice_recording(
    state: State<'_, RecordingState>,
) -> Result<(), String> {
    // Cancel the auto-timeout
    state.timeout_active.store(false, Ordering::SeqCst);

    // Clear handle (housekeeping)
    {
        let mut guard = state
            .handle
            .lock()
            .map_err(|_| "录音状态锁定失败".to_string())?;
        guard.take();
    }

    // Stop recording, discard the WAV — ignore errors
    let _ = tokio::task::spawn_blocking(move || stop_recording_no_handle()).await;
    eprintln!("[voice] recording cancelled, discarding audio");

    Ok(())
}

/// Read current audio level normalized to 0.0-1.0 from the recording callback.
/// Frontend polls this every ~200ms during recording to show a level meter.
#[tauri::command]
pub fn get_audio_level() -> f32 {
    crate::voice::record::AUDIO_LEVEL.load(std::sync::atomic::Ordering::Relaxed) as f32 / 1000.0
}

/// Check recent audio with Silero VAD via Python STT server.
/// Returns true if human speech is detected in the last ~1s of audio.
/// Infrastructure failures are treated as "no speech" here (fail-closed): the
/// wake path must never start a session on an unverified signal.
pub(crate) fn check_silero_vad() -> bool {
    // Read ~1s of recent mono samples (16kHz = 16000 samples)
    let samples = crate::voice::record::read_vad_samples(16000);
    if samples.len() < 3200 {
        // Less than 200ms of audio, not enough for VAD
        return false;
    }
    check_silero_vad_samples(&samples, 16000).unwrap_or(false)
}

/// Check raw audio samples with Silero VAD. Works with any sample rate — the
/// Python server handles the actual analysis; we just build a valid WAV.
///
/// Returns `None` when the check could not be performed at all (stt-server
/// down / busy / timed out / bad response), so callers can decide their own
/// failure policy instead of being forced into a silent `false`. Callers that
/// already hold independent evidence of speech (e.g. sustained RMS) should
/// fail-open on `None`; callers with no other evidence should fail-closed.
pub(crate) fn check_silero_vad_samples(samples: &[i16], sample_rate: u32) -> Option<bool> {
    if samples.len() < 3200 {
        return Some(false);
    }

    // Build WAV bytes
    let sr: u32 = sample_rate;
    let data_len = samples.len() * 2;
    let mut wav = Vec::with_capacity(44 + data_len);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());   // PCM
    wav.extend_from_slice(&1u16.to_le_bytes());   // mono
    wav.extend_from_slice(&sr.to_le_bytes());
    wav.extend_from_slice(&(sr * 2).to_le_bytes()); // byte rate
    wav.extend_from_slice(&2u16.to_le_bytes());   // block align
    wav.extend_from_slice(&16u16.to_le_bytes());  // bits per sample
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data_len as u32).to_le_bytes());
    for &s in samples {
        wav.extend_from_slice(&s.to_le_bytes());
    }

    // 800ms: this call blocks the conversation worker thread during the
    // barge-in confirmation gate, so a slow stt-server must not stall the
    // interrupt. Past the deadline the caller applies its own failure policy.
    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_millis(800))
        .build()
    {
        Ok(c) => c,
        Err(_) => return None,
    };

    let part = match reqwest::blocking::multipart::Part::bytes(wav)
        .file_name("vad.wav")
        .mime_str("audio/wav")
    {
        Ok(p) => p,
        Err(_) => return None,
    };

    let form = reqwest::blocking::multipart::Form::new().part("file", part);

    match client
        .post("http://127.0.0.1:8651/vad/check")
        .multipart(form)
        .send()
    {
        // A response without a usable has_speech field is an infrastructure
        // failure too, not a "no speech" verdict.
        Ok(resp) => match resp.json::<serde_json::Value>() {
            Ok(v) => v["has_speech"].as_bool(),
            Err(_) => None,
        },
        Err(_) => None,
    }
}

#[tauri::command]
pub fn start_continuous_conversation(
    app: AppHandle,
    silence_timeout_secs: Option<u64>,
    pause_tolerance_ms: Option<u64>,
    speech_rms_threshold: Option<f32>,
    single_shot: Option<bool>,
    barge_in_rms_threshold: Option<f32>,
    wake_word_threshold: Option<f32>,
    barge_in_enabled: Option<bool>,
) -> Result<(), String> {
    crate::voice::conversation::start_conversation(
        app,
        silence_timeout_secs,
        pause_tolerance_ms,
        speech_rms_threshold,
        single_shot,
        barge_in_rms_threshold,
        wake_word_threshold,
        barge_in_enabled,
    )
}

#[tauri::command]
pub fn stop_continuous_conversation() -> Result<(), String> {
    // Stopping the conversation must also silence whatever is already queued —
    // otherwise the assistant keeps reading a long reply after the user has
    // explicitly ended the session. Safe for single-shot too: that path stops
    // before send_message allocates its turn, so the generation bump is a no-op.
    crate::commands::chat::stop_audio_queue("conversation stopped");
    crate::voice::conversation::stop_conversation();
    Ok(())
}

#[tauri::command]
pub fn notify_conversation_tts_started() -> Result<(), String> {
    crate::voice::conversation::on_tts_started();
    Ok(())
}

#[tauri::command]
pub fn notify_conversation_tts_done(generation: u64) -> Result<(), String> {
    crate::voice::conversation::on_tts_done(generation);
    Ok(())
}

#[tauri::command]
pub fn is_continuous_conversation_active() -> bool {
    crate::voice::conversation::is_active()
}

#[tauri::command]
pub async fn start_wake_word_listening(app: AppHandle, threshold: Option<f32>, speaker_name: Option<String>) -> Result<(), String> {
    tokio::task::spawn_blocking(move || crate::voice::sherpa_wake::start_wake_listener(app, threshold.unwrap_or(0.65), speaker_name))
        .await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn stop_wake_word_listening() -> Result<(), String> {
    crate::voice::sherpa_wake::stop_wake_listener();
    Ok(())
}

#[tauri::command]
pub fn is_wake_word_active() -> bool {
    crate::voice::sherpa_wake::is_wake_active()
}

#[tauri::command]
pub fn is_app_ready() -> bool {
    crate::voice::stt::is_app_ready()
}

// Speaker-enrollment recording: same recorder as single-shot dictation, but the
// matching stop command skips Whisper STT and hands the WAV over to a file of
// its own, which the consuming command deletes when done.
#[tauri::command]
pub fn start_enroll_recording(
    state: State<'_, RecordingState>,
) -> Result<(), String> {
    let _gate = lock_capture_gate();

    match current_owner() {
        OWNER_CONVERSATION => {
            return Err("连续对话进行中，无法启动唤醒词录制".into());
        }
        OWNER_WAKE => {
            crate::voice::sherpa_wake::stop_wake_listener();
        }
        _ => {}
    }

    {
        let guard = state
            .handle
            .lock()
            .map_err(|_| "录音状态锁定失败".to_string())?;
        if guard.is_some() {
            return Ok(());
        }
    }

    try_acquire_capture(OWNER_SINGLE_SHOT)?;
    let reservation = ReservationGuard { committed: false };

    crate::commands::chat::stop_audio_queue("enroll recording start");
    crate::voice::record::pre_start();

    let handle = match take_pre_started() {
        Some(h) => h,
        None => start_recording()?,
    };

    {
        let mut guard = state
            .handle
            .lock()
            .map_err(|_| "录音状态锁定失败".to_string())?;
        if guard.is_some() {
            return Ok(());
        }
        *guard = Some(handle);
    }

    reservation.commit();
    eprintln!("[voice] enroll recording started");
    Ok(())
}

#[tauri::command]
pub async fn stop_enroll_recording(
    state: State<'_, RecordingState>,
) -> Result<String, String> {
    state.timeout_active.store(false, Ordering::SeqCst);

    {
        let mut guard = state
            .handle
            .lock()
            .map_err(|_| "录音状态锁定失败".to_string())?;
        guard.take();
    }

    // Not the shared recording file: a dictation started while the voiceprint
    // request is in flight would reuse that name and be deleted with it.
    let dest = exclusive_clip_path("enroll");
    let dest_clone = dest.clone();
    let wav_path = tokio::task::spawn_blocking(move || stop_recording_into(dest_clone))
        .await
        .map_err(|e| format!("停止录音失败: {}", e))?
        .inspect_err(|_| remove_temp_recording(&dest))
        // The wake settings page shows only coded errors; a silent mic is one
        // the user can fix, so give it a code instead of "wake service error".
        .map_err(|e| {
            if e.starts_with(crate::voice::record::SILENT_RECORDING) {
                eprintln!("[voice] enroll recording is pure silence");
                crate::voice::sherpa_wake::wake_error("wake_mic_silent", serde_json::json!({}))
            } else {
                e
            }
        })?;

    let path_clone = wav_path.clone();
    let duration_secs = tokio::task::spawn_blocking(move || wav_duration_secs(&path_clone))
        .await
        .map_err(|e| format!("时长检查失败: {}", e))?
        .inspect_err(|_| remove_temp_recording(&dest))?;

    if duration_secs < MIN_RECORDING_SECS {
        remove_temp_recording(&dest);
        return Err(format!("录音时间太短 ({:.1}s)", duration_secs));
    }

    let rms_db = wav_rms_dbfs(&wav_path);
    eprintln!(
        "[voice] enroll recording stopped, {:.1}s, rms={:.1}dBFS, wav={}",
        duration_secs, rms_db, wav_path
    );
    Ok(wav_path)
}

#[tauri::command]
pub async fn enroll_speaker(
    name: String,
    audio_path: String,
    source: Option<VoiceSource>,
) -> Result<crate::voice::sherpa_wake::EnrollResult, String> {
    let source = source.unwrap_or(VoiceSource::Desktop);
    // Off the main thread: enrollment is a blocking HTTP request.
    tokio::task::spawn_blocking(move || {
        let result = crate::voice::sherpa_wake::enroll_speaker(&name, &audio_path, source);
        remove_temp_recording(std::path::Path::new(&audio_path));
        result
    })
    .await
    .map_err(|e| format!("enroll task failed: {e}"))?
}

#[tauri::command]
pub fn voiceprint_samples(name: String) -> Result<Vec<crate::voice::sherpa_wake::SampleInfo>, String> {
    crate::voice::sherpa_wake::voiceprint_samples(&name)
}

/// Whether wake and conversation listen on the phone right now; changes are
/// announced by `audio-source-changed`.
#[tauri::command]
pub fn audio_source_is_phone() -> bool {
    crate::voice::remote::is_online()
}

/// Delete one of `name`'s samples; the other stays.
#[tauri::command]
pub fn remove_voiceprint_sample(name: String, source: VoiceSource) -> Result<(), String> {
    crate::voice::sherpa_wake::remove_voiceprint_sample(&name, source)
}

static CLIP_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A temp WAV path no other recording uses: `pocket-agent-<kind>-<pid>-<n>.wav`.
fn exclusive_clip_path(kind: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "pocket-agent-{kind}-{}-{}.wav",
        std::process::id(),
        CLIP_ID.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Record `seconds` (1–10) from the phone for a voiceprint sample or a test,
/// through the same 16 kHz path conversations use on the phone. Returns a temp
/// WAV path. Wake is paused like start_enroll_recording; the caller resumes it.
#[tauri::command]
pub async fn record_phone_clip(seconds: u32) -> Result<String, String> {
    let seconds = seconds.clamp(1, 10);
    tokio::task::spawn_blocking(move || record_phone_clip_blocking(seconds))
        .await
        .map_err(|e| format!("phone clip task failed: {e}"))?
}

fn record_phone_clip_blocking(seconds: u32) -> Result<String, String> {
    let not_source = || crate::voice::sherpa_wake::wake_error("phone_not_source", serde_json::json!({}));
    {
        let _gate = lock_capture_gate();
        match current_owner() {
            OWNER_CONVERSATION => return Err("连续对话进行中，无法录制手机样本".into()),
            OWNER_SINGLE_SHOT | OWNER_VOICE_CLIP => return Err("录音进行中，无法录制手机样本".into()),
            OWNER_WAKE => {
                crate::voice::sherpa_wake::stop_wake_listener();
                for _ in 0..50 {
                    if current_owner() == OWNER_NONE {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
            _ => {}
        }
    }
    // Epoch first: a switch after this read is caught below, one before it
    // shows here as the phone no longer being the source.
    let epoch = crate::voice::remote::source_epoch();
    if !crate::voice::remote::is_online() {
        return Err(not_source());
    }
    crate::commands::chat::stop_audio_queue("phone clip start");

    let samples = Arc::new(Mutex::new(Vec::<i16>::new()));
    let sink = samples.clone();
    // Its own owner: a hotkey dictation or another recording started meanwhile
    // is refused as busy instead of sharing (and later losing) the slot.
    let handle = crate::voice::record::start_streaming_capture(OWNER_VOICE_CLIP, move |chunk, _sr| {
        if let Ok(mut buf) = sink.lock() {
            buf.extend_from_slice(chunk);
        }
    })?;
    let sample_rate = handle.sample_rate;
    std::thread::sleep(std::time::Duration::from_secs(seconds as u64));
    let switched = handle.is_stale() || crate::voice::remote::source_epoch() != epoch;
    crate::voice::record::stop_streaming_capture(handle);
    let samples = std::mem::take(&mut *samples.lock().map_err(|_| "phone clip lock".to_string())?);
    if switched || samples.is_empty() {
        eprintln!("[voice] phone clip discarded (source switched={}, samples={})", switched, samples.len());
        return Err(not_source());
    }

    let path = exclusive_clip_path("phone-clip");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec).map_err(|e| format!("wav create: {}", e))?;
    for &s in &samples {
        writer.write_sample(s).map_err(|e| format!("wav write: {}", e))?;
    }
    writer.finalize().map_err(|e| format!("wav finalize: {}", e))?;
    eprintln!("[voice] phone clip recorded, {:.1}s", samples.len() as f32 / sample_rate as f32);
    Ok(path.to_string_lossy().into_owned())
}

#[derive(serde::Serialize)]
pub struct SampleScore {
    source: VoiceSource,
    score: f32,
}

#[derive(serde::Serialize)]
pub struct VoiceprintTest {
    scores: Vec<SampleScore>,
    /// The sample a real check on `source` would use; None = it would refuse.
    used: Option<VoiceSource>,
    threshold: f32,
    passed: bool,
}

/// Score a test clip recorded on `source` against each of `name`'s samples,
/// pass or fail it on the sample a real check would use (effective_voiceprint)
/// with the saved strictness, then delete the clip.
#[tauri::command]
pub async fn test_voiceprint(name: String, audio_path: String, source: VoiceSource) -> Result<VoiceprintTest, String> {
    tokio::task::spawn_blocking(move || {
        let scores = crate::voice::sherpa_wake::owner_scores(&audio_path, &name);
        remove_temp_recording(std::path::Path::new(&audio_path));
        let scores = scores?;
        let threshold = crate::commands::settings_repository::load()?.wake_word_threshold;
        let used = crate::voice::sherpa_wake::effective_voiceprint(
            &crate::voice::sherpa_wake::voiceprints_dir(),
            &name,
            source,
        )
        .map(|(used, _)| used);
        let score = |want| scores.iter().find(|(s, _)| Some(*s) == want).map(|&(_, score)| score);
        Ok(VoiceprintTest {
            passed: score(used).is_some_and(|score| score >= threshold),
            scores: scores.into_iter().map(|(source, score)| SampleScore { source, score }).collect(),
            used,
            threshold,
        })
    })
    .await
    .map_err(|e| format!("voiceprint test task failed: {e}"))?
}

/// Deletes `path` only when it is one of PA's own recordings: directly in the
/// temp dir (after resolving links) and named `pocket-agent-*.wav`.
fn remove_temp_recording(path: &std::path::Path) {
    if is_pa_temp_recording(path) {
        let _ = std::fs::remove_file(path);
    } else {
        eprintln!("[voice] not deleting {}: not a PA recording", path.display());
    }
}

fn is_pa_temp_recording(path: &std::path::Path) -> bool {
    let (Ok(file), Ok(temp)) = (std::fs::canonicalize(path), std::fs::canonicalize(std::env::temp_dir())) else {
        return false;
    };
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    file.parent() == Some(temp.as_path()) && name.starts_with("pocket-agent-") && name.ends_with(".wav")
}


#[tauri::command]
pub async fn train_speaker(
    name: String,
    audio_path: String,
) -> Result<crate::voice::sherpa_wake::EnrollResult, String> {
    tokio::task::spawn_blocking(move || crate::voice::sherpa_wake::train_speaker(&name, &audio_path))
        .await
        .map_err(|e| format!("train task failed: {e}"))?
}


#[tauri::command]
pub fn get_wake_variant_count(name: String) -> usize {
    crate::voice::sherpa_wake::get_wake_variant_count(&name)
}

#[tauri::command]
pub fn get_wake_words(name: String) -> Vec<String> {
    crate::voice::sherpa_wake::get_wake_words(&name)
}

#[tauri::command]
pub fn remove_wake_word(name: String, word: String) -> Vec<String> {
    crate::voice::sherpa_wake::remove_wake_word(&name, &word)
}

#[tauri::command]
pub async fn verify_speaker(
    audio_path: String,
    threshold: Option<f32>,
) -> Result<crate::voice::sherpa_wake::VerifyResult, String> {
    tokio::task::spawn_blocking(move || crate::voice::sherpa_wake::verify_speaker(&audio_path, threshold))
        .await
        .map_err(|e| format!("verify task failed: {e}"))?
}

#[tauri::command]
pub fn list_speakers() -> Result<Vec<crate::voice::sherpa_wake::SpeakerInfo>, String> {
    crate::voice::sherpa_wake::list_speakers()
}

#[tauri::command]
pub fn remove_speaker(name: String) -> Result<(), String> {
    crate::voice::sherpa_wake::remove_speaker(&name)
}

// Legacy: sherpa-onnx wake doesn't write probe files.
// Kept as a no-op for frontend compatibility during transition.
#[tauri::command]
pub fn consume_wake_probe(_path: String) -> Result<(), String> {
    Ok(())
}
#[tauri::command]
pub fn acknowledge_conversation_ready(id: u64, played: bool, ready_delay_ms: Option<u64>) {
    crate::voice::conversation::acknowledge_ready(id, played, ready_delay_ms);
}

#[tauri::command]
pub async fn wake_model_status() -> Result<serde_json::Value, String> {
    tokio::task::spawn_blocking(|| crate::voice::sherpa_wake::kws_model_action(false))
        .await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn wake_pronunciation(phrase: String) -> Result<serde_json::Value, String> {
    tokio::task::spawn_blocking(move || crate::voice::sherpa_wake::kws_pronounce(&phrase))
        .await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn install_wake_model() -> Result<serde_json::Value, String> {
    tokio::task::spawn_blocking(|| crate::voice::sherpa_wake::kws_model_action(true))
        .await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn test_wake_word(audio_path: String, config: crate::commands::config::AppConfig) -> Result<serde_json::Value, String> {
    tokio::task::spawn_blocking(move || {
        let result = crate::voice::sherpa_wake::test_wake_audio(&audio_path, &config);
        remove_temp_recording(std::path::Path::new(&audio_path));
        result
    })
    .await.map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review round 1: a voiceprint request finishing after the next dictation
    /// began must not delete that dictation's recording.
    #[test]
    fn voiceprint_cleanup_spares_a_dictation_started_meanwhile() {
        let pid = std::process::id();
        let shared = std::env::temp_dir().join(format!("pocket-agent-recording-test-{pid}.wav"));
        let spec = hound::WavSpec { channels: 1, sample_rate: 16000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let record = |samples: &[i16]| {
            let mut w = hound::WavWriter::create(&shared, spec).unwrap();
            for &x in samples {
                w.write_sample(x).unwrap();
            }
            w
        };

        // Voiceprint recording stops; the daemon hands it to its own file.
        let dest = exclusive_clip_path("enroll");
        let enrolled = crate::voice::record::finish_recording(record(&[1; 160]), &shared, Some(&dest)).unwrap();
        assert_eq!(std::path::Path::new(&enrolled), dest.as_path());
        // While the HTTP request is in flight, the next dictation records.
        let next = crate::voice::record::finish_recording(record(&[7; 320]), &shared, None).unwrap();
        // The voiceprint request completes and cleans up.
        remove_temp_recording(std::path::Path::new(&enrolled));

        assert!(!dest.exists());
        let samples: Vec<i16> = hound::WavReader::open(&next).unwrap().samples().map(|x| x.unwrap()).collect();
        assert_eq!(samples, vec![7; 320]);
        let _ = std::fs::remove_file(&shared);
    }

    #[test]
    fn only_pa_recordings_in_the_temp_dir_are_deletable() {
        let temp = std::env::temp_dir();
        let pid = std::process::id();
        let ours = temp.join(format!("pocket-agent-phone-clip-test-{pid}.wav"));
        let foreign = temp.join(format!("other-{pid}.wav"));
        let not_wav = temp.join(format!("pocket-agent-test-{pid}.txt"));
        let sub = temp.join(format!("pa-voice-test-{pid}"));
        std::fs::create_dir_all(&sub).unwrap();
        let nested = sub.join("pocket-agent-recording.wav");
        let outside = sub.join("target.wav");
        let link = temp.join(format!("pocket-agent-link-{pid}.wav"));
        for f in [&ours, &foreign, &not_wav, &nested, &outside] {
            std::fs::write(f, b"x").unwrap();
        }
        remove_temp_recording(&ours);
        assert!(!ours.exists());
        for kept in [&foreign, &not_wav, &nested, &outside] {
            remove_temp_recording(kept);
            assert!(kept.exists(), "{} must stay", kept.display());
        }
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(&link);
            std::os::unix::fs::symlink(&outside, &link).unwrap();
            remove_temp_recording(&link);
            assert!(outside.exists(), "a link out of the temp dir is not followed");
            let _ = std::fs::remove_file(&link);
        }
        assert!(!is_pa_temp_recording(std::path::Path::new("/etc/hosts")));
        assert!(!is_pa_temp_recording(&temp.join("pocket-agent-missing.wav")));

        for f in [&foreign, &not_wav] {
            let _ = std::fs::remove_file(f);
        }
        let _ = std::fs::remove_dir_all(&sub);
    }
}
