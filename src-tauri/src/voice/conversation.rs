// conversation.rs — Continuous-conversation state machine
//
// Drives multi-turn voice ↔ TTS interaction:
//   start → Listening (energy-threshold VAD on streaming PCM)
//        → segment-end (≥700ms silence after ≥600ms speech)
//        → Transcribing (POST /transcribe in a worker thread)
//        → emit "stt-result" → Speaking
//        → on TtsDone (signalled from the frontend) → Listening
//        → if Listening idle for `silence_timeout_s` → emit "conversation-ended" → Idle
//
// During Speaking, VAD keeps running: if the user starts talking, we trigger
// barge-in (stop_audio_queue + emit "conversation-barge-in") and pre-load the
// fresh utterance into a new Listening buffer.
//
// All cross-thread coordination flows through a single mpsc channel into the
// worker thread, so the worker is the sole owner of mutable state — no shared
// locks beyond the channel itself and the active-flag atomic.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use hound::{SampleFormat as HoundSampleFormat, WavSpec, WavWriter};
use tauri::{AppHandle, Emitter};

use crate::voice::record::{
    current_owner, start_streaming_capture, stop_streaming_capture, StreamingHandle,
    OWNER_CONVERSATION, OWNER_NONE, OWNER_WAKE,
};
use crate::voice::stt::{transcribe, SttResult};

/// Fallback default for the per-utterance silence flush window. The live value
/// comes from `pause_tolerance_ms` passed in by the frontend; this constant is
/// only used when the frontend omits it. Keep low enough that an unconfigured
/// install still feels responsive.
const SEGMENT_END_SILENCE_MS: u64 = 1500;
/// Lowered from 600 ms because energy-VAD with hysteresis still occasionally
/// produces short bursts (e.g. "嗯", "是"). 300 ms is long enough to filter
/// stray clicks/coughs but short enough that brief replies still flush.
const MIN_UTTERANCE_MS: u64 = 300;
const MAX_UTTERANCE_S: f32 = 30.0;
/// Listening floor RMS (normalized to [0, 1]). ~ -36 dBFS. Tuned for typical
/// laptop built-in mic at conversational distance. Combined with an adaptive
/// noise-floor EMA (see worker_loop) so background noise can't keep the
/// segment-end-silence detector permanently armed.
const SPEECH_RMS_THRESHOLD: f32 = 0.015;
/// Multiplier applied to the rolling noise-floor EMA when computing the
/// effective speech threshold. Effective = max(SPEECH_RMS_THRESHOLD,
/// noise_floor * N).min(CAP). Kept light so speech still triggers easily.
const NOISE_FLOOR_MARGIN: f32 = 1.6;
/// Hard ceiling on the effective speech threshold — prevents runaway adaptive
/// floor from making the mic effectively deaf in noisy rooms.
const EFFECTIVE_THRESHOLD_CAP: f32 = 0.03;
/// Hysteresis release floor: once we're in a speech burst, RMS must drop below
/// this to exit the burst. Set well above the captured ambient noise floor
/// (~0.0005) but below typical voice-valley RMS (~0.004-0.012), so consonant→
/// vowel transitions don't drop us out of speech mid-word.
const SPEECH_RELEASE_RMS_THRESHOLD: f32 = 0.003;
/// Release-threshold companion to NOISE_FLOOR_MARGIN: the effective release
/// also scales with noise so a noisy room doesn't trap us in burst=true.
const RELEASE_NOISE_FLOOR_MARGIN: f32 = 1.2;
/// Window after TtsStarted during which Speaking-mode VAD is suppressed.
/// Covers TTS playback ramp-up and the early speaker-echo burst.
const BARGE_IN_WARMUP_MS: u64 = 600;
/// Continuous above-threshold speech required to actually fire barge-in.
/// Filters short TTS-echo bursts (typically < 200 ms) while letting real
/// sustained speech interrupt.
const BARGE_IN_MIN_SUSTAINED_MS: u64 = 350;
/// Louder RMS bar applied in Speaking mode only, so TTS bleed must clear it
/// continuously for BARGE_IN_MIN_SUSTAINED_MS before interrupting. ~ -28 dBFS.
const BARGE_IN_RMS_THRESHOLD: f32 = 0.04;
/// Release threshold for barge-in hysteresis.  Once barge-in accumulation
/// starts, RMS must drop below this to reset the run counter.  Prevents
/// syllable gaps in TTS echo (or user speech) from clearing the counter.
/// Set well below BARGE_IN_RMS_THRESHOLD so normal inter-syllable dips
/// (which briefly touch 0.035-0.039) don't break the sustained run.
const BARGE_IN_RELEASE_RMS: f32 = 0.02;
const SPEAKING_SAFETY_TIMEOUT_S: u64 = 30;
const TICK_MS: u64 = 100;
/// Max consecutive empty STT results before auto-ending the conversation.
/// Each empty result means VAD detected sound but Silero/Whisper found no human speech
/// (birds, clicks, background noise). After this many in a row, end the conversation
/// and return to wake/Fn-key idle state.
const MAX_CONSECUTIVE_EMPTY_STT: u32 = 1;

