use std::path::PathBuf;
use std::process::{Command, Stdio, Child};
use std::io::BufRead;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
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

/// Show the main window without declaring PA ready, so first-run setup and
/// voice failures are visible. The start sound and wake still wait for `app-ready`.
pub fn reveal_main_window(handle: &tauri::AppHandle, source: &str) {
    // A /ui client (the phone) drives PA: the desktop window stays hidden.
    if let Some(window) = handle.get_webview_window("main").filter(|_| !crate::api::remote::is_ui_remote_active()) {
        let _ = window.show();
        eprintln!("[stt] window shown ({source})");
    }
}

fn show_main_window(handle: Option<&tauri::AppHandle>, source: &str) {
    if let Some(h) = handle {
        reveal_main_window(h, source);
        let _ = h.emit("app-ready", ());
    }
}

/// `voice-service-status`: `starting` (slow first start), `ready`, or
/// `failed` with a message telling the user how to recover.
fn emit_service_status(handle: Option<&tauri::AppHandle>, state: &str, message: &str) {
    if let Some(h) = handle {
        // Same mapping as the venv-setup-* events: slow start shows as installing.
        let (setup_state, phase) = match state {
            "starting" => ("installing", "starting-service"),
            "failed" => ("error", "voice-service"),
            _ => ("ready", ""),
        };
        crate::voice::setup_status::publish(
            h,
            "voice-service-status",
            serde_json::json!({ "state": state, "message": message }),
            setup_state,
            phase,
            message,
        );
    }
}

fn report_service_failure(handle: Option<&tauri::AppHandle>, message: &str) {
    eprintln!("[stt] voice service failed: {message}");
    emit_service_status(handle, "failed", message);
    if let Some(h) = handle {
        reveal_main_window(h, "voice service failed");
    }
}

/// Whether the spawned server process has exited. A missing child means it
/// was shut down on purpose (PA quitting or a reload): stop quietly.
fn child_state() -> Option<Probe> {
    let mut guard = STT_CHILD.lock().unwrap_or_else(|p| p.into_inner());
    match guard.as_mut() {
        None => Some(Probe::Superseded),
        Some(child) => match child.try_wait() {
            Ok(Some(status)) => Some(Probe::Exited(status.to_string())),
            _ => None,
        },
    }
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

/// A Python child (stt-server, stt-helper): no console window, and UTF-8
/// stdio. Windows otherwise encodes a pipe in the ANSI code page, so the
/// Chinese PA reads back (transcripts, error lines) fails or comes back garbled.
fn python_command(python: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(python);
    crate::platform::no_window(&mut cmd);
    cmd.env("PYTHONUTF8", "1").env("PYTHONIOENCODING", "utf-8");
    cmd
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

/// Bumped by `cancel_fallback` to end any stt-helper still running.
static FALLBACK_EPOCH: AtomicU64 = AtomicU64::new(0);

/// End the transcriptions started so far (their conversation is over): one
/// still in HTTP or its retry wait never starts the fallback helper, a running
/// helper is killed. Transcriptions started afterwards are not affected.
pub fn cancel_fallback() {
    FALLBACK_EPOCH.fetch_add(1, Ordering::AcqRel);
}

/// Cancellation scope of one transcription, taken when it starts.
#[derive(Clone, Copy)]
pub struct CancelToken(u64);

impl CancelToken {
    pub fn new() -> Self {
        CancelToken(FALLBACK_EPOCH.load(Ordering::Acquire))
    }

    fn cancelled(self) -> bool {
        FALLBACK_EPOCH.load(Ordering::Acquire) != self.0
    }
}

/// Whisper model for both the resident server and the fallback helper.
fn stt_model() -> String {
    std::env::var("STT_MODEL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "base".to_string())
}

/// Script prompt from the primary TTS voice: zh-TW / zh-HK / zh-MO / ja-* →
/// traditional, everything else → simplified. Shared by server and helper.
fn lang_prompt() -> &'static str {
    let primary_voice = std::env::var("TTS_PRIMARY_VOICE").unwrap_or_default();
    if ["zh-TW", "zh-HK", "zh-MO", "ja-"].iter().any(|p| primary_voice.starts_with(p)) {
        "trad"
    } else {
        ""
    }
}

/// Time one fallback run may take: a cached model loads in seconds (the helper
/// never downloads), and CPU transcription stays within a few times real time.
fn fallback_limit(audio_secs: f32) -> std::time::Duration {
    std::time::Duration::from_secs(30) + std::time::Duration::from_secs_f32(audio_secs.clamp(0.0, 600.0) * 3.0)
}

/// Run `cmd` to completion, or kill and reap it once it outlives `limit` or
/// `cancelled()` turns true. Returns stdout, the last stderr line and the status.
fn run_bounded(
    mut cmd: Command,
    limit: std::time::Duration,
    cancelled: &dyn Fn() -> bool,
) -> Result<(String, String, std::process::ExitStatus), String> {
    use std::io::Read;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("stt-helper 启动失败: {}", e))?;
    let mut stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out_thread = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(out) = stdout.as_mut() {
            let _ = out.read_to_string(&mut text);
        }
        text
    });
    // Progress lines go to the PA log; the last one explains a failure.
    let err_thread = std::thread::spawn(move || {
        let mut last = String::new();
        for line in stderr.into_iter().flat_map(|e| std::io::BufReader::new(e).lines().map_while(Result::ok)) {
            if !line.is_empty() {
                eprintln!("{}", line);
                last = line;
            }
        }
        last
    });

    let deadline = std::time::Instant::now() + limit;
    let stopped = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if cancelled() => break Err(CANCELLED.to_string()),
            Ok(None) if std::time::Instant::now() >= deadline => {
                break Err(format!("备用转写超过 {} 秒未完成，已结束", limit.as_secs()))
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
            Err(e) => break Err(format!("stt-helper 等待失败: {}", e)),
        }
    };
    if stopped.is_err() {
        crate::platform::kill_tree(&mut child);
        let _ = child.wait();
    }
    let out = out_thread.join().unwrap_or_default();
    let last_err = err_thread.join().unwrap_or_default();
    stopped.map(|status| (out, last_err, status))
}

