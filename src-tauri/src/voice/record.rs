// record.rs — Low-latency audio capture via pre-built stream daemon
//
// Architecture:
//   prewarm() spawns a daemon thread that pre-builds a cpal stream (the expensive part).
//   When hotkey fires, pre_start() signals the daemon -> daemon calls stream.play() (~10ms).
//   After each recording stops, daemon drops stream and pre-builds the next one.
//
// Latency: fn press -> first audio sample ~ 10-20ms (vs 200-500ms before)

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use hound::{WavSpec, WavWriter};
use std::fs::File;
use std::io::BufWriter;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};

pub const RECORDING_PATH: &str = "/tmp/pocket-agent-recording.wav";

/// Global audio level indicator — updated by CPAL callback during recording.
/// Frontend polls this via Tauri command every ~200ms. Range: 0-1000 (0.0-1.0 * 1000)
pub static AUDIO_LEVEL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// True when start_streaming_capture is engaged. Single-shot recording refuses
/// to start while this is set, and vice versa, to keep the input device sane.
pub static STREAMING_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Set synchronously by the single-shot entry as soon as it commits, so the
/// streaming entry can see the reservation even before the daemon thread has
/// finished raising `start_ack` (~10ms async). Cleared by stop/cancel.
pub static SINGLE_SHOT_RESERVED: AtomicBool = AtomicBool::new(false);

/// Serializes capture *reservation* across single-shot and streaming entry
/// points. Held only across the start-time check+set window — never during
/// actual capture — so contention is negligible. Eliminates the TOCTOU window
/// where both paths could simultaneously observe each other as inactive.
pub static CAPTURE_GATE: Mutex<()> = Mutex::new(());

/// Acquire the capture gate, recovering silently from a poisoned mutex.
/// The gate guards atomic-flag checks, not user data; poison from a panicked
/// caller doesn't invalidate the underlying state.
pub fn lock_capture_gate() -> std::sync::MutexGuard<'static, ()> {
    CAPTURE_GATE.lock().unwrap_or_else(|p| p.into_inner())
}

// -- Shared state between audio callback and daemon thread --

struct CaptureShared {
    writer: Mutex<Option<WavWriter<BufWriter<File>>>>,
    recording: AtomicBool,
}

impl CaptureShared {
    fn write_f32(&self, data: &[f32]) {
        if !self.recording.load(Ordering::Relaxed) {
            return;
        }
        // Compute RMS audio level and store in global for frontend polling
        // Every buffer ~1024 samples at 48kHz ≈ 21ms — fast enough for level meter
        let rms = (data.iter().map(|&s| s * s).sum::<f32>() / data.len() as f32).sqrt();
        let db = if rms > 0.001 { 20.0 * rms.log10() } else { -60.0 };
        let level = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        AUDIO_LEVEL.store((level * 1000.0) as u32, std::sync::atomic::Ordering::Relaxed);

        if let Ok(mut g) = self.writer.try_lock() {
            if let Some(wr) = g.as_mut() {
                for &s in data {
                    let _ = wr.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
                }
            }
        }
    }

    fn write_i16(&self, data: &[i16]) {
        if !self.recording.load(Ordering::Relaxed) {
            return;
        }
        if let Ok(mut g) = self.writer.try_lock() {
            if let Some(wr) = g.as_mut() {
                for &s in data {
                    let _ = wr.write_sample(s);
                }
            }
        }
    }
}

// -- Daemon control --

enum DaemonCmd {
    Start(Option<Sender<Result<(), String>>>),
    Stop(Sender<Result<String, String>>),
}

struct Daemon {
    cmd_tx: Sender<DaemonCmd>,
    stream_ready: AtomicBool,
    start_ack: AtomicBool,
}

static DAEMON: OnceLock<Daemon> = OnceLock::new();

// -- Public API --

pub struct RecordingHandle {
    _priv: (),
}

/// Initialize at app startup: discover device, spawn daemon thread
pub fn prewarm() {
    if DAEMON.get().is_some() {
        return;
    }

    let host = cpal::default_host();
    let device = match host.default_input_device() {
        Some(d) => d,
        None => {
            eprintln!("[record] prewarm: no mic found");
            return;
        }
    };
    let supported = match device.default_input_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[record] prewarm: config error: {}", e);
            return;
        }
    };

    let wav_spec = WavSpec {
        channels: supported.channels(),
        sample_rate: supported.sample_rate().0,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let sample_fmt = supported.sample_format();
    let stream_cfg: cpal::StreamConfig = supported.into();

    eprintln!(
        "[record] prewarm: device={} rate={} ch={} fmt={:?}",
        device.name().unwrap_or_default(),
        wav_spec.sample_rate,
        wav_spec.channels,
        sample_fmt
    );

    let (cmd_tx, cmd_rx) = mpsc::channel::<DaemonCmd>();

    std::thread::Builder::new()
        .name("audio-daemon".to_string())
        .spawn(move || daemon_loop(device, stream_cfg, sample_fmt, wav_spec, cmd_rx))
        .expect("audio daemon spawn");

    DAEMON.get_or_init(|| Daemon {
        cmd_tx,
        stream_ready: AtomicBool::new(false),
        start_ack: AtomicBool::new(false),
    });
}

