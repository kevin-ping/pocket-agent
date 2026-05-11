use chrono;
use crate::voice::record::{start_recording, stop_recording_no_handle, take_pre_started, RecordingHandle};
use crate::voice::stt::transcribe;
use std::sync::Mutex;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, State};

const MAX_RECORDING_SECS: u64 = 30;
const MIN_RECORDING_SECS: f32 = 1.5;

/// Read WAV duration from file header (no decoding needed)
fn wav_duration_secs(path: &str) -> Result<f32, String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| format!("打开WAV失败: {}", e))?;
    let mut header = [0u8; 44];
    f.read_exact(&mut header).map_err(|e| format!("读取WAV头失败: {}", e))?;
    // WAV header: sample_rate at offset 24 (4 bytes LE), data_chunk_size at offset 40 (4 bytes LE)
    let sample_rate = u32::from_le_bytes([header[24], header[25], header[26], header[27]]);
    let data_size = u32::from_le_bytes([header[40], header[41], header[42], header[43]]);
    if sample_rate == 0 {
        return Ok(0.0);
    }
    // 16-bit stereo = 4 bytes per sample
    Ok(data_size as f32 / (sample_rate as f32 * 4.0))
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

#[tauri::command]
pub fn start_voice_recording(
    app: AppHandle,
    state: State<'_, RecordingState>,
) -> Result<(), String> {
    {
        let guard = state
            .handle
            .lock()
            .map_err(|_| "录音状态锁定失败".to_string())?;
        if guard.is_some() {
            return Ok(());
        }
    }

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

    // Mark timeout as active
    state.timeout_active.store(true, Ordering::SeqCst);
    let timeout_flag = state.timeout_active.clone();
    let app_clone = app.clone();

    // Spawn auto-timeout thread (std::thread works from sync context, tokio::spawn does not)
    std::thread::Builder::new()
        .name("recording-timeout".to_string())
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(MAX_RECORDING_SECS));
            // Only trigger if timeout wasn't cancelled (user didn't manually stop)
            if timeout_flag.compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                eprintln!("[voice] auto-timeout after {}s, stopping recording", MAX_RECORDING_SECS);
                // Emit fn-key-up to trigger the normal stop flow via App.svelte
                let _ = app_clone.emit("fn-key-up", ());
            }
        })
        .map_err(|e| format!("启动超时线程失败: {}", e))?;

    eprintln!("[voice] recording started (max {}s)", MAX_RECORDING_SECS);
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