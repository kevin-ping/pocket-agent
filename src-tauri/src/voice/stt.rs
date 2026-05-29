use std::path::PathBuf;
use std::process::{Command, Stdio, Child};
use std::io::BufRead;
use std::sync::Mutex;

/// Store the STT server child process so we can kill it when PA exits.
static STT_CHILD: Mutex<Option<Child>> = Mutex::new(None);

const STT_SERVER_URL: &str = "http://127.0.0.1:8651";

fn helper_path() -> PathBuf {
    // app bundle: .app/Contents/MacOS/pocket-agent → .app/Contents/Resources/stt-helper
    if let Ok(exe) = std::env::current_exe() {
        if let Some(macos_dir) = exe.parent() {
            if let Some(contents_dir) = macos_dir.parent() {
                let bundled = contents_dir.join("Resources").join("stt-helper");
                if bundled.exists() {
                    return bundled;
                }
            }
        }
    }

    // dev fallback
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let dev_path = PathBuf::from(manifest_dir).join("resources").join("stt-helper");
        if dev_path.exists() {
            return dev_path;
        }
    }

    PathBuf::from("src-tauri/resources/stt-helper")
}

fn server_path() -> PathBuf {
    // Same lookup as helper_path, but for stt-server.py
    if let Ok(exe) = std::env::current_exe() {
        if let Some(macos_dir) = exe.parent() {
            if let Some(contents_dir) = macos_dir.parent() {
                let bundled = contents_dir.join("Resources").join("stt-server.py");
                if bundled.exists() {
                    return bundled;
                }
            }
        }
    }

    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let dev_path = PathBuf::from(manifest_dir).join("resources").join("stt-server.py");
        if dev_path.exists() {
            return dev_path;
        }
    }

    PathBuf::from("src-tauri/resources/stt-server.py")
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

    if text.is_empty() {
        return Err("识别结果为空".to_string());
    }

    Ok(SttResult { text, language })
}

/// Fallback: spawn stt-helper subprocess (cold start, slower)
fn transcribe_subprocess(wav_path: &str) -> Result<SttResult, String> {
    let helper = helper_path();

    if !helper.exists() {
        return Err(format!("stt-helper 未找到: {}", helper.display()));
    }

    let mut child = if let Ok(python) = std::env::var("STT_PYTHON") {
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
            return Err("识别结果为空".to_string());
        }

        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&stdout) {
            let text = v["text"].as_str().unwrap_or("").to_string();
            let language = v["language"].as_str().unwrap_or("zh").to_string();
            if text.is_empty() {
                return Err("识别结果为空".to_string());
            }
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

/// Main entry: try HTTP first, fallback to subprocess
pub fn transcribe(wav_path: &str) -> Result<SttResult, String> {
    // Step 1: Try resident HTTP server (fast, model already loaded)
    match transcribe_http(wav_path) {
        Ok(result) => {
            eprintln!("[stt] HTTP transcription succeeded");
            return Ok(result);
        }
        Err(e) => {
            eprintln!("[stt] HTTP failed ({}), falling back to subprocess...", e);
        }
    }

    // Step 2: Fallback to subprocess (cold start)
    transcribe_subprocess(wav_path)
}

/// Ensure the resident STT server is running; spawn it if not.
/// Called once at app startup.
pub fn ensure_stt_server() {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build();

    let running = match client {
        Ok(c) => c.get(format!("{}/health", STT_SERVER_URL)).send().is_ok(),
        Err(_) => false,
    };

    if running {
        eprintln!("[stt] resident server already running on :8651");
        return;
    }

    let server_script = server_path();
    if !server_script.exists() {
        eprintln!("[stt] stt-server.py not found at {}, skipping auto-start", server_script.display());
        return;
    }

    eprintln!("[stt] starting resident STT server...");

    // Resolve python: STT_PYTHON env > venv python > system python3
    let python = if let Ok(p) = std::env::var("STT_PYTHON") {
        p
    } else if let Ok(home) = std::env::var("HOME") {
        let venv_python = std::path::Path::new(&home)
            .join(".hermes/hermes-agent/venv/bin/python3");
        if venv_python.exists() {
            venv_python.to_string_lossy().to_string()
        } else {
            "python3".to_string()
        }
    } else {
        "python3".to_string()
    };

    match Command::new(&python)
        .arg(&server_script)
        .arg("--port")
        .arg("8651")
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