/// Non-blocking: called from CGEvent hotkey callback.
/// Sends Start to daemon; daemon calls stream.play() on its own thread.
pub fn pre_start() {
    if let Some(daemon) = DAEMON.get() {
        // Send Start even if stream not ready — daemon will process it
        // after Phase 1 (stream build) completes
        daemon.start_ack.store(false, Ordering::Release);
        let _ = daemon.cmd_tx.send(DaemonCmd::Start(None));
        eprintln!("[record] pre_start: Start sent to daemon (stream_ready={})", 
            daemon.stream_ready.load(Ordering::Acquire));
    }
}

/// Check if pre_start succeeded (called from Tauri command, ~50-200ms after hotkey)
pub fn take_pre_started() -> Option<RecordingHandle> {
    DAEMON.get().and_then(|d| {
        if d.start_ack.load(Ordering::Acquire) {
            Some(RecordingHandle { _priv: () })
        } else {
            None
        }
    })
}

/// Fallback: start recording synchronously (called if take_pre_started returns None)
pub fn start_recording() -> Result<RecordingHandle, String> {
    if let Some(daemon) = DAEMON.get() {
        if daemon.start_ack.load(Ordering::Acquire) {
            return Ok(RecordingHandle { _priv: () });
        }
        let (resp_tx, resp_rx) = mpsc::channel();
        daemon
            .cmd_tx
            .send(DaemonCmd::Start(Some(resp_tx)))
            .map_err(|e| format!("send: {}", e))?;
        match resp_rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(Ok(())) => {
                daemon.start_ack.store(true, Ordering::Release);
                Ok(RecordingHandle { _priv: () })
            }
            Ok(Err(e)) => Err(e),
            Err(_) => Err("start timeout".into()),
        }
    } else {
        Err("audio not initialized".into())
    }
}

/// Stop recording without requiring a handle (for race-safe stop)
pub fn stop_recording_no_handle() -> Result<String, String> {
    stop_recording_internal()
}

fn stop_recording_internal() -> Result<String, String> {
    // Release the single-shot reservation regardless of whether the daemon
    // succeeds — once stop is requested, the single-shot slot is no longer ours.
    SINGLE_SHOT_RESERVED.store(false, Ordering::Release);
    let daemon = DAEMON.get().ok_or_else(|| "audio not initialized".to_string())?;
    daemon.start_ack.store(false, Ordering::Release);
    let (resp_tx, resp_rx) = mpsc::channel();
    daemon
        .cmd_tx
        .send(DaemonCmd::Stop(resp_tx))
        .map_err(|e| format!("send: {}", e))?;
    // Daemon may still be in Phase 1 (build_stream ~200ms) when Stop arrives —
    // give it enough headroom to walk through Phase 1 → Phase 2 → Phase 3 → finalize.
    match resp_rx.recv_timeout(std::time::Duration::from_secs(8)) {
        Ok(result) => result,
        Err(_) => Err("stop timeout".into()),
    }
}

// -- Daemon implementation --