/// Fallback: spawn stt-helper subprocess (cold start, slower). Same model and
/// script prompt as the resident server, cached models only, bounded in time,
/// and ended once `cancelled()` turns true.
fn transcribe_subprocess(wav_path: &str, cancelled: &dyn Fn() -> bool) -> Result<SttResult, String> {
    let helper = helper_path();

    if !helper.exists() {
        return Err(format!("stt-helper 未找到: {}", helper.display()));
    }

    let mut cmd = match resolve_python() {
        Some(python) => {
            let mut c = python_command(python);
            c.arg(&helper);
            c
        }
        // Windows does not run scripts by their shebang line.
        None if cfg!(windows) => {
            return Err(format!(
                "语音环境未就绪({} 不存在),无法运行 stt-helper",
                crate::voice::venv::venv_python_path().display()
            ))
        }
        None => Command::new(&helper),
    };
    crate::platform::no_window(&mut cmd);
    let model = stt_model();
    cmd.arg(wav_path).arg("--model").arg(&model).arg("--lang-prompt").arg(lang_prompt());
    eprintln!("[stt] fallback stt-helper with cached model '{}'", model);

    let audio_secs = hound::WavReader::open(wav_path)
        .map(|r| r.duration() as f32 / r.spec().sample_rate.max(1) as f32)
        .unwrap_or(60.0);
    let (stdout, last_err, status) = run_bounded(cmd, fallback_limit(audio_secs), cancelled)?;

    if status.success() {
        let stdout = stdout.trim().to_string();
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
        Err(format!("stt-helper 错误: {}", last_err))
    }
}

const CANCELLED: &str = "备用转写已取消";

/// Main entry: try HTTP first, retry once on transient failure, then fallback to subprocess
pub fn transcribe(wav_path: &str) -> Result<SttResult, String> {
    transcribe_until(wav_path, CancelToken::new())
}

/// `transcribe`, ended by any `cancel_fallback` after `token` was taken.
pub fn transcribe_until(wav_path: &str, token: CancelToken) -> Result<SttResult, String> {
    transcribe_staged(
        &|| token.cancelled(),
        std::time::Duration::from_secs(1),
        &|| transcribe_http(wav_path),
        &|cancelled| transcribe_subprocess(wav_path, cancelled),
    )
}

/// HTTP, one retry after `retry_delay`, then the helper. `cancelled` is checked
/// before every stage and during the wait, and passed on to the helper.
fn transcribe_staged(
    cancelled: &dyn Fn() -> bool,
    retry_delay: std::time::Duration,
    http: &dyn Fn() -> Result<SttResult, String>,
    fallback: &dyn Fn(&dyn Fn() -> bool) -> Result<SttResult, String>,
) -> Result<SttResult, String> {
    if cancelled() {
        return Err(CANCELLED.to_string());
    }
    // Step 1: Try resident HTTP server (fast, model already loaded)
    match http() {
        Ok(result) => {
            eprintln!("[stt] HTTP transcription succeeded");
            return Ok(result);
        }
        Err(e) => {
            eprintln!("[stt] HTTP failed ({}), retrying in {:?}...", e, retry_delay);
        }
    }

    // Step 2: Retry once after short delay (server might be temporarily busy)
    let resume = std::time::Instant::now() + retry_delay;
    while std::time::Instant::now() < resume {
        if cancelled() {
            return Err(CANCELLED.to_string());
        }
        std::thread::sleep(std::time::Duration::from_millis(20).min(retry_delay));
    }
    if cancelled() {
        return Err(CANCELLED.to_string());
    }
    match http() {
        Ok(result) => {
            eprintln!("[stt] HTTP transcription succeeded (retry)");
            return Ok(result);
        }
        Err(e) => {
            eprintln!("[stt] HTTP retry also failed ({}), falling back to subprocess...", e);
        }
    }

    // Step 3: Fallback to subprocess (cold start, no VAD — may hallucinate)
    if cancelled() {
        eprintln!("[stt] transcription cancelled; not starting stt-helper");
        return Err(CANCELLED.to_string());
    }
    fallback(cancelled)
}

