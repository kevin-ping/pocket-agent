use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::json;
use tauri::{AppHandle, Emitter};

const VENV_DIR_RELATIVE: &str = ".pocket-agent/venv";
const VENV_MARKER_RELATIVE: &str = ".pocket-agent/.venv-ready";

fn home_dir() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_default()
}

pub fn venv_root() -> PathBuf {
    home_dir().join(VENV_DIR_RELATIVE)
}

pub fn venv_python_path() -> PathBuf {
    venv_root().join("bin").join("python3")
}

fn venv_pip_path() -> PathBuf {
    venv_root().join("bin").join("pip")
}

fn marker_path() -> PathBuf {
    home_dir().join(VENV_MARKER_RELATIVE)
}

pub fn venv_ready() -> bool {
    venv_python_path().exists() && marker_path().exists()
}

/// Look up a bundled resource (e.g. `requirements-stt.txt`) using the same
/// .app/Contents/Resources → CARGO_MANIFEST_DIR/resources fallback chain that
/// stt.rs uses for stt-server.py.
pub fn resource_path(filename: &str) -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(macos_dir) = exe.parent() {
            if let Some(contents_dir) = macos_dir.parent() {
                let bundled = contents_dir.join("Resources").join(filename);
                if bundled.exists() {
                    return bundled;
                }
            }
        }
    }

    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let dev_path = PathBuf::from(manifest_dir).join("resources").join(filename);
        if dev_path.exists() {
            return dev_path;
        }
    }

    PathBuf::from(format!("src-tauri/resources/{}", filename))
}

/// Locate a usable system python3 to bootstrap the venv. Returns the path that
/// can be passed to `Command::new`.
pub fn find_system_python3() -> Result<String, String> {
    const CANDIDATES: &[&str] = &[
        "python3",
        "/opt/homebrew/bin/python3",
        "/usr/local/bin/python3",
        "/usr/bin/python3",
    ];

    for candidate in CANDIDATES {
        let ok = Command::new(candidate)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            return Ok(candidate.to_string());
        }
    }

    Err("找不到 python3。请安装 Xcode Command Line Tools 或 Homebrew Python 后重试。".to_string())
}

fn emit_progress(app: &AppHandle, phase: &str, detail: Option<&str>) {
    let _ = app.emit(
        "venv-setup-progress",
        json!({ "phase": phase, "detail": detail.unwrap_or("") }),
    );
}

fn emit_error(app: &AppHandle, phase: &str, message: &str) {
    eprintln!("[venv] {} failed: {}", phase, message);
    let _ = app.emit(
        "venv-setup-error",
        json!({ "phase": phase, "message": message }),
    );
}

fn run_streaming(
    app: &AppHandle,
    phase: &str,
    mut cmd: Command,
) -> Result<(), String> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn failed: {}", e))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let app_out = app.clone();
    let phase_out = phase.to_string();
    let stdout_thread = stdout.map(|s| {
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(s).lines().map_while(Result::ok) {
                if line.is_empty() {
                    continue;
                }
                eprintln!("[venv:{}] {}", phase_out, line);
                emit_progress(&app_out, &phase_out, Some(&line));
            }
        })
    });

    let app_err = app.clone();
    let phase_err = phase.to_string();
    let stderr_thread = stderr.map(|s| {
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(s).lines().map_while(Result::ok) {
                if line.is_empty() {
                    continue;
                }
                eprintln!("[venv:{}!] {}", phase_err, line);
                emit_progress(&app_err, &phase_err, Some(&line));
            }
        })
    });

    let status = child
        .wait()
        .map_err(|e| format!("wait failed: {}", e))?;

    if let Some(t) = stdout_thread {
        let _ = t.join();
    }
    if let Some(t) = stderr_thread {
        let _ = t.join();
    }

    if status.success() {
        Ok(())
    } else {
        Err(format!("exit code {:?}", status.code()))
    }
}

/// Bootstrap the PA-owned venv at ~/.pocket-agent/venv/. Blocking — call from
/// a dedicated thread or async runtime so the UI thread is not stalled. Emits
/// `venv-setup-started`, `venv-setup-progress { phase, detail }`,
/// `venv-setup-done`, `venv-setup-error { phase, message }` on the AppHandle.
///
/// On success the marker file ~/.pocket-agent/.venv-ready is written. The marker
/// is intentionally separate from venv_python_path so a half-installed venv
/// (interrupted pip install) won't be mistaken for ready on next launch.
pub fn ensure_venv(app: &AppHandle) -> Result<(), String> {
    if venv_ready() {
        let _ = app.emit("venv-setup-ready", ());
        return Ok(());
    }

    let _ = app.emit("venv-setup-started", ());

    let requirements = resource_path("requirements-stt.txt");
    if !requirements.exists() {
        let msg = format!("requirements-stt.txt 缺失: {}", requirements.display());
        emit_error(app, "checking-resources", &msg);
        return Err(msg);
    }

    let system_python = match find_system_python3() {
        Ok(p) => p,
        Err(e) => {
            emit_error(app, "finding-python", &e);
            return Err(e);
        }
    };

    if !venv_python_path().exists() {
        emit_progress(app, "creating-venv", None);
        let venv_root = venv_root();
        if let Some(parent) = venv_root.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                let msg = format!("无法创建 {}: {}", parent.display(), e);
                emit_error(app, "creating-venv", &msg);
                return Err(msg);
            }
        }
        let mut cmd = Command::new(&system_python);
        cmd.arg("-m").arg("venv").arg(&venv_root);
        if let Err(e) = run_streaming(app, "creating-venv", cmd) {
            emit_error(app, "creating-venv", &e);
            return Err(e);
        }
    }

    emit_progress(app, "upgrading-pip", None);
    {
        let mut cmd = Command::new(venv_python_path());
        cmd.arg("-m")
            .arg("pip")
            .arg("install")
            .arg("--upgrade")
            .arg("pip");
        if let Err(e) = run_streaming(app, "upgrading-pip", cmd) {
            // Non-fatal: pip can still install with the bundled version
            eprintln!("[venv] pip upgrade non-fatal failure: {}", e);
        }
    }

    emit_progress(app, "installing-deps", None);
    {
        let mut cmd = Command::new(venv_pip_path());
        cmd.arg("install")
            .arg("--disable-pip-version-check")
            .arg("-r")
            .arg(&requirements);
        if let Err(e) = run_streaming(app, "installing-deps", cmd) {
            emit_error(app, "installing-deps", &e);
            return Err(e);
        }
    }

    if let Err(e) = std::fs::write(marker_path(), "ok\n") {
        let msg = format!("无法写 marker {}: {}", marker_path().display(), e);
        emit_error(app, "writing-marker", &msg);
        return Err(msg);
    }

    let _ = app.emit("venv-setup-done", ());
    eprintln!("[venv] PA venv ready at {}", venv_root().display());
    Ok(())
}
