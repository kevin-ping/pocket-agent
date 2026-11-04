use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::json;
use tauri::AppHandle;

use super::setup_status;

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

/// Read the bundled requirements-stt.txt. Used both for the marker content and
/// to drive `pip install -r`. Returning the full text (not a hash) keeps the
/// marker self-describing — a maintainer can `cat` it to see what was installed.
fn marker_expected() -> Result<String, String> {
    let req_path = resource_path("requirements-stt.txt");
    std::fs::read_to_string(&req_path).map_err(|e| {
        format!("无法读 requirements ({}): {}", req_path.display(), e)
    })
}

pub fn venv_ready() -> bool {
    if !venv_python_path().exists() {
        return false;
    }
    let Ok(actual) = std::fs::read_to_string(marker_path()) else {
        return false;
    };
    let Ok(expected) = marker_expected() else {
        return false;
    };
    // Tolerate trailing whitespace differences (e.g. some editors strip the final newline).
    actual.trim_end() == expected.trim_end()
}

/// Whether a pip failure may fall back to an earlier full install. The marker
/// lives outside the venv, so it only counts while the venv's interpreter still
/// exists; a missing interpreter means the venv is about to be rebuilt and the
/// marker is dropped so a half-built venv is never taken for the old one.
fn previous_install(python: &Path, marker: &Path) -> bool {
    if !python.exists() {
        let _ = std::fs::remove_file(marker);
        return false;
    }
    marker.exists()
}

/// Modules stt-server cannot start without. edge-tts is left out on purpose:
/// an older edge-tts still serves STT, wake and voiceprints.
const CORE_IMPORTS: &str =
    "import numpy, faster_whisper, onnxruntime, fastapi, uvicorn, scipy, kaldi_native_fbank";

fn core_deps_importable(python: &Path) -> bool {
    Command::new(python)
        .arg("-c")
        .arg(CORE_IMPORTS)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Look up a bundled resource (e.g. `requirements-stt.txt`) using the same
/// .app/Contents/Resources → CARGO_MANIFEST_DIR/resources fallback chain that
/// stt.rs uses for stt-server.py.
pub fn resource_path(filename: &str) -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(bundled) = resource_path_from(&exe, filename) {
            return bundled;
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

/// Bundled copy next to `exe` inside a .app. tauri.conf.json maps
/// `resources/<file>` to `Contents/Resources/resources/<file>`; the flat
/// `Contents/Resources/<file>` is kept for older bundles.
fn resource_path_from(exe: &Path, filename: &str) -> Option<PathBuf> {
    let resources = exe.parent()?.parent()?.join("Resources");
    [resources.join("resources").join(filename), resources.join(filename)]
        .into_iter()
        .find(|p| p.exists())
}

/// Interpreters tried for the venv, in order. The CLT python3 (3.9) comes
/// first: it is what Finder-launched PA finds, and the sherpa compat build
/// for macOS < 13.4 only ships cp39 wheels.
const PYTHON_CANDIDATES: &[&str] = &[
    "/usr/bin/python3",
    "python3",
    "/opt/homebrew/bin/python3.12",
    "/opt/homebrew/bin/python3.11",
    "/opt/homebrew/bin/python3.10",
    "/usr/local/bin/python3.12",
    "/usr/local/bin/python3.11",
    "/usr/local/bin/python3.10",
    "/opt/homebrew/bin/python3",
    "/usr/local/bin/python3",
];

/// requirements-stt.txt pins numpy<2, whose last release (1.26) stops at 3.12.
const PYTHON_MIN: (u32, u32) = (3, 9);
const PYTHON_MAX: (u32, u32) = (3, 12);

/// `Ok(version)` when `candidate` runs and can `import venv`; otherwise a
/// short reason for the error message.
fn probe_python(candidate: &str) -> Result<(u32, u32), String> {
    let out = Command::new(candidate)
        .arg("-c")
        .arg("import sys, venv; print('%d.%d' % sys.version_info[:2])")
        .stdin(Stdio::null())
        .output()
        .map_err(|_| "不存在".to_string())?;
    if !out.status.success() {
        return Err("无法运行或缺少 venv 模块".to_string());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let (major, minor) = text.trim().split_once('.').ok_or("无法读取版本")?;
    match (major.parse(), minor.parse()) {
        (Ok(major), Ok(minor)) => Ok((major, minor)),
        _ => Err("无法读取版本".to_string()),
    }
}

fn pick_python<'a>(candidates: &[&'a str]) -> Result<&'a str, String> {
    let mut report = Vec::new();
    for &candidate in candidates {
        match probe_python(candidate) {
            Ok(v) if (PYTHON_MIN..=PYTHON_MAX).contains(&v) => return Ok(candidate),
            Ok((major, minor)) => report.push(format!("{candidate}: {major}.{minor}(不支持)")),
            Err(reason) => report.push(format!("{candidate}: {reason}")),
        }
    }
    Err(format!(
        "找不到可用的 Python {}.{}–{}.{}。已检查:{}。请运行 xcode-select --install 安装 Xcode Command Line Tools(自带 Python 3.9),然后重启 Pocket Agent。",
        PYTHON_MIN.0, PYTHON_MIN.1, PYTHON_MAX.0, PYTHON_MAX.1,
        report.join("；"),
    ))
}

