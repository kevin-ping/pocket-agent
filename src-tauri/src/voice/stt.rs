use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::io::BufRead;

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

pub struct SttResult {
    pub text: String,
    pub language: String,
}

pub fn transcribe(wav_path: &str) -> Result<SttResult, String> {
    let helper = helper_path();

    if !helper.exists() {
        return Err(format!(
            "stt-helper 未找到: {}",
            helper.display()
        ));
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

        // Try parsing JSON: {"text": "...", "language": "zh"}
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&stdout) {
            let text = v["text"].as_str().unwrap_or("").to_string();
            let language = v["language"].as_str().unwrap_or("zh").to_string();
            if text.is_empty() {
                return Err("识别结果为空".to_string());
            }
            Ok(SttResult { text, language })
        } else {
            // Fallback: plain text (old format)
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