fn daemon_loop(
    device: cpal::Device,
    stream_cfg: cpal::StreamConfig,
    sample_fmt: SampleFormat,
    wav_spec: WavSpec,
    cmd_rx: Receiver<DaemonCmd>,
) {
    let daemon = match DAEMON.get() {
        Some(d) => d,
        None => return,
    };

    loop {
        // -- Phase 1: Pre-build stream (expensive, ~200ms) --
        let shared = Arc::new(CaptureShared {
            writer: Mutex::new(None),
            recording: AtomicBool::new(false),
        });

        let shared_cb = shared.clone();
        let stream = match build_stream(&device, &stream_cfg, sample_fmt, shared_cb) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[record] build failed: {}", e);
                std::thread::sleep(std::time::Duration::from_secs(1));
                continue;
            }
        };

        daemon.stream_ready.store(true, Ordering::Release);
        eprintln!("[record] stream ready (pre-built)");

        // -- Phase 2: Wait for Start command --
        let start_cmd = match cmd_rx.recv() {
            Ok(c) => c,
            Err(_) => break,
        };

        match start_cmd {
            DaemonCmd::Start(resp) => {
                daemon.stream_ready.store(false, Ordering::Release);

                // Create WAV file
                let writer = match WavWriter::create(RECORDING_PATH, wav_spec) {
                    Ok(w) => w,
                    Err(e) => {
                        eprintln!("[record] WAV create failed: {}", e);
                        if let Some(r) = resp {
                            let _ = r.send(Err(format!("WAV: {}", e)));
                        }
                        continue;
                    }
                };
                *shared.writer.lock().unwrap() = Some(writer);

                // Start capture - stream already built, this is fast (~10ms)
                if let Err(e) = stream.play() {
                    eprintln!("[record] play failed: {}", e);
                    if let Some(r) = resp {
                        let _ = r.send(Err(format!("play: {}", e)));
                    }
                    continue;
                }

                shared.recording.store(true, Ordering::Release);
                daemon.start_ack.store(true, Ordering::Release);

                if let Some(r) = resp {
                    let _ = r.send(Ok(()));
                }
                eprintln!("[record] recording started");

                // -- Phase 3: Wait for Stop command --
                loop {
                    match cmd_rx.recv() {
                        Ok(DaemonCmd::Stop(stop_resp)) => {
                            shared.recording.store(false, Ordering::Release);
                            let result = {
                                let mut g = shared.writer.lock().unwrap();
                                match g.take() {
                                    Some(w) => w
                                        .finalize()
                                        .map(|_| RECORDING_PATH.to_string())
                                        .map_err(|e| format!("finalize: {}", e)),
                                    None => Err("no writer".into()),
                                }
                            };
                            let _ = stop_resp.send(result);
                            eprintln!("[record] recording stopped");
                            break;
                        }
                        Ok(DaemonCmd::Start(resp)) => {
                            // Spurious Start while recording — already started, ack immediately
                            // so the caller's recv_timeout doesn't hang.
                            if let Some(r) = resp {
                                let _ = r.send(Ok(()));
                            }
                        }
                        Err(_) => return,
                    }
                }
                // stream dropped here -> audio hardware released
                // Loop back to Phase 1 to pre-build next stream
            }
            DaemonCmd::Stop(resp) => {
                let _ = resp.send(Err("not recording".into()));
            }
        }
    }
    eprintln!("[record] daemon exit");
}

fn stream_error(e: cpal::StreamError) {
    eprintln!("[record] stream error: {}", e);
}

fn build_stream(
    device: &cpal::Device,
    cfg: &cpal::StreamConfig,
    fmt: SampleFormat,
    shared: Arc<CaptureShared>,
) -> Result<cpal::Stream, String> {
    match fmt {
        SampleFormat::F32 => device.build_input_stream(
            cfg,
            move |d: &[f32], _| shared.write_f32(d),
            stream_error,
            None,
        ),
        SampleFormat::I16 => device.build_input_stream(
            cfg,
            move |d: &[i16], _| shared.write_i16(d),
            stream_error,
            None,
        ),
        f => return Err(format!("unsupported format: {:?}", f)),
    }
    .map_err(|e| format!("build_input_stream: {}", e))
}

// -- Streaming capture (parallel path for continuous-conversation mode) --
//
// Independent of the prewarm daemon: opens its own cpal stream on a dedicated
// thread, delivers raw i16 PCM + sample rate to a user-supplied callback for
// each cpal buffer (~20ms at 48kHz). Aggregating into longer VAD windows is the
// caller's responsibility (see conversation.rs).
//
// Mutual exclusion with the single-shot path is enforced via STREAMING_ACTIVE.

pub struct StreamingHandle {
    stop_tx: Sender<()>,
    /// Sample rate reported by the input device, in Hz.
    pub sample_rate: u32,
    /// Channel count reported by the input device.
    pub channels: u16,
}