/// Ensure the resident STT server is running; spawn it if not.
/// Called once at app startup.
pub fn ensure_stt_server(app_handle: Option<tauri::AppHandle>) {
    let port = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
    {
        Ok(c) => probe_voice_port(&c, STT_SERVER_URL),
        Err(_) => VoicePort::Free,
    };

    match port {
        VoicePort::Ours(pid) => {
            let spawned_here = STT_CHILD.lock().map(|g| g.is_some()).unwrap_or(false);
            match pid.filter(|_| !spawned_here) {
                // Our own child, or a server too old to report its pid: reuse it.
                None => {
                    eprintln!("[stt] resident server already running on :8651");
                    mark_app_ready(|| show_main_window(app_handle.as_ref(), "existing server"));
                    return;
                }
                Some(pid) => match stop_stale_server(pid) {
                    Ok(true) => {}
                    Ok(false) => {
                        mark_app_ready(|| show_main_window(app_handle.as_ref(), "existing server"));
                        return;
                    }
                    Err(e) => {
                        report_service_failure(app_handle.as_ref(), &e);
                        return;
                    }
                },
            }
        }
        // Not PA's voice service: report it, never kill a process PA does not own.
        VoicePort::Foreign(detail) => {
            report_service_failure(
                app_handle.as_ref(),
                &format!(
                    "语音服务端口 8651 已被其他程序占用（{detail}）。在终端运行 {find} \
                     查看占用的进程，关闭它后重启 Pocket Agent。",
                    find = FIND_LISTENER_HINT,
                ),
            );
            return;
        }
        VoicePort::Free => {}
    }

    let server_script = server_path();
    if !server_script.exists() {
        report_service_failure(
            app_handle.as_ref(),
            &format!("找不到语音服务脚本 {}。请重新安装 Pocket Agent。", server_script.display()),
        );
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

    // STT_MODEL (default: base) lets users switch between tiny/base/small.
    let stt_model = stt_model();
    eprintln!("[stt] using Whisper model: {}", stt_model);
    let lang_prompt = lang_prompt();

    match python_command(&python)
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

            // Poll /health in a background thread. Past 30 s the window is
            // shown with a "starting" status and polling continues until the
            // server is healthy, replaced by a reload, or has exited.
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
                    // 30 s covers cold-boot of faster_whisper + Whisper "base"
                    // model load on a typical laptop. Tune up only if the user
                    // is on a much slower disk / first-ever model download.
                    let probe_deadline_s: u64 = 30;
                    let deadline = std::time::Instant::now()
                        + std::time::Duration::from_secs(probe_deadline_s);
                    let interval = std::time::Duration::from_millis(500);
                    let healthy = || voice_service_healthy(&client);
                    let mut outcome = probe_health(generation, Some(deadline), interval, healthy, child_state);
                    if outcome == Probe::TimedOut {
                        eprintln!(
                            "[stt] resident server not up on :8651 after {}s (first start may be downloading the Whisper model); still waiting.\n\
                             [stt]   venv python: {}",
                            probe_deadline_s, python_for_hint,
                        );
                        emit_service_status(probe_handle.as_ref(), "starting", "");
                        if let Some(h) = probe_handle.as_ref() {
                            reveal_main_window(h, "voice service slow to start");
                        }
                        outcome = probe_health(generation, None, interval, healthy, child_state);
                    }
                    match outcome {
                        Probe::Healthy => {
                            eprintln!("[stt] resident server healthy on :8651");
                            emit_service_status(probe_handle.as_ref(), "ready", "");
                            mark_app_ready(|| show_main_window(probe_handle.as_ref(), "new server"));
                        }
                        Probe::Exited(status) => report_service_failure(
                            probe_handle.as_ref(),
                            &format!(
                                "语音服务进程已退出（{status}）。首次启动需要联网下载语音模型（HuggingFace），\
                                 请检查网络后重启 Pocket Agent；在终端里运行 PA 可以看到 [stt-server] 的详细日志。"
                            ),
                        ),
                        Probe::Superseded | Probe::TimedOut => {}
                    }
                })
                .ok();
        }
        Err(e) => report_service_failure(
            app_handle.as_ref(),
            &format!("语音服务启动失败（{python}: {e}）。删除 ~/.pocket-agent/.venv-ready 后重启 Pocket Agent 会重建语音环境。"),
        ),
    }
}

