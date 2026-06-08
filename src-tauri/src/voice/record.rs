// record.rs — Low-latency audio capture via pre-built stream daemon
//
// Architecture:
//   prewarm() spawns a daemon thread that pre-builds a cpal stream (the expensive part).
//   When hotkey fires, pre_start() signals the daemon -> daemon calls stream.play() (~10ms).
//   After each recording stops, daemon drops stream and pre-builds the next one.
//
// Latency: fn press -> first audio sample ~ 10-20ms (vs 200-500ms before)
//
// -- Capture ownership state machine (ENH-B3) ---------------------------------
//
// CAPTURE_OWNER is the single source of truth for who currently holds the
// microphone. All transitions go through CAPTURE_GATE.
//
//     from         to            allowed   notes
//     ----         --            -------   -----
//     NONE         SINGLE_SHOT      ✓
//     NONE         CONVERSATION     ✓
//     NONE         WAKE             ✓
//     SINGLE_SHOT  NONE             ✓      on stop / cancel / RAII drop
//     CONVERSATION NONE             ✓      on idle-end / stop
//     WAKE         NONE             ✓      on stop_wake_listener
//     WAKE         SINGLE_SHOT      ✓      hotkey pre-empts wake. wake_word
//                                          self-recovers after single-shot stop
//                                          iff wake_word_enabled (frontend re-arm).
//     WAKE         CONVERSATION     ✓      wake detection → conversation. wake
//                                          re-arms after conversation idle-end.
//     <other>      <other>          ✗      Err("capture busy: <prev>")
//     same         same             ✓ no-op (idempotent)
//
// SINGLE_SHOT and CONVERSATION never pre-empt each other — they must both go
// through NONE. The frontend surfaces an error toast in those cases.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use hound::{WavSpec, WavWriter};
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};

// Recording WAV path. Lives inside the OS temp dir (not hardcoded `/tmp`) so
// that speaker_verify's `validate_audio_path` containment check — which roots
// off `std::env::temp_dir()` — passes on macOS, where `$TMPDIR` is per-user
// under `/var/folders/...` rather than `/tmp`.
static RECORDING_PATH: OnceLock<PathBuf> = OnceLock::new();

pub fn recording_path() -> &'static Path {
    RECORDING_PATH
        .get_or_init(|| std::env::temp_dir().join("pocket-agent-recording.wav"))
        .as_path()
}

/// Global audio level indicator — updated by CPAL callback during recording.
/// Frontend polls this via Tauri command every ~200ms. Range: 0-1000 (0.0-1.0 * 1000)
pub static AUDIO_LEVEL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

// Ring buffer of recent mono i16 samples for VAD analysis.
// Stores ~3s at 16kHz = 48000 samples. Written by recording callback, read by VAD thread.
const VAD_RING_CAP: usize = 48000;
static VAD_RING: std::sync::Mutex<(Vec<i16>, usize)> = std::sync::Mutex::new((Vec::new(), 0));

/// Push mono samples into the VAD ring buffer (called from audio callback).
pub fn push_vad_samples(samples: &[i16]) {
    if let Ok(mut ring) = VAD_RING.lock() {
        let (buf, pos) = &mut *ring;
        if buf.len() < VAD_RING_CAP {
            let remaining = VAD_RING_CAP - buf.len();
            let take = samples.len().min(remaining);
            buf.extend_from_slice(&samples[..take]);
            *pos = buf.len() % VAD_RING_CAP;
            if take < samples.len() {
                let extra = &samples[take..];
                let write_len = extra.len().min(VAD_RING_CAP);
                buf[..write_len].copy_from_slice(&extra[..write_len]);
                *pos = write_len % VAD_RING_CAP;
            }
        } else {
            for &s in samples {
                buf[*pos] = s;
                *pos = (*pos + 1) % VAD_RING_CAP;
            }
        }
    }
}

/// Read recent mono samples from the VAD ring buffer (called by VAD thread).
/// Returns up to `max_samples` most recent samples in chronological order.
pub fn read_vad_samples(max_samples: usize) -> Vec<i16> {
    if let Ok(ring) = VAD_RING.lock() {
        let (buf, pos) = &*ring;
        let n = buf.len().min(max_samples);
        if n == 0 { return Vec::new(); }
        let start = if buf.len() >= VAD_RING_CAP {
            (*pos + buf.len() - n) % VAD_RING_CAP
        } else {
            buf.len().saturating_sub(n)
        };
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            out.push(buf[(start + i) % VAD_RING_CAP]);
        }
        out
    } else {
        Vec::new()
    }
}

/// Clear the VAD ring buffer.
pub fn clear_vad_samples() {
    if let Ok(mut ring) = VAD_RING.lock() {
        ring.0.clear();
        ring.1 = 0;
    }
}

// -- Capture owner ------------------------------------------------------------

pub const OWNER_NONE: u8 = 0;
pub const OWNER_SINGLE_SHOT: u8 = 1;
pub const OWNER_CONVERSATION: u8 = 2;
pub const OWNER_WAKE: u8 = 3;

