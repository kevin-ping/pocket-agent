use std::path::PathBuf;
use std::process::{Command, Stdio, Child};
use std::io::BufRead;
use std::sync::Mutex;
use tauri::{Emitter, Manager};

/// Store the STT server child process so we can kill it when PA exits.
static STT_CHILD: Mutex<Option<Child>> = Mutex::new(None);
static APP_READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn is_app_ready() -> bool {
    APP_READY.load(std::sync::atomic::Ordering::Acquire)
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
        APP_READY.store(true, std::sync::atomic::Ordering::Release);
        if let Some(h) = app_handle.as_ref() {
            if let Some(window) = h.get_webview_window("main") {
                let _ = window.show();
                eprintln!("[stt] window shown (existing server)");
            }
            let _ = h.emit("app-ready", ());
        }
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

    match Command::new(&python)
        .arg(&server_script)
        .arg("--port")
        .arg("8651")
        .arg("--model")
        .arg(&stt_model)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())  // Show model loading progress in PA logs
        .spawn()
    {
        Ok(child) => {
            let pid = child.id();
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
                    while std::time::Instant::now() < deadline {
                        if client.get(&url).send().map(|r| r.status().is_success()).unwrap_or(false) {
                            eprintln!("[stt] resident server healthy on :8651");
            APP_READY.store(true, std::sync::atomic::Ordering::Release);
                            if let Some(h) = probe_handle.as_ref() {
                                if let Some(window) = h.get_webview_window("main") {
                                    let _ = window.show();
                                    eprintln!("[stt] window shown (new server)");
                                }
                                let _ = h.emit("app-ready", ());
                            }
                            return;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(500));
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