/// What answers on the voice-service port.
#[derive(Debug, PartialEq)]
enum VoicePort {
    /// Nothing listening: PA starts its own server.
    Free,
    /// PA's stt-server, with the pid its `/health` reports (absent before 0.3.1).
    Ours(Option<u32>),
    /// Something else holds the port; carries what it answered.
    Foreign(String),
}

/// stt-server's `/health` is `{"status": "ok", "model": …, "vad": …}`. Any
/// other answer (404, 500, unrelated JSON) is not PA's voice service.
fn is_voice_health(status: u16, body: &str) -> bool {
    if !(200..300).contains(&status) {
        return false;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return false;
    };
    v["status"] == "ok" && v["model"].is_string() && v["vad"].is_string()
}

fn health_pid(body: &str) -> Option<u32> {
    let v = serde_json::from_str::<serde_json::Value>(body).ok()?;
    v["pid"].as_u64().and_then(|p| u32::try_from(p).ok())
}

/// How to find the process listening on 8651, for error messages.
#[cfg(not(windows))]
const FIND_LISTENER_HINT: &str = "lsof -iTCP:8651 -sTCP:LISTEN";
#[cfg(windows)]
const FIND_LISTENER_HINT: &str = "netstat -ano | findstr :8651";

/// Pids listening on `port`; `None` when lsof cannot be run.
#[cfg(not(windows))]
fn listening_pids(port: u16) -> Option<Vec<u32>> {
    let out = Command::new("lsof")
        .args(["-nP", &format!("-tiTCP:{port}"), "-sTCP:LISTEN"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.trim().parse().ok()).collect())
}

