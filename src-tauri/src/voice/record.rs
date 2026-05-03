use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use hound::{WavSpec, WavWriter};
use std::fs::File;
use std::io::BufWriter;
use std::sync::{mpsc, Arc, Mutex};

use std::sync::OnceLock;

pub const RECORDING_PATH: &str = "/tmp/pocket-agent-recording.wav";

/// Pre-warmed audio device cache (initialized once on app start)
struct AudioCache {
    device: cpal::Device,
    sample_rate: u32,
    channels: u16,
    sample_format: SampleFormat,
    stream_config: cpal::StreamConfig,
}

static AUDIO_CACHE: OnceLock<AudioCache> = OnceLock::new();

/// Pre-warm audio device on app start to eliminate device discovery latency
pub fn prewarm() {
    let host = cpal::default_host();
    let device = match host.default_input_device() {
        Some(d) => d,
        None => { eprintln!("[record] prewarm: no mic found"); return; }
    };
    let supported = match device.default_input_config() {
        Ok(c) => c,
        Err(e) => { eprintln!("[record] prewarm: config error: {}", e); return; }
    };
    let sample_rate = supported.sample_rate().0;
    let channels = supported.channels();
    let sample_format = supported.sample_format();
    let stream_config: cpal::StreamConfig = supported.clone().into();

    eprintln!("[record] prewarmed: device={}, rate={}, ch={}, fmt={:?}",
        device.name().unwrap_or_default(), sample_rate, channels, sample_format);

    let _ = AUDIO_CACHE.set(AudioCache {
        device,
        sample_rate,
        channels,
        sample_format,
        stream_config,
    });
}

/// Pre-started recording handle (set by hotkey callback, consumed by Tauri command)
static PRE_STARTED: OnceLock<Mutex<Option<PreStartResult>>> = OnceLock::new();

struct PreStartResult {
    handle: RecordingHandle,
}

/// Called from hotkey callback to start recording IMMEDIATELY (no frontend round-trip)
pub fn pre_start() {
    let lock = PRE_STARTED.get_or_init(|| Mutex::new(None));
    match start_recording() {
        Ok(handle) => {
            if let Ok(mut g) = lock.lock() {
                *g = Some(PreStartResult { handle });
            }
            eprintln!("[record] pre-started recording from hotkey");
        }
        Err(e) => eprintln!("[record] pre-start failed: {}", e),
    }
}

/// Take the pre-started recording handle (called from Tauri command)
pub fn take_pre_started() -> Option<RecordingHandle> {
    PRE_STARTED.get().and_then(|lock| {
        lock.lock().ok().and_then(|mut g| g.take().map(|r| r.handle))
    })
}

/// Only holds Send types, no unsafe needed
pub struct RecordingHandle {
    /// Sending any message triggers stop
    stop_tx: mpsc::Sender<()>,
    /// Recording thread returns WAV path or error
    result_rx: mpsc::Receiver<Result<String, String>>,
}

/// Start recording: spawns a dedicated OS thread, cpal Stream never crosses threads
pub fn start_recording() -> Result<RecordingHandle, String> {
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let (result_tx, result_rx) = mpsc::channel::<Result<String, String>>();

    std::thread::Builder::new()
        .name("recording".to_string())
        .spawn(move || {
            recording_thread(stop_rx, result_tx);
        })
        .map_err(|e| format!("启动录音线程失败: {}", e))?;

    Ok(RecordingHandle { stop_tx, result_rx })
}

/// Stop recording, wait for WAV finalization, return file path
pub fn stop_recording(handle: RecordingHandle) -> Result<String, String> {
    let _ = handle.stop_tx.send(());

    handle
        .result_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|_| "等待录音结束超时".to_string())?
}

/// Recording thread: creates and owns cpal Stream until stop signal
fn recording_thread(
    stop_rx: mpsc::Receiver<()>,
    result_tx: mpsc::Sender<Result<String, String>>,
) {
    let send = |r: Result<String, String>| {
        let _ = result_tx.send(r);
    };

    // Try pre-warmed device first, fallback to fresh discovery
    let (device, spec, sample_fmt, stream_cfg) = if let Some(cache) = AUDIO_CACHE.get() {
        eprintln!("[record] using pre-warmed device");
        let spec = WavSpec {
            channels: cache.channels,
            sample_rate: cache.sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        (cache.device.clone(), spec, cache.sample_format, cache.stream_config.clone())
    } else {
        eprintln!("[record] no pre-warmed device, discovering...");
        let host = cpal::default_host();
        let device = match host.default_input_device() {
            Some(d) => d,
            None => { send(Err("没有可用麦克风".to_string())); return; }
        };
        let config = match device.default_input_config() {
            Ok(c) => c,
            Err(e) => { send(Err(format!("获取输入配置失败: {}", e))); return; }
        };
        let spec = WavSpec {
            channels: config.channels(),
            sample_rate: config.sample_rate().0,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let sample_fmt = config.sample_format();
        let stream_cfg: cpal::StreamConfig = config.into();
        eprintln!("[record] device={}, rate={}, channels={}, fmt={:?}",
            device.name().unwrap_or_default(),
            spec.sample_rate, spec.channels, sample_fmt);
        (device, spec, sample_fmt, stream_cfg)
    };

    let writer = match WavWriter::create(RECORDING_PATH, spec) {
        Ok(w) => Arc::new(Mutex::new(Some(w))),
        Err(e) => { send(Err(format!("创建 WAV 文件失败: {}", e))); return; }
    };

    let err_fn = |e| eprintln!("[record] stream error: {}", e);

    let stream = {
        let w = Arc::clone(&writer);
        let result = match sample_fmt {
            SampleFormat::F32 => device.build_input_stream(
                &stream_cfg,
                move |data: &[f32], _| write_f32(data, &w),
                err_fn,
                None,
            ),
            SampleFormat::I16 => device.build_input_stream(
                &stream_cfg,
                move |data: &[i16], _| write_i16(data, &w),
                err_fn,
                None,
            ),
            fmt => { send(Err(format!("不支持的采样格式: {:?}", fmt))); return; }
        };

        match result {
            Ok(s) => s,
            Err(e) => { send(Err(format!("构建输入流失败: {}", e))); return; }
        }
    };

    if let Err(e) = stream.play() {
        send(Err(format!("启动录音失败: {}", e)));
        return;
    }

    eprintln!("[record] recording…");

    // Block until stop signal (Stream stays alive on this thread)
    stop_rx.recv().ok();

    // Stop stream, then finalize WAV
    drop(stream);

    let result = if let Ok(mut guard) = writer.lock() {
        match guard.take() {
            Some(w) => w
                .finalize()
                .map(|_| RECORDING_PATH.to_string())
                .map_err(|e| format!("WAV finalize 失败: {}", e)),
            None => Err("WAV writer 已释放".to_string()),
        }
    } else {
        Err("无法获取 WAV writer 锁".to_string())
    };

    eprintln!("[record] done: {:?}", result);
    send(result);
}

fn write_f32(data: &[f32], w: &Arc<Mutex<Option<WavWriter<BufWriter<File>>>>>) {
    if let Ok(mut g) = w.try_lock() {
        if let Some(wr) = g.as_mut() {
            for &s in data {
                let _ = wr.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
            }
        }
    }
}

fn write_i16(data: &[i16], w: &Arc<Mutex<Option<WavWriter<BufWriter<File>>>>>) {
    if let Ok(mut g) = w.try_lock() {
        if let Some(wr) = g.as_mut() {
            for &s in data {
                let _ = wr.write_sample(s);
            }
        }
    }
}
