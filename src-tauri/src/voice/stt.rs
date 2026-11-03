use std::path::PathBuf;
use std::process::{Command, Stdio, Child};
use std::io::BufRead;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use tauri::{Emitter, Manager};

/// Store the STT server child process so we can kill it when PA exits.
static STT_CHILD: Mutex<Option<Child>> = Mutex::new(None);
static APP_READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Bumped on every spawn; a health probe for an older process stops quietly.
static SERVER_GEN: AtomicU64 = AtomicU64::new(0);

pub fn is_app_ready() -> bool {
    APP_READY.load(std::sync::atomic::Ordering::Acquire)
}

/// First healthy server shows the main window and emits `app-ready`; later
/// ones (a reload) do not replay the start sequence.
fn mark_app_ready(notify: impl FnOnce()) {
    if !APP_READY.swap(true, Ordering::AcqRel) {
        notify();
    }
}

fn show_main_window(handle: Option<&tauri::AppHandle>, source: &str) {
    if let Some(h) = handle {
        if let Some(window) = h.get_webview_window("main") {
            let _ = window.show();
            eprintln!("[stt] window shown ({source})");
        }
        let _ = h.emit("app-ready", ());
    }
}

/// Requests in flight against the resident server, and whether a reload is
/// swapping the server (and its token) out from under them.
struct VoiceGate {
    active: u32,
    reloading: bool,
}

static VOICE_GATE: Mutex<VoiceGate> = Mutex::new(VoiceGate { active: 0, reloading: false });
static VOICE_GATE_CV: Condvar = Condvar::new();

fn voice_gate() -> std::sync::MutexGuard<'static, VoiceGate> {
    VOICE_GATE.lock().unwrap_or_else(|p| p.into_inner())
}

/// Lease on the resident server for one request. A reload waits until no
/// lease is held; a new lease waits until the reloaded server is healthy.
pub struct VoiceTxn(());

pub fn voice_txn() -> VoiceTxn {
    let mut gate = VOICE_GATE_CV
        .wait_while(voice_gate(), |g| g.reloading)
        .unwrap_or_else(|p| p.into_inner());
    gate.active += 1;
    VoiceTxn(())
}

/// Like `voice_txn`, but `None` instead of waiting while a reload is running.
pub fn try_voice_txn() -> Option<VoiceTxn> {
    let mut gate = voice_gate();
    if gate.reloading {
        return None;
    }
    gate.active += 1;
    Some(VoiceTxn(()))
}

impl Drop for VoiceTxn {
    fn drop(&mut self) {
        voice_gate().active -= 1;
        VOICE_GATE_CV.notify_all();
    }
}

/// Held for the whole reload; dropping it lets waiting requests through.
struct ReloadLease(());

impl Drop for ReloadLease {
    fn drop(&mut self) {
        voice_gate().reloading = false;
        VOICE_GATE_CV.notify_all();
    }
}

/// Claim the server for a reload only if no request is in flight and `idle`
/// still holds, checked under the same lock new requests take.
fn try_begin_reload(idle: impl Fn() -> bool) -> Option<ReloadLease> {
    let mut gate = voice_gate();
    if gate.active > 0 || gate.reloading || !idle() {
        return None;
    }
    gate.reloading = true;
    Some(ReloadLease(()))
}

const STT_SERVER_URL: &str = "http://127.0.0.1:8651";

fn helper_path() -> PathBuf {
    crate::voice::venv::resource_path("stt-helper")
}

fn server_path() -> PathBuf {
    crate::voice::venv::resource_path("stt-server.py")
}

/// STT_PYTHON env (dev override) > PA's own venv at ~/.pocket-agent/venv/.
/// Returns None when neither is available — caller decides whether to skip or
/// fall back to spawning stt-helper directly.
fn resolve_python() -> Option<String> {
    if let Ok(p) = std::env::var("STT_PYTHON") {
        return Some(p);
    }
    let venv_python = crate::voice::venv::venv_python_path();
    if venv_python.exists() {
        return Some(venv_python.to_string_lossy().to_string());
    }
    None
}

pub struct SttResult {
    pub text: String,
    pub language: String,
}

/// Try HTTP transcription via resident stt-server.py
fn transcribe_http(wav_path: &str) -> Result<SttResult, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let file_bytes = std::fs::read(wav_path)
        .map_err(|e| format!("读取 WAV 失败: {}", e))?;

    let file_name = PathBuf::from(wav_path)
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
        .post(format!("{}/transcribe", STT_SERVER_URL))
        .multipart(form)
        .send()
        .map_err(|e| format!("HTTP 请求失败: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(format!("stt-server 返回 {}: {}", status, body));
    }

    let v: serde_json::Value = resp.json()
        .map_err(|e| format!("解析 JSON 失败: {}", e))?;

    let text = v["text"].as_str().unwrap_or("").to_string();
    let language = v["language"].as_str().unwrap_or("zh").to_string();

    // Empty text is a legitimate outcome (VAD short-circuit on silent audio).
    // Return Ok with empty text — callers decide policy (single-shot shows a toast,
    // continuous mode stays in Listening).
    Ok(SttResult { text, language })
}