/// Pids listening on `port`; `None` when netstat cannot be run.
#[cfg(windows)]
fn listening_pids(port: u16) -> Option<Vec<u32>> {
    let out = crate::platform::no_window(&mut Command::new("netstat"))
        .args(["-ano", "-p", "TCP"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let mut pids = netstat_listeners(&String::from_utf8_lossy(&out.stdout), port);
    // `-p TCP` lists IPv4 only; IPv6 listeners come from `-p TCPv6`.
    if let Ok(v6) = crate::platform::no_window(&mut Command::new("netstat"))
        .args(["-ano", "-p", "TCPv6"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    {
        pids.extend(netstat_listeners(&String::from_utf8_lossy(&v6.stdout), port));
    }
    pids.sort_unstable();
    pids.dedup();
    Some(pids)
}

/// Listening pids for `port` in `netstat -ano` output. The state column is
/// localized ("LISTENING", "侦听", ...), so a listener is recognised by its
/// foreign address instead, which is always `0.0.0.0:0` or `[::]:0`.
#[cfg_attr(not(windows), allow(dead_code))]
fn netstat_listeners(text: &str, port: u16) -> Vec<u32> {
    let suffix = format!(":{port}");
    text.lines()
        .filter_map(|line| {
            let cols: Vec<&str> = line.split_whitespace().collect();
            match cols.as_slice() {
                [proto, local, foreign, _state, pid]
                    if proto.starts_with("TCP")
                        && local.ends_with(&suffix)
                        && (*foreign == "0.0.0.0:0" || *foreign == "[::]:0") =>
                {
                    pid.parse().ok()
                }
                _ => None,
            }
        })
        .collect()
}

/// Ask `pid` to exit (SIGTERM; on Windows `taskkill /F`, since a console
/// python ignores a polite close).
fn terminate(pid: u32) {
    #[cfg(not(windows))]
    let _ = Command::new("/bin/kill").args(["-TERM", &pid.to_string()]).stderr(Stdio::null()).status();
    #[cfg(windows)]
    let _ = crate::platform::no_window(&mut Command::new("taskkill"))
        .args(["/PID", &pid.to_string(), "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// A server left by an earlier PA runs old code with old settings, and its
/// stderr may be a pipe nobody reads any more (every log line then fails).
/// Stop it so PA can start a fresh one, but only when the reported pid is the
/// process listening on 8651. Ok(true): port is free; Ok(false): keep reusing.
fn stop_stale_server(pid: u32) -> Result<bool, String> {
    let Some(listeners) = listening_pids(8651) else {
        eprintln!("[stt] cannot list listeners on :8651; reusing the server there (pid={pid})");
        return Ok(false);
    };
    if !listeners.contains(&pid) {
        return Err(format!(
            "语音服务端口 8651 上的程序报告 pid {pid}，但监听该端口的是 {listeners:?}。在终端运行 \
             {find} 查看占用的进程，关闭它后重启 Pocket Agent。",
            find = FIND_LISTENER_HINT,
        ));
    }
    eprintln!("[stt] replacing stale stt-server (pid={pid})");
    terminate(pid);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if listening_pids(8651).is_some_and(|l| l.is_empty()) {
            return Ok(true);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    let kill = if cfg!(windows) { format!("taskkill /PID {pid} /F") } else { format!("kill {pid}") };
    Err(format!(
        "上次运行留下的语音服务（pid {pid}）5 秒内没有退出。在终端运行 {kill} 后重启 Pocket Agent。"
    ))
}

/// Whether nothing listens at `base` (`http://host:port`). Windows answers a
/// connect to a closed local port only after ~2 s of retries, so a short HTTP
/// timeout cannot tell "free" from "busy"; a plain connect with room to wait can.
fn connection_refused(base: &str) -> bool {
    let Some(addr) = base
        .strip_prefix("http://")
        .and_then(|a| a.parse::<std::net::SocketAddr>().ok())
    else {
        return false;
    };
    matches!(
        std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(5)),
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused
    )
}

fn probe_voice_port(client: &reqwest::blocking::Client, base: &str) -> VoicePort {
    match client.get(format!("{base}/health")).send() {
        Err(e) if e.is_connect() => VoicePort::Free,
        Err(e) if e.is_timeout() && connection_refused(base) => VoicePort::Free,
        Err(e) => VoicePort::Foreign(format!("/health 无响应：{e}")),
        Ok(resp) => {
            let status = resp.status().as_u16();
            let body = resp.text().unwrap_or_default();
            if is_voice_health(status, &body) {
                VoicePort::Ours(health_pid(&body))
            } else {
                VoicePort::Foreign(format!("/health 返回 HTTP {status}，不是 Pocket Agent 语音服务"))
            }
        }
    }
}

fn voice_service_healthy(client: &reqwest::blocking::Client) -> bool {
    matches!(probe_voice_port(client, STT_SERVER_URL), VoicePort::Ours(_))
}

#[derive(Debug, PartialEq)]
enum Probe {
    Healthy,
    Superseded,
    TimedOut,
    /// The server process exited; carries its exit status.
    Exited(String),
}

/// Poll `healthy` until the deadline (`None`: no deadline), giving up as soon
/// as a newer server process (a reload) owns the port or `child_state`
/// reports the process gone.
fn probe_health(
    generation: u64,
    deadline: Option<std::time::Instant>,
    interval: std::time::Duration,
    healthy: impl Fn() -> bool,
    child_state: impl Fn() -> Option<Probe>,
) -> Probe {
    while deadline.map_or(true, |d| std::time::Instant::now() < d) {
        if SERVER_GEN.load(Ordering::Acquire) != generation {
            return Probe::Superseded;
        }
        if healthy() {
            return Probe::Healthy;
        }
        if let Some(end) = child_state() {
            return end;
        }
        std::thread::sleep(interval);
    }
    if SERVER_GEN.load(Ordering::Acquire) != generation {
        Probe::Superseded
    } else {
        Probe::TimedOut
    }
}

/// Kill the resident STT server. Called when PA exits.
pub fn shutdown_stt_server() {
    if let Ok(mut guard) = STT_CHILD.lock() {
        if let Some(ref mut child) = *guard {
            eprintln!("[stt] shutting down STT server (pid={})...", child.id());
            crate::platform::kill_tree(child);
            let _ = child.wait(); // reap zombie
        }
        *guard = None;
    }
}

/// Serializes tests that touch process-wide server state.
#[cfg(test)]
pub(crate) fn test_serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL.lock().unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    #[test]
    fn probe_keeps_waiting_past_the_first_deadline_until_healthy() {
        let _serial = test_serial();
        let generation = SERVER_GEN.fetch_add(1, Ordering::AcqRel) + 1;
        let soon = Instant::now() + Duration::from_millis(50);
        assert_eq!(probe_health(generation, Some(soon), Duration::from_millis(10), || false, || None), Probe::TimedOut);
        let healthy_at = Instant::now() + Duration::from_millis(80);
        assert_eq!(
            probe_health(generation, None, Duration::from_millis(10), || Instant::now() >= healthy_at, || None),
            Probe::Healthy
        );
    }

    #[test]
    fn probe_stops_when_the_server_exits_or_is_replaced() {
        let _serial = test_serial();
        let generation = SERVER_GEN.fetch_add(1, Ordering::AcqRel) + 1;
        assert_eq!(
            probe_health(generation, None, Duration::from_millis(10), || false, || Some(Probe::Exited("exit status: 1".into()))),
            Probe::Exited("exit status: 1".into())
        );
        let (tx, rx) = mpsc::channel();
        let probe = std::thread::spawn(move || {
            probe_health(generation, None, Duration::from_millis(10), || { let _ = tx.send(()); false }, || None)
        });
        rx.recv().unwrap();
        SERVER_GEN.fetch_add(1, Ordering::AcqRel);
        assert_eq!(probe.join().unwrap(), Probe::Superseded);
    }

    #[test]
    fn reload_before_first_health_still_shows_the_window_once() {
        let _serial = test_serial();
        APP_READY.store(false, Ordering::Release);
        // First server spawned; a second one replaces it before it is healthy.
        let first = SERVER_GEN.fetch_add(1, Ordering::AcqRel) + 1;
        let (gen_tx, gen_rx) = mpsc::channel();
        let old_probe = std::thread::spawn(move || {
            let outcome = probe_health(first, Some(Instant::now() + Duration::from_secs(2)), Duration::from_millis(10), || {
                let _ = gen_tx.send(());
                false
            }, || None);
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
        assert_eq!(probe_health(second, Some(Instant::now() + Duration::from_secs(1)), Duration::from_millis(10), || true, || None), Probe::Healthy);
        mark_app_ready(|| shown += 1);
        mark_app_ready(|| shown += 1);
        assert_eq!(shown, 1, "app-ready fires for the first healthy server only");
        assert!(is_app_ready());
    }

    #[test]
    fn only_pa_health_counts_as_the_voice_service() {
        let pa = r#"{"status": "ok", "model": "base", "vad": "silero"}"#;
        assert!(is_voice_health(200, pa));
        assert!(!is_voice_health(404, pa));
        assert!(!is_voice_health(500, pa));
        assert!(!is_voice_health(200, ""));
        assert!(!is_voice_health(200, "{}"));
        assert!(!is_voice_health(200, r#"{"status": "ok"}"#));
        assert!(!is_voice_health(200, r#"{"status": "healthy", "model": "x", "vad": "y"}"#));
        assert!(!is_voice_health(200, "<html>not found</html>"));
    }

    /// One-shot HTTP server answering every request with `status` + `body`.
    fn serve_once(status: &'static str, body: &'static str) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut conn, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = conn.read(&mut buf);
                let _ = write!(
                    conn,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn port_probe_tells_free_ours_and_foreign_apart() {
        let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(2)).build().unwrap();
        // Nothing listening.
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = free.local_addr().unwrap();
        drop(free);
        assert_eq!(probe_voice_port(&client, &format!("http://{addr}")), VoicePort::Free);

        let ours = serve_once("200 OK", r#"{"status":"ok","model":"base","vad":"silero"}"#);
        assert_eq!(probe_voice_port(&client, &ours), VoicePort::Ours(None));
        let ours = serve_once("200 OK", r#"{"status":"ok","model":"base","vad":"silero","pid":4242}"#);
        assert_eq!(probe_voice_port(&client, &ours), VoicePort::Ours(Some(4242)));

        for (status, body) in [("404 Not Found", ""), ("500 Internal Server Error", ""), ("200 OK", "{}"), ("200 OK", r#"{"name":"other"}"#)] {
            let base = serve_once(status, body);
            assert!(
                matches!(probe_voice_port(&client, &base), VoicePort::Foreign(_)),
                "{status} {body} must not count as PA's voice service"
            );
        }
    }

    #[test]
    fn health_pid_is_optional() {
        assert_eq!(health_pid(r#"{"status":"ok","model":"base","vad":"silero","pid":4242}"#), Some(4242));
        assert_eq!(health_pid(r#"{"status":"ok","model":"base","vad":"silero"}"#), None);
        assert_eq!(health_pid(r#"{"pid":"4242"}"#), None);
        assert_eq!(health_pid(r#"{"pid":-1}"#), None);
    }

    #[test]
    fn listening_pids_finds_the_listener() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert_eq!(listening_pids(port), Some(vec![std::process::id()]));
        drop(listener);
        assert_eq!(listening_pids(port), Some(vec![]));
    }

    #[test]
    fn netstat_listeners_ignore_the_localized_state_column() {
        let text = "
Active Connections

  Proto  Local Address          Foreign Address        State           PID
  TCP    0.0.0.0:135            0.0.0.0:0              LISTENING       1100
  TCP    127.0.0.1:8651         0.0.0.0:0              侦听            4242
  TCP    127.0.0.1:8651         127.0.0.1:50000        ESTABLISHED     4242
  TCP    127.0.0.1:50000        127.0.0.1:8651         ESTABLISHED     9999
  TCP    127.0.0.1:18651        0.0.0.0:0              LISTENING       7777
  TCP    [::]:8651              [::]:0                 ABHÖREN         5151
";
        assert_eq!(netstat_listeners(text, 8651), vec![4242, 5151]);
        assert_eq!(netstat_listeners(text, 135), vec![1100]);
        assert!(netstat_listeners(text, 50000).is_empty());
    }

    #[cfg(unix)]
    fn sh(script: &str) -> Command {
        let mut c = Command::new("/bin/sh");
        c.arg("-c").arg(script);
        c
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pa-stt-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(unix)]
    fn process_alive(pid: &str) -> bool {
        Command::new("/bin/kill").args(["-0", pid.trim()]).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
    }

    #[cfg(unix)]
    #[test]
    fn stuck_helper_is_killed_at_the_limit() {
        let dir = scratch("stuck");
        let pid_file = dir.join("pid");
        let started = std::time::Instant::now();
        let err = run_bounded(sh(&format!("echo $$ > {}; exec sleep 30", pid_file.display())), Duration::from_millis(500), &|| false).unwrap_err();
        assert!(err.contains("超过"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(!process_alive(&std::fs::read_to_string(&pid_file).unwrap()), "helper reaped");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn cancelled_helper_is_killed_promptly() {
        let dir = scratch("cancel");
        let pid_file = dir.join("pid");
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            flag.store(true, Ordering::Release);
        });
        let started = std::time::Instant::now();
        let err = run_bounded(sh(&format!("echo $$ > {}; exec sleep 30", pid_file.display())), Duration::from_secs(60), &|| cancel.load(Ordering::Acquire)).unwrap_err();
        assert!(err.contains("已取消"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(!process_alive(&std::fs::read_to_string(&pid_file).unwrap()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn finished_helper_returns_its_output_and_last_error_line() {
        let (out, last, status) = run_bounded(sh("echo '{\"text\":\"hi\"}'; echo progress >&2; echo 'final reason' >&2; exit 3"), Duration::from_secs(10), &|| false).unwrap();
        assert_eq!(out.trim(), r#"{"text":"hi"}"#);
        assert_eq!(last, "final reason");
        assert_eq!(status.code(), Some(3));
    }

    #[test]
    fn longer_audio_gets_more_time() {
        assert_eq!(fallback_limit(0.0), Duration::from_secs(30));
        assert_eq!(fallback_limit(10.0), Duration::from_secs(60));
    }

    /// One transcription through the real stages, token and `run_bounded`:
    /// HTTP is `http`, stt-helper a script that records its pid and start, then
    /// answers "helper" (or hangs while `dir/hang` exists).
    #[cfg(unix)]
    fn staged(token: CancelToken, dir: &std::path::Path, http: &dyn Fn() -> Result<SttResult, String>) -> Result<String, String> {
        let script = format!(
            "echo $$ > {d}/pid; touch {d}/started; [ -e {d}/hang ] && exec sleep 30; echo helper",
            d = dir.display()
        );
        transcribe_staged(&|| token.cancelled(), Duration::from_millis(300), http, &|cancelled| {
            let (out, _, _) = run_bounded(sh(&script), Duration::from_secs(20), cancelled)?;
            Ok(SttResult { text: out.trim().to_string(), language: String::new() })
        })
        .map(|r| r.text)
    }

    #[cfg(unix)]
    fn http_down() -> Result<SttResult, String> {
        Err("connection refused".into())
    }

    /// After a cancelled request: the next one runs normally, through the helper.
    #[cfg(unix)]
    fn next_round_works(dir: &std::path::Path) {
        let _ = std::fs::remove_file(dir.join("hang"));
        let _ = std::fs::remove_file(dir.join("started"));
        assert_eq!(staged(CancelToken::new(), dir, &http_down).unwrap(), "helper");
    }

    #[cfg(unix)]
    #[test]
    fn cancel_during_http_never_starts_the_helper() {
        let _serial = test_serial();
        let dir = scratch("cancel-http");
        let token = CancelToken::new();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let entered_tx = Mutex::new(entered_tx);
        let release_rx = Mutex::new(release_rx);
        let calls = AtomicU64::new(0);
        std::thread::scope(|s| {
            let old = s.spawn(|| {
                staged(token, &dir, &|| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    entered_tx.lock().unwrap().send(()).unwrap();
                    // Bounded, so a request that keeps going fails the test instead of hanging it.
                    let _ = release_rx.lock().unwrap().recv_timeout(Duration::from_secs(2));
                    http_down()
                })
            });
            entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            cancel_fallback();
            // Taken after the cancel, before the old request ends: unaffected.
            let next = CancelToken::new();
            release_tx.send(()).unwrap();
            assert_eq!(old.join().unwrap().unwrap_err(), CANCELLED);
            assert!(!next.cancelled(), "the old request ending does not cancel the next one");
        });
        assert_eq!(calls.load(Ordering::SeqCst), 1, "no retry after the cancel");
        assert!(!dir.join("started").exists(), "no helper after the cancel");
        next_round_works(&dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn cancel_during_retry_wait_never_starts_the_helper() {
        let _serial = test_serial();
        let dir = scratch("cancel-wait");
        let token = CancelToken::new();
        let (failed_tx, failed_rx) = mpsc::channel();
        let failed_tx = Mutex::new(failed_tx);
        let calls = AtomicU64::new(0);
        let started = Instant::now();
        std::thread::scope(|s| {
            let old = s.spawn(|| {
                staged(token, &dir, &|| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    failed_tx.lock().unwrap().send(()).unwrap();
                    http_down()
                })
            });
            failed_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            cancel_fallback();
            assert_eq!(old.join().unwrap().unwrap_err(), CANCELLED);
        });
        assert!(started.elapsed() < Duration::from_millis(300), "the wait ends at the cancel");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "no retry after the cancel");
        assert!(!dir.join("started").exists());
        next_round_works(&dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn cancel_just_before_the_helper_never_starts_it() {
        let _serial = test_serial();
        let dir = scratch("cancel-before-helper");
        let calls = AtomicU64::new(0);
        let err = staged(CancelToken::new(), &dir, &|| {
            // The cancel lands as the retry fails, right before the fallback.
            if calls.fetch_add(1, Ordering::SeqCst) == 1 {
                cancel_fallback();
            }
            http_down()
        })
        .unwrap_err();
        assert_eq!(err, CANCELLED);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(!dir.join("started").exists());
        next_round_works(&dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn cancel_while_the_helper_runs_kills_and_reaps_it() {
        let _serial = test_serial();
        let dir = scratch("cancel-helper");
        std::fs::write(dir.join("hang"), "").unwrap();
        let token = CancelToken::new();
        std::thread::scope(|s| {
            let old = s.spawn(|| staged(token, &dir, &http_down));
            let deadline = Instant::now() + Duration::from_secs(5);
            while !dir.join("started").exists() {
                assert!(Instant::now() < deadline, "helper never started");
                std::thread::sleep(Duration::from_millis(20));
            }
            let cancelled_at = Instant::now();
            cancel_fallback();
            assert_eq!(old.join().unwrap().unwrap_err(), CANCELLED);
            assert!(cancelled_at.elapsed() < Duration::from_secs(3));
        });
        assert!(!process_alive(&std::fs::read_to_string(dir.join("pid")).unwrap()), "helper reaped");
        next_round_works(&dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The real stt-helper against a stand-in faster_whisper that records how
    /// the model is loaded: same model and prompt as the server, cached only.
    fn run_helper(dir: &std::path::Path, cached: bool, extra: &[&str]) -> (std::process::Output, String) {
        let fake = dir.join("fake");
        std::fs::create_dir_all(fake.join("faster_whisper")).unwrap();
        let log = dir.join("calls.json");
        std::fs::write(fake.join("faster_whisper/__init__.py"), format!(r#"
import json
class _Info: language = "zh"
class _Seg: text = "测试"
class WhisperModel:
    def __init__(self, name, **kw):
        self.call = {{"model": name, **kw}}
        if not {cached}: raise RuntimeError("not found in local cache")
    def transcribe(self, path, **kw):
        self.call.update(kw)
        json.dump(self.call, open({log:?}, "w"), ensure_ascii=False)
        return [_Seg()], _Info()
"#, cached = if cached { "True" } else { "False" }, log = log.to_str().unwrap())).unwrap();
        let out = python_command(if cfg!(windows) { "python" } else { "/usr/bin/python3" })
            .arg(helper_path())
            .arg(std::env::temp_dir().join("x.wav"))
            .args(extra)
            .env("PYTHONPATH", &fake)
            .output()
            .unwrap();
        (out, std::fs::read_to_string(&log).unwrap_or_default())
    }

    #[test]
    fn helper_uses_the_configured_model_and_prompt_from_cache_only() {
        let dir = scratch("helper");
        let (out, calls) = run_helper(&dir, true, &["--model", "small", "--lang-prompt", "trad"]);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let call: serde_json::Value = serde_json::from_str(&calls).unwrap();
        assert_eq!(call["model"], "small");
        assert_eq!(call["local_files_only"], true);
        assert_eq!(call["initial_prompt"], "以下是繁體中文的句子。");
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), r#"{"text": "测试", "language": "zh"}"#);

        let (_, calls) = run_helper(&dir, true, &["--model", "base", "--lang-prompt", ""]);
        let call: serde_json::Value = serde_json::from_str(&calls).unwrap();
        assert_eq!((call["model"].as_str(), call["initial_prompt"].as_str()), (Some("base"), Some("以下是普通话的句子。")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn helper_reports_an_uncached_model_instead_of_downloading() {
        let dir = scratch("uncached");
        let (out, _) = run_helper(&dir, false, &["--model", "small"]);
        assert_eq!(out.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&out.stderr).contains("不在本机缓存中"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