/// Current owner of the capture device. See the state-machine comment above.
pub static CAPTURE_OWNER: AtomicU8 = AtomicU8::new(OWNER_NONE);

pub fn owner_name(o: u8) -> &'static str {
    match o {
        OWNER_NONE => "none",
        OWNER_SINGLE_SHOT => "single_shot",
        OWNER_CONVERSATION => "conversation",
        OWNER_WAKE => "wake",
        _ => "?",
    }
}

pub fn current_owner() -> u8 {
    CAPTURE_OWNER.load(Ordering::Acquire)
}

/// Atomically transition NONE → `owner`. Returns Err("capture busy: <prev>")
/// if the device is held by someone else. Same-owner re-acquire is a no-op.
/// Callers should hold CAPTURE_GATE across the broader check-and-commit window
/// so a peer can't slip in between this and any follow-up steps.
pub fn try_acquire_capture(owner: u8) -> Result<(), String> {
    match CAPTURE_OWNER.compare_exchange(
        OWNER_NONE,
        owner,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => Ok(()),
        Err(prev) if prev == owner => Ok(()),
        Err(prev) => Err(format!("capture busy: {}", owner_name(prev))),
    }
}

/// Release the capture device. No-op if the current owner is not `expected`
/// (defensive — keeps a stale release from clobbering a fresh owner).
pub fn release_capture(expected: u8) {
    let _ = CAPTURE_OWNER.compare_exchange(
        expected,
        OWNER_NONE,
        Ordering::AcqRel,
        Ordering::Acquire,
    );
}

/// Serializes the check-and-commit window of all capture transitions. Held
/// only briefly — never during actual capture — so contention is negligible.
pub static CAPTURE_GATE: Mutex<()> = Mutex::new(());

pub fn lock_capture_gate() -> std::sync::MutexGuard<'static, ()> {
    CAPTURE_GATE.lock().unwrap_or_else(|p| p.into_inner())
}

// -- Shared state between audio callback and daemon thread --