/// Fallback: spawn stt-helper subprocess (cold start, slower)
fn transcribe_subprocess(wav_path: &str) -> Result<SttResult, String> {
    let helper = helper_path();

    if !helper.exists() {
        return Err(format!("stt-helper 未找到: {}", helper.display()));
    }

    let mut child = if let Some(python) = resolve_python() {
        Command::new(python)
            .arg(&helper)
            .arg(wav_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("stt-helper 启动失败: {}", e))?
    } else {
        Command::new(&helper)
            .arg(wav_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("stt-helper 启动失败: {}", e))?
    };

    // Stream stderr lines in real-time for progress logging
    if let Some(stderr) = child.stderr.take() {
        let reader = std::io::BufReader::new(stderr);
        for line in reader.lines() {
            match line {
                Ok(l) if !l.is_empty() => eprintln!("{}", l),
                _ => break,
            }
        }
    }

    let output = child.wait_with_output()
        .map_err(|e| format!("stt-helper 等待失败: {}", e))?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stdout.is_empty() {
            // Empty subprocess output — treat as empty result (consistent with HTTP path).
            return Ok(SttResult { text: String::new(), language: String::new() });
        }

        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&stdout) {
            let text = v["text"].as_str().unwrap_or("").to_string();
            let language = v["language"].as_str().unwrap_or("zh").to_string();
            Ok(SttResult { text, language })
        } else {
            Ok(SttResult {
                text: stdout,
                language: "zh".to_string(),
            })
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!("stt-helper 错误: {}", stderr))
    }
}

/// Main entry: try HTTP first, retry once on transient failure, then fallback to subprocess
pub fn transcribe(wav_path: &str) -> Result<SttResult, String> {
    let _txn = voice_txn();
    // Step 1: Try resident HTTP server (fast, model already loaded)
    match transcribe_http(wav_path) {
        Ok(result) => {
            eprintln!("[stt] HTTP transcription succeeded");
            return Ok(result);
        }
        Err(e) => {
            eprintln!("[stt] HTTP failed ({}), retrying in 1s...", e);
        }
    }

    // Step 2: Retry once after short delay (server might be temporarily busy)
    std::thread::sleep(std::time::Duration::from_secs(1));
    match transcribe_http(wav_path) {
        Ok(result) => {
            eprintln!("[stt] HTTP transcription succeeded (retry)");
            return Ok(result);
        }
        Err(e) => {
            eprintln!("[stt] HTTP retry also failed ({}), falling back to subprocess...", e);
        }
    }

    // Step 3: Fallback to subprocess (cold start, no VAD — may hallucinate)
    transcribe_subprocess(wav_path)
}

