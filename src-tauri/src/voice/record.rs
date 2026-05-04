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

/// Stop recording, finalize WAV, return file path
pub fn stop_recording(_handle: RecordingHandle) -> Result<String, String> {
    if let Some(daemon) = DAEMON.get() {
        daemon.start_ack.store(false, Ordering::Release);
        let (resp_tx, resp_rx) = mpsc::channel();
        daemon
            .cmd_tx
            .send(DaemonCmd::Stop(resp_tx))
            .map_err(|e| format!("send: {}", e))?;
        match resp_rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(result) => result,
            Err(_) => Err("stop timeout".into()),
        }
    } else {
        Err("audio not initialized".into())
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
                        Ok(DaemonCmd::Start(_)) => {
                            // Spurious Start while recording - ignore
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