// Use the user-configured silence_timeout_s everywhere — no hardcoded override.
const CONVERSATION_WAV_PATH: &str = "/tmp/pocket-agent-conversation.wav";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    Listening,
    Transcribing,
    Speaking,
}

enum Msg {
    AudioChunk(Vec<i16>, u32),
    SttResult(Result<SttResult, String>),
    TtsStarted,
    TtsDone,
    Stop,
}

/// Slot holds the current worker's generation id + sender. Generation increments on
/// every `start_conversation` so a late-exiting worker can detect when its slot has
/// already been replaced and skip cleanup.
static WORKER_TX: OnceLock<Mutex<Option<(u64, Sender<Msg>)>>> = OnceLock::new();
static ACTIVE: AtomicBool = AtomicBool::new(false);
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn worker_tx_slot() -> &'static Mutex<Option<(u64, Sender<Msg>)>> {
    WORKER_TX.get_or_init(|| Mutex::new(None))
}

fn send_msg(msg: Msg) -> bool {
    if let Ok(guard) = worker_tx_slot().lock() {
        if let Some((_, tx)) = guard.as_ref() {
            return tx.send(msg).is_ok();
        }
    }
    false
}

pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

pub fn on_tts_started() {
    let _ = send_msg(Msg::TtsStarted);
}

pub fn on_tts_done() {
    let _ = send_msg(Msg::TtsDone);
}

pub fn stop_conversation() {
    let _ = send_msg(Msg::Stop);
}