/// Ensure the resident STT server is running; spawn it if not.
/// Called once at app startup.
pub fn ensure_stt_server(app_handle: Option<tauri::AppHandle>) {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build();

    let running = match client {
        Ok(c) => c.get(format!("{}/health", STT_SERVER_URL)).send().is_ok(),
        Err(_) => false,
    };

    if running {
        eprintln!("[stt] resident server already running on :8651");
        mark_app_ready(|| show_main_window(app_handle.as_ref(), "existing server"));
        return;
    }

    let server_script = server_path();
    if !server_script.exists() {
        eprintln!("[stt] stt-server.py not found at {}, skipping auto-start", server_script.display());
        return;
    }

    // PA-owned venv is the only supported runtime. If it's not ready yet, bail
    // out — the venv bootstrap thread in lib.rs will retry ensure_stt_server()
    // after the install finishes.
    let python = match resolve_python() {
        Some(p) => p,
        None => {
            eprintln!(
                "[stt] PA venv not ready at {}; STT server will start after venv setup completes",
                crate::voice::venv::venv_python_path().display()
            );
            return;
        }
    };

    eprintln!("[stt] starting resident STT server...");

    // Read STT_MODEL from env (default: base). Lets users switch between tiny/base/small.
    let stt_model = std::env::var("STT_MODEL").unwrap_or_else(|_| "base".to_string());
    eprintln!("[stt] using Whisper model: {}", stt_model);

    // Derive lang-prompt from primary TTS voice to control Whisper output script.
    // zh-TW, zh-HK, zh-MO, ja-* → traditional; everything else → simplified.
    let primary_voice = std::env::var("TTS_PRIMARY_VOICE").unwrap_or_default();
    let lang_prompt = if primary_voice.starts_with("zh-TW")
        || primary_voice.starts_with("zh-HK")
        || primary_voice.starts_with("zh-MO")
        || primary_voice.starts_with("ja-")
    {
        "trad"
    } else {
        ""
    };

    match Command::new(&python)
        .arg(&server_script)
        .arg("--port")
        .arg("8651")
        .arg("--model")
        .arg(&stt_model)
        .arg("--lang-prompt")
        .arg(&lang_prompt)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())  // Show model loading progress in PA logs
        .spawn()
    {
        Ok(child) => {
            let pid = child.id();
            let generation = SERVER_GEN.fetch_add(1, Ordering::AcqRel) + 1;
            if let Ok(mut guard) = STT_CHILD.lock() {
                *guard = Some(child);
            }
            eprintln!("[stt] STT server spawned (pid={}, python={})", pid, python);

            // Poll /health for up to 6s in a background thread. If the server
            // never comes up, print one high-visibility hint with the venv path
            // and the exact pip install command — instead of silently slipping
            // into the ~13s subprocess fallback on every utterance.
            let python_for_hint = python.clone();
            let probe_handle = app_handle.clone();
            std::thread::Builder::new()
                .name("stt-health-probe".to_string())
                .spawn(move || {
                    let client = match reqwest::blocking::Client::builder()
                        .timeout(std::time::Duration::from_millis(500))
                        .build()
                    {
                        Ok(c) => c,
                        Err(_) => return,
                    };
                    let url = format!("{}/health", STT_SERVER_URL);
                    // 30 s covers cold-boot of faster_whisper + Whisper "base"
                    // model load on a typical laptop. Tune up only if the user
                    // is on a much slower disk / first-ever model download.
                    let probe_deadline_s: u64 = 30;
                    let deadline = std::time::Instant::now()
                        + std::time::Duration::from_secs(probe_deadline_s);
                    let outcome = probe_health(
                        generation,
                        deadline,
                        std::time::Duration::from_millis(500),
                        || client.get(&url).send().map(|r| r.status().is_success()).unwrap_or(false),
                    );
                    match outcome {
                        Probe::Healthy => {
                            eprintln!("[stt] resident server healthy on :8651");
                            mark_app_ready(|| show_main_window(probe_handle.as_ref(), "new server"));
                            return;
                        }
                        Probe::Superseded => return,
                        Probe::TimedOut => {}
                    }
                    eprintln!(
                        "[stt] WARNING: resident server did not respond on :8651 within {}s.\n\
                         [stt]   STT will fall back to slow per-utterance subprocess.\n\
                         [stt]   venv python: {}\n\
                         [stt]   If this persists, delete ~/.pocket-agent/.venv-ready and restart PA\n\
                         [stt]   to trigger a fresh venv bootstrap.",
                        probe_deadline_s, python_for_hint,
                    );
                })
                .ok();
        }
        Err(e) => eprintln!("[stt] failed to spawn STT server: {}", e),
    }
}

#[derive(Debug, PartialEq)]
enum Probe {
    Healthy,
    Superseded,
    TimedOut,
}

/// Poll `healthy` until the deadline, giving up as soon as a newer server
/// process (a reload) owns the port.
fn probe_health(
    generation: u64,
    deadline: std::time::Instant,
    interval: std::time::Duration,
    healthy: impl Fn() -> bool,
) -> Probe {
    while std::time::Instant::now() < deadline {
        if SERVER_GEN.load(Ordering::Acquire) != generation {
            return Probe::Superseded;
        }
        if healthy() {
            return Probe::Healthy;
        }
        std::thread::sleep(interval);
    }
    if SERVER_GEN.load(Ordering::Acquire) != generation {
        Probe::Superseded
    } else {
        Probe::TimedOut
    }
}

/// No microphone owner, conversation or wake listener — nothing is waiting on
/// the resident server. In-flight requests are tracked by `VoiceTxn`.
fn voice_idle() -> bool {
    crate::voice::record::current_owner() == crate::voice::record::OWNER_NONE
        && !crate::voice::conversation::is_active()
        && !crate::voice::sherpa_wake::is_wake_active()
}

fn server_idle() -> bool {
    voice_idle() && voice_gate().active == 0
}