/// Begin a long-lived capture session. The callback fires on cpal's audio
/// thread for every input buffer — keep it lightweight (e.g., push to a queue).
pub fn start_streaming_capture<F>(callback: F) -> Result<StreamingHandle, String>
where
    F: FnMut(&[i16], u32) + Send + 'static,
{
    // Hold the capture gate across the entire check-and-reserve step so the
    // single-shot entry can't slip between our two flag reads.
    let _gate = lock_capture_gate();

    // SINGLE_SHOT_RESERVED is flipped synchronously by the single-shot entry;
    // start_ack is the daemon's async confirmation. Check both.
    if SINGLE_SHOT_RESERVED.load(Ordering::Acquire) {
        return Err("single-shot recording in progress".into());
    }
    if let Some(d) = DAEMON.get() {
        if d.start_ack.load(Ordering::Acquire) {
            return Err("single-shot recording in progress".into());
        }
    }

    if STREAMING_ACTIVE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("streaming capture already active".into());
    }
    drop(_gate);

    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(u32, u16), String>>();

    let cb = Arc::new(Mutex::new(callback));

    std::thread::Builder::new()
        .name("audio-streaming".into())
        .spawn(move || {
            let host = cpal::default_host();
            let device = match host.default_input_device() {
                Some(d) => d,
                None => {
                    let _ = ready_tx.send(Err("no input device".into()));
                    STREAMING_ACTIVE.store(false, Ordering::Release);
                    return;
                }
            };
            let supported = match device.default_input_config() {
                Ok(c) => c,
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("config: {}", e)));
                    STREAMING_ACTIVE.store(false, Ordering::Release);
                    return;
                }
            };

            let sample_rate = supported.sample_rate().0;
            let channels = supported.channels();
            let sample_fmt = supported.sample_format();
            let stream_cfg: cpal::StreamConfig = supported.into();

            eprintln!(
                "[record] streaming: device={} rate={} ch={} fmt={:?}",
                device.name().unwrap_or_default(),
                sample_rate,
                channels,
                sample_fmt
            );

            let stream = match build_streaming_stream(&device, &stream_cfg, sample_fmt, sample_rate, cb) {
                Ok(s) => s,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    STREAMING_ACTIVE.store(false, Ordering::Release);
                    return;
                }
            };

            if let Err(e) = stream.play() {
                let _ = ready_tx.send(Err(format!("play: {}", e)));
                STREAMING_ACTIVE.store(false, Ordering::Release);
                return;
            }

            let _ = ready_tx.send(Ok((sample_rate, channels)));
            eprintln!("[record] streaming started @ {}Hz", sample_rate);

            // Block until stop is signaled (or sender dropped — handle leaked).
            let _ = stop_rx.recv();

            drop(stream);
            STREAMING_ACTIVE.store(false, Ordering::Release);
            AUDIO_LEVEL.store(0, Ordering::Relaxed);
            eprintln!("[record] streaming stopped");
        })
        .map_err(|e| {
            STREAMING_ACTIVE.store(false, Ordering::Release);
            format!("spawn: {}", e)
        })?;

    match ready_rx.recv_timeout(std::time::Duration::from_secs(3)) {
        Ok(Ok((rate, ch))) => Ok(StreamingHandle {
            stop_tx,
            sample_rate: rate,
            channels: ch,
        }),
        Ok(Err(e)) => Err(e),
        Err(_) => {
            STREAMING_ACTIVE.store(false, Ordering::Release);
            Err("streaming start timeout".into())
        }
    }
}

/// Stop a streaming session: signals the dedicated thread to drop its stream
/// and release the device. Idempotent if the thread already exited.
pub fn stop_streaming_capture(handle: StreamingHandle) {
    let _ = handle.stop_tx.send(());
    // handle drops here; the dedicated thread is detached and will release the
    // device on its own once the stop signal is processed.
}

fn build_streaming_stream<F>(
    device: &cpal::Device,
    cfg: &cpal::StreamConfig,
    fmt: SampleFormat,
    sample_rate: u32,
    cb: Arc<Mutex<F>>,
) -> Result<cpal::Stream, String>
where
    F: FnMut(&[i16], u32) + Send + 'static,
{
    fn update_level_f32(data: &[f32]) {
        if data.is_empty() {
            return;
        }
        let rms = (data.iter().map(|&s| s * s).sum::<f32>() / data.len() as f32).sqrt();
        let db = if rms > 0.001 { 20.0 * rms.log10() } else { -60.0 };
        let level = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        AUDIO_LEVEL.store((level * 1000.0) as u32, Ordering::Relaxed);
    }
    fn update_level_i16(data: &[i16]) {
        if data.is_empty() {
            return;
        }
        let rms = (data
            .iter()
            .map(|&s| {
                let f = s as f32 / i16::MAX as f32;
                f * f
            })
            .sum::<f32>()
            / data.len() as f32)
            .sqrt();
        let db = if rms > 0.001 { 20.0 * rms.log10() } else { -60.0 };
        let level = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        AUDIO_LEVEL.store((level * 1000.0) as u32, Ordering::Relaxed);
    }

    match fmt {
        SampleFormat::F32 => {
            let cb = cb.clone();
            device.build_input_stream(
                cfg,
                move |d: &[f32], _| {
                    update_level_f32(d);
                    let buf: Vec<i16> = d
                        .iter()
                        .map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                        .collect();
                    if let Ok(mut cb) = cb.lock() {
                        cb(&buf, sample_rate);
                    }
                },
                stream_error,
                None,
            )
        }
        SampleFormat::I16 => {
            let cb = cb.clone();
            device.build_input_stream(
                cfg,
                move |d: &[i16], _| {
                    update_level_i16(d);
                    if let Ok(mut cb) = cb.lock() {
                        cb(d, sample_rate);
                    }
                },
                stream_error,
                None,
            )
        }
        f => return Err(format!("unsupported format: {:?}", f)),
    }
    .map_err(|e| format!("build_input_stream: {}", e))
}