pub fn start_conversation(
    app: AppHandle,
    silence_timeout_s: Option<u64>,
    pause_tolerance_ms: Option<u64>,
    speech_rms_threshold: Option<f32>,
    single_shot: Option<bool>,
    barge_in_rms_threshold: Option<f32>,
    barge_in_enabled: Option<bool>,
) -> Result<(), String> {
    if ACTIVE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("conversation already active".into());
    }

    // Owner-transition gate (record.rs file-header doc):
    //   SINGLE_SHOT  → CONVERSATION  ✗ refused (different active flow)
    //   WAKE         → CONVERSATION  ✓ pre-empt: stop wake, frontend re-arms
    //   NONE         → CONVERSATION  ✓
    //   CONVERSATION → CONVERSATION  ✗ already caught by ACTIVE check above
    match current_owner() {
        OWNER_WAKE => {
            crate::voice::sherpa_wake::stop_wake_listener();
            // stop_wake_listener joins the worker, but the audio-streaming
            // thread releases the capture device asynchronously via Drop.
            // Spin briefly until it's done so start_streaming_capture won't
            // hit "capture busy: wake".
            for _ in 0..50 {
                if current_owner() == OWNER_NONE {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        OWNER_NONE | OWNER_CONVERSATION => {}
        _ => {
            ACTIVE.store(false, Ordering::Release);
            return Err("单次录音进行中，无法启动连续对话".into());
        }
    }

    let (tx, rx) = mpsc::channel::<Msg>();
    let tx_for_cpal = tx.clone();

    let stream_handle = match start_streaming_capture(OWNER_CONVERSATION, move |samples, sr| {
        let _ = tx_for_cpal.send(Msg::AudioChunk(samples.to_vec(), sr));
    }) {
        Ok(h) => h,
        Err(e) => {
            ACTIVE.store(false, Ordering::Release);
            return Err(format!("streaming capture failed: {}", e));
        }
    };

    // Reserve a fresh generation id; the worker uses it to scope its cleanup so a
    // late-exiting old worker can't clobber the slot of a newly-started session.
    let my_gen = GENERATION.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
    {
        let mut guard = worker_tx_slot()
            .lock()
            .map_err(|_| "worker tx poisoned".to_string())?;
        *guard = Some((my_gen, tx));
    }

    let timeout = silence_timeout_s.unwrap_or(5).clamp(2, 30);
    let pause_tolerance = pause_tolerance_ms
        .unwrap_or(SEGMENT_END_SILENCE_MS)
        .clamp(500, 5000);
    let speech_threshold = speech_rms_threshold
        .unwrap_or(SPEECH_RMS_THRESHOLD)
        .clamp(0.003, 0.030);
    let app_clone = app.clone();

    std::thread::Builder::new()
        .name("conversation-worker".into())
        .spawn(move || {
            worker_loop(
                app_clone,
                rx,
                stream_handle,
                timeout,
                pause_tolerance,
                speech_threshold,
                single_shot.unwrap_or(false),
                barge_in_rms_threshold.unwrap_or(BARGE_IN_RMS_THRESHOLD),
                barge_in_enabled.unwrap_or(true),
            );
            // Only clear shared state if our generation still owns the slot.
            // Otherwise, a newer start_conversation has already taken over.
            if let Ok(mut guard) = worker_tx_slot().lock() {
                let still_mine = guard.as_ref().map(|(g, _)| *g == my_gen).unwrap_or(false);
                if still_mine {
                    *guard = None;
                    ACTIVE.store(false, Ordering::Release);
                }
            }
        })
        .map_err(|e| {
            // Spawn failed: roll back the slot we just reserved.
            if let Ok(mut guard) = worker_tx_slot().lock() {
                if guard.as_ref().map(|(g, _)| *g == my_gen).unwrap_or(false) {
                    *guard = None;
                }
            }
            ACTIVE.store(false, Ordering::Release);
            format!("worker spawn: {}", e)
        })?;

    let _ = app.emit("conversation-state", "listening");
    eprintln!(
        "[conv] started — initial wait {}s, post-empty wait {}s, pause tolerance {}ms, mic sensitivity {:.4}",
        timeout, timeout, pause_tolerance, speech_threshold
    );
    Ok(())
}

fn worker_loop(
    app: AppHandle,
    rx: std::sync::mpsc::Receiver<Msg>,
    stream_handle: StreamingHandle,
    silence_timeout_s: u64,
    pause_tolerance_ms: u64,
    speech_rms_threshold: f32,
    single_shot: bool,
    barge_in_rms_threshold: f32,
    barge_in_enabled: bool,
) {
    let device_channels = stream_handle.channels.max(1);
    let mut mode = Mode::Listening;
    let mut utter_buf: Vec<i16> = Vec::with_capacity(48_000);
    let mut utter_sr: u32 = stream_handle.sample_rate;
    let mut silence_run_ms: u64 = 0;
    let mut accumulated_speech_ms: u64 = 0;
    let mut listening_idle_ms: u64 = 0;
    // Active idle deadline — uses silence_timeout_s from user settings.
    let mut current_idle_target_ms: u64 = silence_timeout_s * 1000;
    let mut speaking_since: Option<Instant> = None;
    // Rolling EMA of RMS during quiet Listening — adapts threshold to room noise.
    let mut noise_floor: f32 = 0.0;
    let mut consecutive_empty_stt: u32 = 0;
    // Set when Speaking begins (TtsStarted or non-empty SttResult). Used to
    // suppress VAD during BARGE_IN_WARMUP_MS so the TTS ramp-up doesn't
    // self-trigger.
    let mut tts_started_at: Option<Instant> = None;
    // Continuous above-BARGE_IN_RMS_THRESHOLD time in Speaking mode. Resets on
    // any quiet chunk; barge-in fires only when this clears BARGE_IN_MIN_SUSTAINED_MS.
    let mut barge_in_run_ms: u64 = 0;
    // Hysteresis: once barge-in run starts, RMS must drop below BARGE_IN_RELEASE_RMS
    // to reset the counter.  Prevents inter-syllable dips from clearing the tally.
    let mut barge_in_active: bool = false;
    // When true, the next completed utterance will be verified against enrolled
    // speakers before sending to STT.  Set on barge-in so a stranger's voice
    // doesn't hijack the conversation.
    let mut verify_next_utterance: bool = false;
    // Hysteresis state: once an above-threshold chunk flips this true, we stay
    // "in speech" through vowel valleys until RMS drops below effective_release.
    // Without this, single-threshold VAD misses inter-syllable dips and the
    // accumulated_speech_ms tally never reaches MIN_UTTERANCE_MS.
    let mut in_speech_burst: bool = false;
    // Listening-mode diagnostics: emit logs only on speech edges, not every tick.
    let mut listening_speech_active: bool = false;
    let mut listening_speech_run_ms: u64 = 0;
    let mut listening_speech_peak_rms: f32 = 0.0;

    let emit_state = |s: &str| {
        let _ = app.emit("conversation-state", s);
    };

    loop {
        match rx.recv_timeout(Duration::from_millis(TICK_MS)) {
            Ok(Msg::Stop) => {
                // Flush any buffered speech before stopping.
                if !utter_buf.is_empty() && accumulated_speech_ms >= MIN_UTTERANCE_MS {
                    let flushed = std::mem::take(&mut utter_buf);
                    eprintln!(
                        "[conv] Stop: flushing {} samples ({}ms speech)",
                        flushed.len(), accumulated_speech_ms
                    );
                    // Don't break yet — let the SttResult handler do it
                    // for single-shot, or transition to Speaking for continuous.
                    spawn_transcribe(flushed, utter_sr, false, false);
                    mode = Mode::Transcribing;
                    emit_state("transcribing");
                    // Continue the loop so SttResult can be processed.
                    // A second Stop will be a hard break.
                    continue;
                }
                let _ = app.emit("conversation-ended", ());
                break;
            },

            Ok(Msg::AudioChunk(samples, sr)) => {
                if samples.is_empty() {
                    continue;
                }
                // Downmix to mono if needed, in-place into a scratch buffer.
                let mono = downmix_to_mono(&samples, device_channels);
                let chunk_ms = (mono.len() as u64 * 1000) / sr.max(1) as u64;
                let rms = rms_i16(&mono);
                // Effective speech bar adapts to room noise: max of the static
                // floor and a margin above the rolling noise EMA, then capped so
                // a runaway floor can't make the mic effectively deaf.
                let effective_threshold = speech_rms_threshold
                    .max(noise_floor * NOISE_FLOOR_MARGIN)
                    .min(EFFECTIVE_THRESHOLD_CAP);
                let effective_release = SPEECH_RELEASE_RMS_THRESHOLD
                    .max(noise_floor * RELEASE_NOISE_FLOOR_MARGIN);
                if rms > effective_threshold {
                    in_speech_burst = true;
                } else if rms < effective_release {
                    in_speech_burst = false;
                }
                let is_speech = in_speech_burst;

                match mode {
                    Mode::Listening => {
                        if is_speech {
                            listening_speech_run_ms = listening_speech_run_ms.saturating_add(chunk_ms);
                            listening_speech_peak_rms = listening_speech_peak_rms.max(rms);
                            if !listening_speech_active {
                                listening_speech_active = true;
                                eprintln!(
                                    "[conv] speech detected: rms={:.4} start={:.4} rel={:.4} floor={:.4}",
                                    rms, effective_threshold, effective_release, noise_floor
                                );
                            }
                        } else if listening_speech_active {
                            eprintln!(
                                "[conv] speech ended: duration={}ms peak_rms={:.4}",
                                listening_speech_run_ms, listening_speech_peak_rms
                            );
                            listening_speech_active = false;
                            listening_speech_run_ms = 0;
                            listening_speech_peak_rms = 0.0;
                        }

                        if is_speech {
                            utter_buf.extend_from_slice(&mono);
                            utter_sr = sr;
                            accumulated_speech_ms += chunk_ms;
                            silence_run_ms = 0;
                            listening_idle_ms = 0;
                        } else {
                            // Quiet chunk outside an active utterance — feed the
                            // noise-floor EMA so we adapt to the room's baseline.
                            // Gate on rms well below threshold so quiet speech that
                            // sits just under the bar can't be absorbed as noise
                            // (which would raise effective_threshold and lock us out).
                            if accumulated_speech_ms == 0
                                && rms < speech_rms_threshold * 0.4
                            {
                                noise_floor = 0.95 * noise_floor + 0.05 * rms;
                            }
                            // Padding silence inside an active utterance still
                            // gets buffered (so trailing silence reaches Whisper).
                            if accumulated_speech_ms > 0 {
                                utter_buf.extend_from_slice(&mono);
                                silence_run_ms += chunk_ms;

                                let total_secs =
                                    utter_buf.len() as f32 / utter_sr.max(1) as f32;
                                if (silence_run_ms >= pause_tolerance_ms
                                    && accumulated_speech_ms >= MIN_UTTERANCE_MS)
                                    || total_secs >= MAX_UTTERANCE_S
                                {
                                    if listening_speech_active {
                                        eprintln!(
                                            "[conv] speech ended: duration={}ms peak_rms={:.4}",
                                            listening_speech_run_ms, listening_speech_peak_rms
                                        );
                                        listening_speech_active = false;
                                        listening_speech_run_ms = 0;
                                        listening_speech_peak_rms = 0.0;
                                    }
                                    let flushed = std::mem::take(&mut utter_buf);
                                    accumulated_speech_ms = 0;
                                    silence_run_ms = 0;
                                    mode = Mode::Transcribing;
                                    emit_state("transcribing");
                                    spawn_transcribe(flushed, utter_sr, verify_next_utterance, barge_in_enabled);
                                    verify_next_utterance = false;
                                }
                            } else {
                                listening_idle_ms += chunk_ms;
                                if listening_idle_ms >= current_idle_target_ms {
                                    let _ = app.emit("conversation-ended", ());
                                    eprintln!(
                                        "[conv] idle {}s, ending",
                                        current_idle_target_ms / 1000
                                    );
                                    break;
                                }
                            }
                        }
                    }
                    Mode::Transcribing => {
                        // Hold off accumulating during STT round-trip; samples
                        // captured here would belong to the model's response
                        // window, not the user turn.
                        listening_speech_active = false;
                        listening_speech_run_ms = 0;
                        listening_speech_peak_rms = 0.0;
                    }
                    Mode::Speaking => {
                        if !barge_in_enabled {
                            // Barge-in disabled by user — skip all VAD during TTS playback.
                            continue;
                        }
                        // Suppress barge-in detection entirely until TTS audio
                        // has actually started playing.  Before TtsStarted
                        // arrives the LLM is still thinking — there is nothing
                        // to barge into, and any high-RMS reading is ambient
                        // noise or a race with the IPC round-trip.
                        let tts_active = tts_started_at.is_some();
                        let in_warmup = tts_started_at
                            .map(|t| t.elapsed().as_millis() < BARGE_IN_WARMUP_MS as u128)
                            .unwrap_or(false);

                        if !tts_active || in_warmup {
                            barge_in_run_ms = 0;
                            barge_in_active = false;
                        } else if rms > barge_in_rms_threshold {
                            barge_in_active = true;
                            barge_in_run_ms = barge_in_run_ms.saturating_add(chunk_ms);
                        } else if barge_in_active && rms > BARGE_IN_RELEASE_RMS {
                            // Hysteresis: still in active burst, inter-syllable dip
                            // above release floor — don't reset the counter.
                            barge_in_run_ms = barge_in_run_ms.saturating_add(chunk_ms);
                        } else {
                            // Below release floor — burst truly ended, reset.
                            barge_in_active = false;
                            barge_in_run_ms = 0;
                        }

                        if barge_in_run_ms >= BARGE_IN_MIN_SUSTAINED_MS {
                            // Barge-in: cut TTS, switch to a fresh listening
                            // buffer pre-loaded with this chunk.
                            crate::commands::chat::stop_audio_queue();
                            let _ = app.emit("conversation-barge-in", ());
                            eprintln!("[conv] barge-in detected");

                            utter_buf.clear();
                            // Don't pre-load this chunk — it contains TTS echo.
                            // The user must continue speaking into the fresh buffer.
                            utter_sr = sr;
                            accumulated_speech_ms = 0;
                            silence_run_ms = 0;
                            listening_idle_ms = 0;
                            speaking_since = None;
                            tts_started_at = None;
                            barge_in_run_ms = 0;
                            barge_in_active = false;
                            in_speech_burst = false;
                            verify_next_utterance = true;
                            mode = Mode::Listening;
                            emit_state("listening");
                        }
                    }
                }
            }

            Ok(Msg::SttResult(result)) => {
                let had_text = match &result {
                    Ok(r) => !r.text.is_empty(),
                    Err(_) => false,
                };
                match result {
                    Ok(r) => {
                        eprintln!("[conv] stt-result: {:?} ({})", r.text, r.language);
                        let _ = app.emit(
                            "stt-result",
                            serde_json::json!({
                                "text": r.text,
                                "language": r.language,
                            }),
                        );
                    }
                    Err(e) => {
                        eprintln!("[conv] stt-error: {}", e);
                        let _ =
                            app.emit("stt-error", serde_json::json!({ "error": e }));
                    }
                }
                if had_text {
                    // Real utterance → expect a TTS response next.
                    consecutive_empty_stt = 0;
                    if single_shot {
                        // Single-shot mode: emit result and exit immediately.
                        // Frontend handles STT → LLM pipeline.
                        eprintln!("[conv] single-shot: stt done, exiting");
                        let _ = app.emit("conversation-ended", ());
                        break;
                    }
                    mode = Mode::Speaking;
                    speaking_since = Some(Instant::now());
                    // Do NOT set tts_started_at here — TTS won't play for
                    // several seconds while the LLM thinks.  Starting the
                    // warmup now means it expires long before audio actually
                    // reaches the speakers, leaving a window where mic echo
                    // can trigger a false barge-in.  The real warmup is set by
                    // Msg::TtsStarted, which fires only when audio begins.
                    tts_started_at = None;
                    barge_in_run_ms = 0;
                    in_speech_burst = false;
                    emit_state("speaking");
                } else {
                    // Empty result (VAD false-positive from birds/noise).
                    consecutive_empty_stt += 1;
                    eprintln!("[conv] empty STT result ({}/{})", consecutive_empty_stt, MAX_CONSECUTIVE_EMPTY_STT);
                    if consecutive_empty_stt >= MAX_CONSECUTIVE_EMPTY_STT {
                        eprintln!("[conv] {} consecutive empty results, ending conversation", consecutive_empty_stt);
                        let _ = app.emit("conversation-ended", ());
                        break;
                    }
                    // Go back to Listening with shorter idle deadline.
                    mode = Mode::Listening;
                    listening_idle_ms = 0;
                    current_idle_target_ms = silence_timeout_s * 1000;
                    silence_run_ms = 0;
                    accumulated_speech_ms = 0;
                    utter_buf.clear();
                    speaking_since = None;
                    tts_started_at = None;
                    barge_in_run_ms = 0;
                    in_speech_burst = false;
                    emit_state("listening");
                }
            }

            Ok(Msg::TtsStarted) => {
                if mode != Mode::Listening && mode != Mode::Transcribing {
                    mode = Mode::Speaking;
                    speaking_since = Some(Instant::now());
                    emit_state("speaking");
                }
                // Reset warmup window for the new chunk.  Do NOT reset
                // barge_in_run_ms — if the user has been speaking across a
                // chunk boundary we want to honour that accumulation.
                tts_started_at = Some(Instant::now());
            }

            Ok(Msg::TtsDone) => {
                mode = Mode::Listening;
                listening_idle_ms = 0;
                // Patient deadline: give the user time to think before the next turn.
                current_idle_target_ms = silence_timeout_s * 1000;
                silence_run_ms = 0;
                accumulated_speech_ms = 0;
                tts_started_at = None;
                barge_in_run_ms = 0;
                in_speech_burst = false;
                utter_buf.clear();
                speaking_since = None;
                emit_state("listening");
            }

            Err(RecvTimeoutError::Timeout) => {
                if mode == Mode::Speaking {
                    if let Some(since) = speaking_since {
                        if since.elapsed().as_secs() >= SPEAKING_SAFETY_TIMEOUT_S {
                            eprintln!(
                                "[conv] speaking safety timeout {}s, returning to listening",
                                SPEAKING_SAFETY_TIMEOUT_S
                            );
                            mode = Mode::Listening;
                            listening_idle_ms = 0;
                            current_idle_target_ms = silence_timeout_s * 1000;
                            speaking_since = None;
                            tts_started_at = None;
                            barge_in_run_ms = 0;
                            in_speech_burst = false;
                            emit_state("listening");
                        }
                    }
                }
            }

            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    stop_streaming_capture(stream_handle);
    eprintln!("[conv] worker exit");
}

fn spawn_transcribe(samples: Vec<i16>, sample_rate: u32, verify_speaker: bool, barge_in_enabled: bool) {
    std::thread::Builder::new()
        .name("conv-transcribe".into())
        .spawn(move || {
            let wav_result = write_wav(&samples, sample_rate, CONVERSATION_WAV_PATH);
            let wav_path = match wav_result {
                Ok(()) => CONVERSATION_WAV_PATH,
                Err(e) => {
                    let _ = send_msg(Msg::SttResult(Err(e)));
                    return;
                }
            };

            // If this utterance came right after a barge-in and the user has
            // enrolled speakers, verify the voice before transcribing.
            // Only runs when barge-in toggle is on.
            if verify_speaker && barge_in_enabled {
                let vp_dir = crate::voice::sherpa_wake::voiceprints_dir();
                let has_speakers = vp_dir.exists() && std::fs::read_dir(&vp_dir)
                    .map(|mut d| d.next().is_some())
                    .unwrap_or(false);
                if has_speakers {
                    eprintln!("[conv] barge-in: verifying speaker...");
                    match crate::voice::sherpa_wake::verify_speaker(wav_path, None) {
                        Ok(vr) => {
                            if vr.verified {
                                eprintln!(
                                    "[conv] barge-in speaker ✓ {} ({:.0}% confidence)",
                                    vr.speaker.as_deref().unwrap_or("?"),
                                    vr.confidence * 100.0
                                );
                            } else {
                                eprintln!(
                                    "[conv] barge-in speaker ✗ rejected (best match: {:.0}% — threshold: 70%)",
                                    vr.confidence * 100.0
                                );
                                let _ = send_msg(Msg::SttResult(Ok(SttResult {
                                    text: String::new(),
                                    language: String::new(),
                                })));
                                return;
                            }
                        }
                        Err(e) => {
                            eprintln!("[conv] barge-in speaker verify error: {} — allowing", e);
                        }
                    }
                } else {
                    eprintln!("[conv] barge-in: no enrolled speakers, skipping verification");
                }
            }

            let result = transcribe(wav_path);
            let _ = send_msg(Msg::SttResult(result));
        })
        .ok();
}

fn write_wav(samples: &[i16], sample_rate: u32, path: &str) -> Result<(), String> {
    let spec = WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: HoundSampleFormat::Int,
    };
    let mut writer =
        WavWriter::create(path, spec).map_err(|e| format!("wav create: {}", e))?;
    for &s in samples {
        writer
            .write_sample(s)
            .map_err(|e| format!("wav write: {}", e))?;
    }
    writer.finalize().map_err(|e| format!("wav finalize: {}", e))
}

fn downmix_to_mono(samples: &[i16], channels: u16) -> Vec<i16> {
    if channels <= 1 {
        return samples.to_vec();
    }
    let ch = channels as usize;
    let frames = samples.len() / ch;
    let mut out = Vec::with_capacity(frames);
    for i in 0..frames {
        let base = i * ch;
        let mut sum: i32 = 0;
        for c in 0..ch {
            sum += samples[base + c] as i32;
        }
        out.push((sum / ch as i32) as i16);
    }
    out
}

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