/// Restart the resident server once voice has been idle for a while, so it
/// re-imports native modules replaced on disk. Requests started meanwhile wait
/// for the new server; `voice-engine-reloaded` follows once it is healthy. A
/// server PA did not spawn is left alone: starting a second one would
/// overwrite its token (see stt-server token file).
pub fn restart_when_idle(app: tauri::AppHandle) {
    const QUIET_SECS: u32 = 10;
    if !STT_CHILD.lock().map(|g| g.is_some()).unwrap_or(false) {
        eprintln!("[stt] server not owned by this PA; restart PA to load the new voice engine");
        return;
    }
    let mut quiet = 0;
    let _lease = loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
        quiet = if server_idle() { quiet + 1 } else { 0 };
        if quiet >= QUIET_SECS {
            match try_begin_reload(voice_idle) {
                Some(lease) => break lease,
                None => quiet = 0,
            }
        }
    };
    eprintln!("[stt] voice idle; restarting STT server to load the new voice engine");
    shutdown_stt_server();
    // Keep the handle: if the first server never got healthy, this one shows
    // the main window instead of the superseded probe.
    ensure_stt_server(Some(app.clone()));
    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_millis(500))
        .build()
    {
        Ok(c) => c,
        Err(_) => return,
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if client.get(format!("{}/health", STT_SERVER_URL)).send().map(|r| r.status().is_success()).unwrap_or(false) {
            eprintln!("[stt] STT server reloaded");
            let _ = app.emit("voice-engine-reloaded", ());
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    eprintln!("[stt] WARNING: STT server did not come back within 30s after reload");
}

/// Kill the resident STT server. Called when PA exits.
pub fn shutdown_stt_server() {
    if let Ok(mut guard) = STT_CHILD.lock() {
        if let Some(ref mut child) = *guard {
            eprintln!("[stt] shutting down STT server (pid={})...", child.id());
            let _ = child.kill();
            let _ = child.wait(); // reap zombie
        }
        *guard = None;
    }
}

/// Serializes tests that touch the process-wide voice gate.
#[cfg(test)]
pub(crate) fn test_serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL.lock().unwrap_or_else(|p| p.into_inner())
}

/// Puts the gate in the reloading state until the returned lease drops.
#[cfg(test)]
pub(crate) fn hold_reload_for_test() -> impl Drop {
    try_begin_reload(|| true).expect("gate idle")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    #[test]
    fn reload_waits_for_requests_in_flight() {
        let _serial = test_serial();
        let txn = voice_txn();
        assert!(try_begin_reload(|| true).is_none(), "a /transcribe in flight blocks the reload");
        drop(txn);
        assert!(try_begin_reload(|| true).is_some());
    }

    #[test]
    fn reload_rechecks_idle_under_the_gate() {
        let _serial = test_serial();
        // A conversation started after the last quiet sample.
        assert!(try_begin_reload(|| false).is_none());
        assert!(!voice_gate().reloading);
    }

    #[test]
    fn requests_started_during_reload_wait_for_the_new_server() {
        let _serial = test_serial();
        let lease = try_begin_reload(|| true).expect("idle");
        assert!(try_voice_txn().is_none(), "VAD does not wait out a reload");
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _txn = voice_txn();
            tx.send(()).unwrap();
        });
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err(), "no request reaches the old server");
        drop(lease);
        rx.recv_timeout(Duration::from_secs(2)).expect("released once the server is back");
        worker.join().unwrap();
        assert_eq!(voice_gate().active, 0);
    }

    #[test]
    fn reload_before_first_health_still_shows_the_window_once() {
        let _serial = test_serial();
        APP_READY.store(false, Ordering::Release);
        // First server spawned; compat install finishes before it is healthy.
        let first = SERVER_GEN.fetch_add(1, Ordering::AcqRel) + 1;
        let (gen_tx, gen_rx) = mpsc::channel();
        let old_probe = std::thread::spawn(move || {
            let outcome = probe_health(first, Instant::now() + Duration::from_secs(2), Duration::from_millis(10), || {
                let _ = gen_tx.send(());
                false
            });
            if outcome == Probe::Healthy {
                mark_app_ready(|| panic!("old probe must not report ready"));
            }
            outcome
        });
        gen_rx.recv().unwrap();
        // Reload spawns a second server that comes up after the old probe gives up.
        let second = SERVER_GEN.fetch_add(1, Ordering::AcqRel) + 1;
        assert_eq!(old_probe.join().unwrap(), Probe::Superseded);

        let mut shown = 0;
        assert_eq!(probe_health(second, Instant::now() + Duration::from_secs(1), Duration::from_millis(10), || true), Probe::Healthy);
        mark_app_ready(|| shown += 1);
        mark_app_ready(|| shown += 1);
        assert_eq!(shown, 1, "app-ready fires for the first healthy server only");
        assert!(is_app_ready());
    }
}