struct CaptureShared {
    writer: Mutex<Option<WavWriter<BufWriter<File>>>>,
    recording: AtomicBool,
    // Device-reported channel count for the interleaved input buffers. The WAV
    // file is always written as mono (the stt-server rejects anything else with
    // 415 expected_mono); when `input_channels > 1` we average each frame's
    // channels into a single sample before writing.
    input_channels: u16,
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
                let ch = self.input_channels.max(1) as usize;
                // Pick the channel with the largest magnitude per frame instead
                // of averaging. Averaging halves amplitude on devices that fill
                // only one channel of a stereo stream (common on USB/BT mics),
                // which can push the signal below the server's -30 dBFS enroll
                // threshold even for loud speech. Max-magnitude is equivalent
                // to averaging when both channels carry the same content.
                let mut mono_buf: Vec<i16> = Vec::with_capacity(data.len() / ch);
                for frame in data.chunks_exact(ch) {
                    let mono = frame.iter().copied().fold(0.0f32, |acc, s| {
                        if s.abs() > acc.abs() { s } else { acc }
                    });
                    let sample = (mono.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                    let _ = wr.write_sample(sample);
                    mono_buf.push(sample);
                }
                push_vad_samples(&mono_buf);
            }
        }
    }

    fn write_i16(&self, data: &[i16]) {
        if !self.recording.load(Ordering::Relaxed) {
            return;
        }
        if let Ok(mut g) = self.writer.try_lock() {
            if let Some(wr) = g.as_mut() {
                let ch = self.input_channels.max(1) as usize;
                let mut mono_buf: Vec<i16> = Vec::with_capacity(data.len() / ch);
                for frame in data.chunks_exact(ch) {
                    let mono = frame.iter().copied().fold(0i16, |acc, s| {
                        if s.unsigned_abs() > acc.unsigned_abs() { s } else { acc }
                    });
                    let _ = wr.write_sample(mono);
                    mono_buf.push(mono);
                }
                push_vad_samples(&mono_buf);
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

    let input_channels = supported.channels();
    // Always write mono — stt-server requires `channels == 1` (see _read_wav_bytes).
    // The capture callback downmixes interleaved frames before writing.
    let wav_spec = WavSpec {
        channels: 1,
        sample_rate: supported.sample_rate().0,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let sample_fmt = supported.sample_format();
    let stream_cfg: cpal::StreamConfig = supported.into();

    eprintln!(
        "[record] prewarm: device={} rate={} input_ch={} wav_ch={} fmt={:?}",
        device.name().unwrap_or_default(),
        wav_spec.sample_rate,
        input_channels,
        wav_spec.channels,
        sample_fmt
    );

    let (cmd_tx, cmd_rx) = mpsc::channel::<DaemonCmd>();

    std::thread::Builder::new()
        .name("audio-daemon".to_string())
        .spawn(move || daemon_loop(device, stream_cfg, sample_fmt, wav_spec, input_channels, cmd_rx))
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
    // Release the single-shot ownership regardless of whether the daemon
    // succeeds — once stop is requested, the slot is no longer ours.
    release_capture(OWNER_SINGLE_SHOT);
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
    input_channels: u16,
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
            input_channels,
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
                let writer = match WavWriter::create(recording_path(), wav_spec) {
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
                                        .map(|_| recording_path().to_string_lossy().into_owned())
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

// -- Streaming capture (parallel path for continuous-conversation + wake) --
//
// Independent of the prewarm daemon: opens its own cpal stream on a dedicated
// thread, delivers raw i16 PCM + sample rate to a user-supplied callback for
// each cpal buffer (~20ms at 48kHz). Aggregating into longer VAD windows is the
// caller's responsibility (see conversation.rs and wake_word.rs).
//
// Ownership is enforced via CAPTURE_OWNER (try_acquire_capture). Caller passes
// the owner constant; the dedicated thread guarantees release on exit via
// ReleaseOnDrop.

pub struct StreamingHandle {
    stop_tx: Sender<()>,
    /// Sample rate reported by the input device, in Hz.
    pub sample_rate: u32,
    /// Channel count reported by the input device.
    pub channels: u16,
}

/// Begin a long-lived capture session. The callback fires on cpal's audio
/// thread for every input buffer — keep it lightweight (e.g., push to a queue).
///
/// `owner` must be one of OWNER_CONVERSATION or OWNER_WAKE. The function
/// holds CAPTURE_GATE briefly while acquiring the owner; on success the
/// streaming thread owns release-on-exit.
pub fn start_streaming_capture<F>(owner: u8, callback: F) -> Result<StreamingHandle, String>
where
    F: FnMut(&[i16], u32) + Send + 'static,
{
    {
        let _gate = lock_capture_gate();
        try_acquire_capture(owner)?;
    }

    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(u32, u16), String>>();

    let cb = Arc::new(Mutex::new(callback));
    let owner_for_thread = owner;

    let spawn_result = std::thread::Builder::new()
        .name("audio-streaming".into())
        .spawn(move || {
            // Guarantee release whether the thread exits normally, via early
            // return, or via panic. AUDIO_LEVEL is also reset.
            struct ReleaseOnDrop(u8);
            impl Drop for ReleaseOnDrop {
                fn drop(&mut self) {
                    release_capture(self.0);
                    AUDIO_LEVEL.store(0, Ordering::Relaxed);
                    clear_vad_samples();
                }
            }
            let _release = ReleaseOnDrop(owner_for_thread);

            let host = cpal::default_host();
            let device = match host.default_input_device() {
                Some(d) => d,
                None => {
                    let _ = ready_tx.send(Err("no input device".into()));
                    return;
                }
            };
            let supported = match device.default_input_config() {
                Ok(c) => c,
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("config: {}", e)));
                    return;
                }
            };

            let sample_rate = supported.sample_rate().0;
            let channels = supported.channels();
            let sample_fmt = supported.sample_format();
            let stream_cfg: cpal::StreamConfig = supported.into();

            eprintln!(
                "[record] streaming(owner={}): device={} rate={} ch={} fmt={:?}",
                owner_name(owner_for_thread),
                device.name().unwrap_or_default(),
                sample_rate,
                channels,
                sample_fmt
            );

            let stream = match build_streaming_stream(&device, &stream_cfg, sample_fmt, sample_rate, cb) {
                Ok(s) => s,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };

            if let Err(e) = stream.play() {
                let _ = ready_tx.send(Err(format!("play: {}", e)));
                return;
            }

            let _ = ready_tx.send(Ok((sample_rate, channels)));
            eprintln!("[record] streaming started @ {}Hz", sample_rate);

            // Block until stop is signaled (or sender dropped — handle leaked).
            let _ = stop_rx.recv();

            drop(stream);
            eprintln!("[record] streaming stopped (owner={})", owner_name(owner_for_thread));
        });

    if let Err(e) = spawn_result {
        release_capture(owner);
        return Err(format!("spawn: {}", e));
    }

    match ready_rx.recv_timeout(std::time::Duration::from_secs(3)) {
        Ok(Ok((rate, ch))) => Ok(StreamingHandle {
            stop_tx,
            sample_rate: rate,
            channels: ch,
        }),
        // Thread holds ReleaseOnDrop and releases as it unwinds.
        Ok(Err(e)) => Err(e),
        Err(_) => {
            // Thread might still be running but wedged; ReleaseOnDrop will fire
            // when it does eventually exit. Force a release here too so a
            // subsequent acquire can succeed if the thread is permanently stuck.
            release_capture(owner);
            Err("streaming start timeout".into())
        }
    }
}

/// Stop a streaming session: signals the dedicated thread to drop its stream
/// and release the device. Idempotent if the thread already exited.
pub fn stop_streaming_capture(handle: StreamingHandle) {
    let _ = handle.stop_tx.send(());
    // handle drops here; the dedicated thread is detached and will release the
    // device + owner on its own once the stop signal is processed.
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