/// Locate a system python3 (3.9–3.12, with venv) to bootstrap the venv.
/// Returns the path that can be passed to `Command::new`.
pub fn find_system_python3() -> Result<String, String> {
    pick_python(PYTHON_CANDIDATES).map(str::to_string)
}

fn emit_progress(app: &AppHandle, phase: &str, detail: Option<&str>) {
    let detail = detail.unwrap_or("");
    setup_status::publish(app, "venv-setup-progress", json!({ "phase": phase, "detail": detail }), "installing", phase, detail);
}

fn emit_error(app: &AppHandle, phase: &str, message: &str) {
    eprintln!("[venv] {} failed: {}", phase, message);
    setup_status::publish(app, "venv-setup-error", json!({ "phase": phase, "message": message }), "error", phase, message);
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
    // The last stderr line (e.g. pip's "ERROR: No matching distribution …")
    // goes into the error the user sees.
    let stderr_thread = stderr.map(|s| {
        std::thread::spawn(move || {
            let mut last = String::new();
            for line in std::io::BufReader::new(s).lines().map_while(Result::ok) {
                if line.is_empty() {
                    continue;
                }
                eprintln!("[venv:{}!] {}", phase_err, line);
                emit_progress(&app_err, &phase_err, Some(&line));
                last = line;
            }
            last
        })
    });

    let status = child
        .wait()
        .map_err(|e| format!("wait failed: {}", e))?;

    if let Some(t) = stdout_thread {
        let _ = t.join();
    }
    let last_stderr = stderr_thread.and_then(|t| t.join().ok()).unwrap_or_default();

    if status.success() {
        Ok(())
    } else if last_stderr.is_empty() {
        Err(status.to_string())
    } else {
        Err(format!("{status}: {last_stderr}"))
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
        setup_status::publish(app, "venv-setup-ready", json!({}), "ready", "", "");
        return Ok(());
    }

    // Marker present but stale: a full install succeeded on an earlier launch.
    let had_previous_install = previous_install(&venv_python_path(), &marker_path());
    setup_status::publish(app, "venv-setup-started", json!({}), "installing", "", "");
    // The install takes minutes; show its progress instead of a hidden window.
    crate::voice::stt::reveal_main_window(app, "venv setup");

    let requirements = resource_path("requirements-stt.txt");
    if !requirements.exists() {
        let msg = format!("requirements-stt.txt 缺失: {}。安装包不完整：请重新安装 Pocket Agent(源码运行请在仓库根目录执行 npm run tauri dev)。", requirements.display());
        emit_error(app, "checking-resources", &msg);
        return Err(msg);
    }

    if !venv_python_path().exists() {
        let system_python = match find_system_python3() {
            Ok(p) => p,
            Err(e) => {
                emit_error(app, "finding-python", &e);
                return Err(e);
            }
        };
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
            // A venv that was ready before only missed an update (new requirements,
            // e.g. a raised edge-tts floor). Offline or pip SSL trouble must not take
            // voice down: keep the old install and retry on the next launch.
            if had_previous_install && core_deps_importable(&venv_python_path()) {
                eprintln!("[venv] dependency update failed ({e}); keeping the previous install, will retry next launch");
                setup_status::publish(app, "venv-setup-ready", json!({}), "ready", "", "");
                return Ok(());
            }
            let msg = format!(
                "pip 安装依赖失败({e})。首次安装需要访问 PyPI(pypi.org):请检查网络或代理设置，然后重启 Pocket Agent 自动重试。"
            );
            emit_error(app, "installing-deps", &msg);
            return Err(msg);
        }
    }

    let marker_data = match marker_expected() {
        Ok(s) => s,
        Err(e) => {
            emit_error(app, "writing-marker", &e);
            return Err(e);
        }
    };
    if let Err(e) = std::fs::write(marker_path(), &marker_data) {
        let msg = format!("无法写 marker {}: {}", marker_path().display(), e);
        emit_error(app, "writing-marker", &msg);
        return Err(msg);
    }

    setup_status::publish(app, "venv-setup-done", json!({}), "ready", "", "");
    eprintln!("[venv] PA venv ready at {}", venv_root().display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fake_python(path: &Path, exit_code: i32) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("#!/bin/sh\nexit {exit_code}\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn bundled_resources_are_found_in_the_app() {
        let dir = std::env::temp_dir().join(format!("pa-venv-res-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let contents = dir.join("Pocket Agent.app/Contents");
        let exe = contents.join("MacOS/pocket-agent");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::create_dir_all(contents.join("Resources/resources")).unwrap();
        std::fs::write(contents.join("Resources/resources/requirements-stt.txt"), "x").unwrap();
        std::fs::write(contents.join("Resources/stt-helper"), "old").unwrap();

        assert_eq!(
            resource_path_from(&exe, "requirements-stt.txt"),
            Some(contents.join("Resources/resources/requirements-stt.txt"))
        );
        assert_eq!(
            resource_path_from(&exe, "stt-helper"),
            Some(contents.join("Resources/stt-helper"))
        );
        assert_eq!(resource_path_from(&exe, "missing.py"), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A python3 stand-in that reports `version` (or fails `import venv`).
    fn fake_versioned_python(path: &Path, version: &str, has_venv: bool) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let body = if has_venv { format!("echo {version}") } else { "echo 'No module named venv' >&2; exit 1".to_string() };
        std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn python_pick_skips_unsupported_versions() {
        let dir = std::env::temp_dir().join(format!("pa-venv-python-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let old = dir.join("old/python3");
        let new = dir.join("new/python3");
        let no_venv = dir.join("novenv/python3");
        let ok = dir.join("ok/python3");
        fake_versioned_python(&old, "3.8", true);
        fake_versioned_python(&new, "3.13", true);
        fake_versioned_python(&no_venv, "3.11", false);
        fake_versioned_python(&ok, "3.9", true);
        let p = |path: &PathBuf| path.to_str().unwrap().to_string();
        let (old, new, no_venv, ok) = (p(&old), p(&new), p(&no_venv), p(&ok));
        let missing = p(&dir.join("missing/python3"));

        // An old interpreter first, a supported one later.
        assert_eq!(pick_python(&[&old, &no_venv, &ok]).unwrap(), ok);

        // None usable: every candidate is listed with its version or reason.
        let err = pick_python(&[&old, &new, &no_venv, &missing]).unwrap_err();
        assert!(err.contains(&format!("{old}: 3.8")), "{err}");
        assert!(err.contains(&format!("{new}: 3.13")), "{err}");
        assert!(err.contains(&format!("{no_venv}: 无法运行或缺少 venv 模块")), "{err}");
        assert!(err.contains(&format!("{missing}: 不存在")), "{err}");
        assert!(err.contains("xcode-select --install"), "{err}");
        assert!(!err.contains('\n'), "the status panel collapses newlines: {err}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn leftover_marker_does_not_survive_a_failed_rebuild() {
        let dir = std::env::temp_dir().join(format!("pa-venv-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let python = dir.join("venv/bin/python3");
        let marker = dir.join(".venv-ready");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&marker, "old requirements").unwrap();

        // Launch 1: venv deleted, marker left over → rebuild, pip fails.
        assert!(!previous_install(&python, &marker));
        assert!(!marker.exists());
        fake_python(&python, 1);

        // Launch 2: half-built interpreter exists, marker gone → still no fallback.
        assert!(!previous_install(&python, &marker));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fallback_requires_core_imports() {
        let dir = std::env::temp_dir().join(format!("pa-venv-imports-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let broken = dir.join("broken/python3");
        let working = dir.join("working/python3");
        fake_python(&broken, 1);
        fake_python(&working, 0);

        assert!(!core_deps_importable(&broken));
        assert!(core_deps_importable(&working));
        assert!(!core_deps_importable(&dir.join("missing/python3")));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
