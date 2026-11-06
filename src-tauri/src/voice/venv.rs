use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::json;
use tauri::AppHandle;

use super::setup_status;

const VENV_DIR_RELATIVE: [&str; 2] = [".pocket-agent", "venv"];
const VENV_MARKER_RELATIVE: [&str; 2] = [".pocket-agent", ".venv-ready"];

fn home_dir() -> PathBuf {
    crate::platform::home_dir()
}

pub fn venv_root() -> PathBuf {
    VENV_DIR_RELATIVE.iter().fold(home_dir(), |p, c| p.join(c))
}

/// A program in the venv at `root`: `bin/<name>` on macOS,
/// `Scripts\<name>.exe` on Windows.
pub fn venv_bin_in(root: &Path, name: &str) -> PathBuf {
    if cfg!(windows) {
        root.join("Scripts").join(format!("{name}.exe"))
    } else {
        root.join("bin").join(name)
    }
}

/// A program in PA's own venv (e.g. `edge-tts`).
pub fn venv_bin(name: &str) -> PathBuf {
    venv_bin_in(&venv_root(), name)
}

fn venv_python_in(root: &Path) -> PathBuf {
    venv_bin_in(root, if cfg!(windows) { "python" } else { "python3" })
}

pub fn venv_python_path() -> PathBuf {
    venv_python_in(&venv_root())
}

fn marker_path() -> PathBuf {
    VENV_MARKER_RELATIVE.iter().fold(home_dir(), |p, c| p.join(c))
}

/// Ready to start stt-server without installing anything: the marker matches
/// the requirements, and the venv it describes still has pip and the core
/// modules. The marker lives outside the venv, so it alone proves nothing: a
/// rebuild interrupted by an older PA can leave it next to a bare venv.
fn venv_ready(root: &Path, marker: &Path, expected: &str) -> bool {
    let Ok(actual) = std::fs::read_to_string(marker) else {
        return false;
    };
    // Tolerate trailing whitespace differences (e.g. some editors strip the final newline).
    actual.trim_end() == expected.trim_end()
        && venv_health(root) == VenvHealth::Usable
        && core_deps_importable(&venv_python_in(root))
}

/// Whether a pip failure may fall back to an earlier full install: a marker
/// (possibly for older requirements) next to a venv that can still import the
/// core modules. A marker without them is left over from an interrupted
/// rebuild and is dropped, so the half-built venv is never taken for the old one.
fn previous_install(python: &Path, marker: &Path) -> bool {
    if !marker.exists() {
        return false;
    }
    if core_deps_importable(python) {
        return true;
    }
    let _ = std::fs::remove_file(marker);
    false
}

/// Modules stt-server cannot start without. edge-tts is left out on purpose:
/// an older edge-tts still serves STT, wake and voiceprints.
const CORE_IMPORTS: &str =
    "import numpy, faster_whisper, onnxruntime, fastapi, uvicorn, scipy, kaldi_native_fbank";

fn core_deps_importable(python: &Path) -> bool {
    crate::platform::no_window(&mut Command::new(python))
        .arg("-c")
        .arg(CORE_IMPORTS)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn runs(program: &Path, args: &[&str]) -> bool {
    crate::platform::no_window(&mut Command::new(program))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[derive(Debug, PartialEq)]
enum VenvHealth {
    /// No interpreter, or one that no longer runs (e.g. its system Python is gone).
    NoInterpreter,
    /// Interpreter runs, pip does not: venv creation was interrupted.
    NoPip,
    Usable,
}

fn venv_health(root: &Path) -> VenvHealth {
    let python = venv_python_in(root);
    if !runs(&python, &["-c", ""]) {
        VenvHealth::NoInterpreter
    } else if !runs(&python, &["-m", "pip", "--version"]) {
        VenvHealth::NoPip
    } else {
        VenvHealth::Usable
    }
}

/// Make the venv at `root` able to run pip: reuse it, restore pip in place
/// (offline ensurepip), or rebuild it with `--clear`, which empties only `root`.
/// `marker` is removed before the rebuild starts, so an interrupted rebuild
/// never leaves a ready marker behind.
/// Returns whether it was rebuilt (anything installed before is gone).
/// Errors carry the phase to report.
fn prepare_venv(
    root: &Path,
    marker: &Path,
    find_python: impl FnOnce() -> Result<String, String>,
    run: &mut dyn FnMut(&str, Command) -> Result<(), String>,
) -> Result<bool, (&'static str, String)> {
    match venv_health(root) {
        VenvHealth::Usable => return Ok(false),
        VenvHealth::NoPip => {
            let mut cmd = Command::new(venv_python_in(root));
            cmd.args(["-m", "ensurepip", "--upgrade", "--default-pip"]);
            if let Err(e) = run("creating-venv", cmd) {
                eprintln!("[venv] ensurepip failed ({e}); rebuilding the venv");
            }
            if venv_health(root) == VenvHealth::Usable {
                return Ok(false);
            }
        }
        VenvHealth::NoInterpreter => {}
    }

    let system_python = find_python().map_err(|e| ("finding-python", e))?;
    if let Some(parent) = root.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ("creating-venv", format!("无法创建 {}: {}", parent.display(), e)))?;
    }
    if let Err(e) = std::fs::remove_file(marker) {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(("creating-venv", format!("无法删除 {}: {}", marker.display(), e)));
        }
    }
    let mut cmd = Command::new(&system_python);
    cmd.args(["-m", "venv", "--clear"]).arg(root);
    run("creating-venv", cmd).map_err(|e| ("creating-venv", e))?;
    match venv_health(root) {
        VenvHealth::Usable => Ok(true),
        _ => Err((
            "creating-venv",
            format!(
                "语音环境不完整：用 {system_python} 新建的 {} 没有可用的 pip。{}",
                root.display(),
                REINSTALL_PYTHON_HINT,
            ),
        )),
    }
}

/// pip's last stderr line when it could not reach the index.
fn looks_like_network_error(e: &str) -> bool {
    [
        "Could not fetch", "NewConnectionError", "Max retries", "SSLError", "ProxyError",
        "ConnectTimeout", "ReadTimeout", "timed out", "Temporary failure in name resolution",
        "nodename nor servname", "Network is unreachable", "from versions: none",
    ]
    .iter()
    .any(|k| e.contains(k))
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
#[cfg(not(windows))]
fn resource_path_from(exe: &Path, filename: &str) -> Option<PathBuf> {
    let resources = exe.parent()?.parent()?.join("Resources");
    [resources.join("resources").join(filename), resources.join(filename)]
        .into_iter()
        .find(|p| p.exists())
}

/// On Windows Tauri's resource directory is the exe's own directory, so
/// `resources/<file>` sits at `<exe dir>\resources\<file>` (also in dev,
/// where the build copies it next to `target\debug\pocket-agent.exe`).
#[cfg(windows)]
fn resource_path_from(exe: &Path, filename: &str) -> Option<PathBuf> {
    Some(exe.parent()?.join("resources").join(filename)).filter(|p| p.exists())
}

/// Interpreters tried for the venv, in order. The CLT python3 (3.9) comes
/// first: it is what Finder-launched PA finds, and the sherpa compat build
/// for macOS < 13.4 only ships cp39 wheels.
#[cfg(not(windows))]
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

#[cfg(not(windows))]
fn python_candidates() -> Vec<String> {
    PYTHON_CANDIDATES.iter().map(|s| s.to_string()).collect()
}

/// Windows: what the `py` launcher finds for 3.12/3.11/3.10 (64-bit), the
/// python.org per-user installs, then `python.exe` on PATH. The Microsoft
/// Store stub in `WindowsApps` is skipped: running it opens the Store.
#[cfg(windows)]
fn python_candidates() -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for minor in [12, 11, 10] {
        let out = crate::platform::no_window(&mut Command::new("py"))
            .args([&format!("-3.{minor}-64"), "-c", "import sys; print(sys.executable)"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        if let Some(out) = out.ok().filter(|o| o.status.success()) {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !path.is_empty() {
                found.push(path);
            }
        }
    }
    if let Some(local) = dirs::data_local_dir() {
        for minor in [12, 11, 10] {
            let p = local.join(format!(r"Programs\Python\Python3{minor}\python.exe"));
            if p.is_file() {
                found.push(p.display().to_string());
            }
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        if let Some(p) = std::env::split_paths(&path)
            .filter(|d| !d.to_string_lossy().to_ascii_lowercase().contains("windowsapps"))
            .map(|d| d.join("python.exe"))
            .find(|p| p.is_file())
        {
            found.push(p.display().to_string());
        }
    }
    let mut seen = std::collections::HashSet::new();
    found.retain(|p| seen.insert(p.to_ascii_lowercase()));
    found
}

#[cfg(not(windows))]
const INSTALL_PYTHON_HINT: &str = "请运行 xcode-select --install 安装 Xcode Command Line Tools(自带 Python 3.9);如果已安装但版本不在此范围，请安装 Python 3.12(例如 brew install python@3.12),然后重启 Pocket Agent。";
#[cfg(windows)]
const INSTALL_PYTHON_HINT: &str = "请安装 64 位 Python 3.12(winget install Python.Python.3.12,或从 python.org 下载),然后重启 Pocket Agent。";

#[cfg(not(windows))]
const REINSTALL_PYTHON_HINT: &str = "请运行 xcode-select --install 重新安装 Command Line Tools(或安装 Python 3.9–3.12),然后重启 Pocket Agent。";
#[cfg(windows)]
const REINSTALL_PYTHON_HINT: &str = "请重新安装 64 位 Python 3.12(winget install Python.Python.3.12,或从 python.org 下载，安装时保留 pip),然后重启 Pocket Agent。";

/// requirements-stt.txt pins numpy<2, whose last release (1.26) stops at 3.12.
const PYTHON_MIN: (u32, u32) = (3, 9);
const PYTHON_MAX: (u32, u32) = (3, 12);

/// `Ok(version)` when `candidate` runs and can `import venv`; otherwise a
/// short reason for the error message.
fn probe_python(candidate: &str) -> Result<(u32, u32), String> {
    let out = crate::platform::no_window(&mut Command::new(candidate))
        .arg("-c")
        .arg("import sys, venv, struct; print('%d.%d%s' % (sys.version_info[0], sys.version_info[1], '' if struct.calcsize('P') == 8 else '-32'))")
        .stdin(Stdio::null())
        .output()
        .map_err(|_| "不存在".to_string())?;
    if !out.status.success() {
        return Err("无法运行或缺少 venv 模块".to_string());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let text = text.trim();
    // onnxruntime and faster-whisper ship 64-bit wheels only.
    if text.ends_with("-32") {
        return Err(format!("{} 32 位(不支持)", text.trim_end_matches("-32")));
    }
    let (major, minor) = text.split_once('.').ok_or("无法读取版本")?;
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
        "找不到可用的 Python {}.{}–{}.{}。已检查:{}。{}",
        PYTHON_MIN.0, PYTHON_MIN.1, PYTHON_MAX.0, PYTHON_MAX.1,
        if report.is_empty() { "无".to_string() } else { report.join("；") },
        INSTALL_PYTHON_HINT,
    ))
}

/// Locate a system python3 (3.9–3.12, with venv) to bootstrap the venv.
/// Returns the path that can be passed to `Command::new`.
pub fn find_system_python3() -> Result<String, String> {
    let candidates = python_candidates();
    let refs: Vec<&str> = candidates.iter().map(String::as_str).collect();
    pick_python(&refs).map(str::to_string)
}

/// `setup_status::publish` without the AppHandle: (event, payload, state, phase, detail).
type Publish<'a> = dyn Fn(&str, serde_json::Value, &'static str, &str, &str) + 'a;

fn emit_progress(publish: &Publish, phase: &str, detail: Option<&str>) {
    let detail = detail.unwrap_or("");
    publish("venv-setup-progress", json!({ "phase": phase, "detail": detail }), "installing", phase, detail);
}

fn emit_error(publish: &Publish, phase: &str, message: &str) {
    eprintln!("[venv] {} failed: {}", phase, message);
    publish("venv-setup-error", json!({ "phase": phase, "message": message }), "error", phase, message);
}

fn app_publisher(app: &AppHandle) -> impl Fn(&str, serde_json::Value, &'static str, &str, &str) + '_ {
    move |event, payload, state, phase, detail| setup_status::publish(app, event, payload, state, phase, detail)
}

fn run_streaming(
    app: &AppHandle,
    phase: &str,
    mut cmd: Command,
) -> Result<(), String> {
    crate::platform::no_window(&mut cmd);
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
                emit_progress(&app_publisher(&app_out), &phase_out, Some(&line));
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
                emit_progress(&app_publisher(&app_err), &phase_err, Some(&line));
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
/// (interrupted pip install) won't be mistaken for ready on next launch. It is
/// removed before any rebuild, and trusted only while the venv still has pip
/// and the core modules.
pub fn ensure_venv(app: &AppHandle) -> Result<(), String> {
    let publish = |event: &str, payload: serde_json::Value, state: &'static str, phase: &str, detail: &str| {
        setup_status::publish(app, event, payload, state, phase, detail);
        if event == "venv-setup-started" {
            // The install takes minutes; show its progress instead of a hidden window.
            crate::voice::stt::reveal_main_window(app, "venv setup");
        }
    };
    let layout = VenvLayout {
        root: venv_root(),
        marker: marker_path(),
        requirements: resource_path("requirements-stt.txt"),
    };
    ensure_venv_in(&layout, find_system_python3, &mut |phase, cmd| run_streaming(app, phase, cmd), &publish)
}

/// Where `ensure_venv` works: the venv, its ready marker, the requirements.
struct VenvLayout {
    root: PathBuf,
    marker: PathBuf,
    requirements: PathBuf,
}

/// All of `ensure_venv` but the AppHandle: commands go through `run`, status
/// events through `publish`.
fn ensure_venv_in(
    layout: &VenvLayout,
    find_python: impl FnOnce() -> Result<String, String>,
    run: &mut dyn FnMut(&str, Command) -> Result<(), String>,
    publish: &Publish,
) -> Result<(), String> {
    let python = venv_python_in(&layout.root);
    let requirements = &layout.requirements;
    // The marker holds the full requirements text (not a hash), so a maintainer
    // can `cat` it to see what was installed.
    let expected = std::fs::read_to_string(requirements)
        .map_err(|e| format!("无法读 requirements ({}): {}", requirements.display(), e));
    if let Ok(expected) = &expected {
        if venv_ready(&layout.root, &layout.marker, expected) {
            publish("venv-setup-ready", json!({}), "ready", "", "");
            return Ok(());
        }
    }

    // Marker present but stale: a full install succeeded on an earlier launch.
    let mut had_previous_install = previous_install(&python, &layout.marker);
    publish("venv-setup-started", json!({}), "installing", "", "");

    if !requirements.exists() {
        let msg = format!("requirements-stt.txt 缺失: {}。安装包不完整：请重新安装 Pocket Agent(源码运行请在仓库根目录执行 npm run tauri dev)。", requirements.display());
        emit_error(publish, "checking-resources", &msg);
        return Err(msg);
    }

    match prepare_venv(&layout.root, &layout.marker, find_python, &mut *run) {
        Ok(false) => {}
        Ok(true) => {
            // A rebuilt venv has nothing installed: no earlier install to fall back to.
            had_previous_install = false;
        }
        Err((phase, e)) => {
            emit_error(publish, phase, &e);
            return Err(e);
        }
    }

    emit_progress(publish, "upgrading-pip", None);
    {
        let mut cmd = Command::new(&python);
        cmd.arg("-m")
            .arg("pip")
            .arg("install")
            .arg("--upgrade")
            .arg("pip");
        if let Err(e) = run("upgrading-pip", cmd) {
            // Non-fatal: pip can still install with the bundled version
            eprintln!("[venv] pip upgrade non-fatal failure: {}", e);
        }
    }

    emit_progress(publish, "installing-deps", None);
    {
        let mut cmd = Command::new(&python);
        cmd.args(["-m", "pip", "install"])
            .arg("--disable-pip-version-check")
            .arg("-r")
            .arg(requirements);
        if let Err(e) = run("installing-deps", cmd) {
            // A venv that was ready before only missed an update (new requirements,
            // e.g. a raised edge-tts floor). Offline or pip SSL trouble must not take
            // voice down: keep the old install and retry on the next launch.
            if had_previous_install && core_deps_importable(&python) {
                eprintln!("[venv] dependency update failed ({e}); keeping the previous install, will retry next launch");
                publish("venv-setup-ready", json!({}), "ready", "", "");
                return Ok(());
            }
            let msg = if looks_like_network_error(&e) {
                format!("无法从 PyPI 下载依赖({e})。首次安装需要访问 pypi.org 和 files.pythonhosted.org:请检查网络或代理设置，然后重启 Pocket Agent,会从中断处继续安装。")
            } else {
                format!("pip 安装依赖失败({e})。完整输出见日志中的 [venv:installing-deps!] 行;修复后重启 Pocket Agent 会重新安装。如果怀疑语音环境损坏：退出 Pocket Agent,删除 ~/.pocket-agent/venv 和 ~/.pocket-agent/.venv-ready 后重启。")
            };
            emit_error(publish, "installing-deps", &msg);
            return Err(msg);
        }
    }

    let marker_data = match expected {
        Ok(s) => s,
        Err(e) => {
            emit_error(publish, "writing-marker", &e);
            return Err(e);
        }
    };
    if let Err(e) = std::fs::write(&layout.marker, &marker_data) {
        let msg = format!("无法写 marker {}: {}", layout.marker.display(), e);
        emit_error(publish, "writing-marker", &msg);
        return Err(msg);
    }

    publish("venv-setup-done", json!({}), "ready", "", "");
    eprintln!("[venv] PA venv ready at {}", layout.root.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A runnable stand-in script at `path` (`path.cmd` on Windows, where a
    /// script needs that extension to run). Returns the path to run.
    fn script(path: &Path, unix_body: &str, windows_body: &str) -> PathBuf {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = windows_body;
            std::fs::write(path, format!("#!/bin/sh\n{unix_body}\n")).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path.to_path_buf()
        }
        #[cfg(windows)]
        {
            let _ = unix_body;
            let path = path.with_extension("cmd");
            std::fs::write(&path, format!("@echo off\r\n{windows_body}\r\n")).unwrap();
            path
        }
    }

    fn fake_python(path: &Path, exit_code: i32) -> PathBuf {
        script(path, &format!("exit {exit_code}"), &format!("exit /b {exit_code}"))
    }

    #[cfg(not(windows))]
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

    #[cfg(windows)]
    #[test]
    fn bundled_resources_are_found_next_to_the_exe() {
        let dir = std::env::temp_dir().join(format!("pa-venv-res-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let exe = dir.join("pocket-agent.exe");
        std::fs::create_dir_all(dir.join("resources")).unwrap();
        std::fs::write(dir.join("resources").join("requirements-stt.txt"), "x").unwrap();
        assert_eq!(resource_path_from(&exe, "requirements-stt.txt"), Some(dir.join("resources").join("requirements-stt.txt")));
        assert_eq!(resource_path_from(&exe, "missing.py"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn venv_programs_use_the_platform_layout() {
        let root = Path::new("venv");
        if cfg!(windows) {
            assert_eq!(venv_python_in(root), root.join("Scripts").join("python.exe"));
            assert_eq!(venv_bin_in(root, "edge-tts"), root.join("Scripts").join("edge-tts.exe"));
        } else {
            assert_eq!(venv_python_in(root), root.join("bin").join("python3"));
            assert_eq!(venv_bin_in(root, "edge-tts"), root.join("bin").join("edge-tts"));
        }
    }

    /// A python3 stand-in that reports `version` (or fails `import venv`).
    fn fake_versioned_python(path: &Path, version: &str, has_venv: bool) -> PathBuf {
        if has_venv {
            script(path, &format!("echo {version}"), &format!("echo {version}"))
        } else {
            script(path, "echo 'No module named venv' >&2; exit 1", "echo No module named venv 1>&2\r\nexit /b 1")
        }
    }

    #[test]
    fn python_pick_skips_unsupported_versions() {
        let dir = std::env::temp_dir().join(format!("pa-venv-python-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let old = fake_versioned_python(&dir.join("old/python3"), "3.8", true);
        let new = fake_versioned_python(&dir.join("new/python3"), "3.13", true);
        let no_venv = fake_versioned_python(&dir.join("novenv/python3"), "3.11", false);
        let ok = fake_versioned_python(&dir.join("ok/python3"), "3.9", true);
        let x86 = fake_versioned_python(&dir.join("x86/python3"), "3.11-32", true);
        let p = |path: &PathBuf| path.to_str().unwrap().to_string();
        let (old, new, no_venv, ok, x86) = (p(&old), p(&new), p(&no_venv), p(&ok), p(&x86));
        let missing = p(&dir.join("missing/python3"));

        // An old interpreter first, a supported one later.
        assert_eq!(pick_python(&[&old, &no_venv, &ok]).unwrap(), ok);

        // None usable: every candidate is listed with its version or reason.
        let err = pick_python(&[&old, &new, &no_venv, &x86, &missing]).unwrap_err();
        assert!(err.contains(&format!("{x86}: 3.11 32 位")), "{err}");
        assert!(err.contains(&format!("{old}: 3.8")), "{err}");
        assert!(err.contains(&format!("{new}: 3.13")), "{err}");
        assert!(err.contains(&format!("{no_venv}: 无法运行或缺少 venv 模块")), "{err}");
        assert!(err.contains(&format!("{missing}: 不存在")), "{err}");
        assert!(err.contains(INSTALL_PYTHON_HINT), "{err}");
        assert!(!err.contains('\n'), "the status panel collapses newlines: {err}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn leftover_marker_does_not_survive_a_failed_rebuild() {
        let dir = std::env::temp_dir().join(format!("pa-venv-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let python = venv_python_in(&dir.join("venv"));
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
        let broken = fake_python(&dir.join("broken/python3"), 1);
        let working = fake_python(&dir.join("working/python3"), 0);

        assert!(!core_deps_importable(&broken));
        assert!(core_deps_importable(&working));
        assert!(!core_deps_importable(&dir.join("missing/python3")));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Real-interpreter scenario: None when this machine has no supported Python.
    fn system_python() -> Option<String> {
        match find_system_python3() {
            Ok(p) => Some(p),
            Err(e) => {
                eprintln!("skipped, no supported Python: {e}");
                None
            }
        }
    }

    fn run_quiet(_phase: &str, mut cmd: Command) -> Result<(), String> {
        let out = cmd.output().map_err(|e| format!("spawn failed: {e}"))?;
        if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).into_owned()) }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pa-venv-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn interrupted_venv_without_pip_is_repaired_in_place() {
        let Some(python) = system_python() else { return };
        let dir = scratch("nopip");
        let root = dir.join("venv");
        // What an interrupted `python -m venv` leaves: interpreter, no pip.
        assert!(Command::new(&python).args(["-m", "venv", "--without-pip"]).arg(&root).status().unwrap().success());
        assert_eq!(venv_health(&root), VenvHealth::NoPip);
        std::fs::write(root.join("kept.txt"), "x").unwrap();

        let rebuilt = prepare_venv(&root, &dir.join(".venv-ready"), || Ok(python.clone()), &mut run_quiet).unwrap();
        assert!(!rebuilt, "pip restored in place");
        assert_eq!(venv_health(&root), VenvHealth::Usable);
        assert!(root.join("kept.txt").exists());
        // A second launch reuses it as is.
        assert!(!prepare_venv(&root, &dir.join(".venv-ready"), || panic!("no rebuild"), &mut run_quiet).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn venv_whose_interpreter_no_longer_runs_is_rebuilt_without_touching_siblings() {
        let Some(python) = system_python() else { return };
        let dir = scratch("broken");
        let root = dir.join("venv");
        // Present, but does not run (on Windows: not a real exe).
        std::fs::create_dir_all(venv_python_in(&root).parent().unwrap()).unwrap();
        std::fs::write(venv_python_in(&root), "not a program").unwrap();
        std::fs::write(root.join("leftover"), "x").unwrap();
        std::fs::write(dir.join(".env"), "API_SERVER_KEY=keep").unwrap();
        std::fs::write(dir.join("settings.db"), "keep").unwrap();

        assert_eq!(venv_health(&root), VenvHealth::NoInterpreter);
        assert!(prepare_venv(&root, &dir.join(".venv-ready"), || Ok(python.clone()), &mut run_quiet).unwrap());
        assert_eq!(venv_health(&root), VenvHealth::Usable);
        assert!(!root.join("leftover").exists(), "--clear empties the venv");
        assert_eq!(std::fs::read_to_string(dir.join(".env")).unwrap(), "API_SERVER_KEY=keep");
        assert!(dir.join("settings.db").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rebuild_that_still_lacks_pip_says_what_to_do() {
        let Some(python) = system_python() else { return };
        let dir = scratch("stillnopip");
        let root = dir.join("venv");
        // A system Python whose venv module never installs pip.
        let creator = script(
            &dir.join("python3"),
            &format!("exec {python} \"$@\" --without-pip"),
            &format!("\"{python}\" %* --without-pip"),
        );
        let creator = creator.to_str().unwrap().to_string();

        let (phase, msg) = prepare_venv(&root, &dir.join(".venv-ready"), || Ok(creator.clone()), &mut |phase, cmd| {
            // ensurepip is not reached: the venv is missing, so it is created first.
            assert_eq!(phase, "creating-venv");
            run_quiet(phase, cmd)
        })
        .unwrap_err();
        assert_eq!(phase, "creating-venv");
        assert!(msg.contains("没有可用的 pip") && msg.contains(REINSTALL_PYTHON_HINT), "{msg}");
        assert!(!msg.contains("网络"), "a local problem is not reported as a network one: {msg}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_python_is_reported_as_such() {
        let dir = scratch("nopython");
        let (phase, msg) = prepare_venv(&dir.join("venv"), &dir.join(".venv-ready"), || Err("找不到可用的 Python".into()), &mut run_quiet).unwrap_err();
        assert_eq!((phase, msg.as_str()), ("finding-python", "找不到可用的 Python"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn network_failures_are_told_apart_from_other_pip_errors() {
        assert!(looks_like_network_error("ERROR: Could not find a version that satisfies the requirement numpy<2 (from versions: none)"));
        assert!(looks_like_network_error("WARNING: Retrying ... NewConnectionError"));
        assert!(!looks_like_network_error("ERROR: Failed building wheel for kaldi-native-fbank"));
        assert!(!looks_like_network_error("spawn failed: No such file or directory (os error 2)"));
    }

    /// A ~/.pocket-agent stand-in: the venv, its marker, the requirements, and
    /// the user's own files next to them.
    struct Home {
        dir: PathBuf,
        layout: VenvLayout,
    }

    const USER_FILES: &[&str] = &[".env", "settings.db", "chat_history.db", "server.token", "voiceprints/owner.bin", "models/whisper/model.bin"];

    impl Home {
        fn new(name: &str) -> Home {
            let dir = scratch(name);
            let requirements = dir.join("requirements-stt.txt");
            std::fs::write(&requirements, "numpy<2\nfaster-whisper\n").unwrap();
            for f in USER_FILES {
                let path = dir.join(f);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, format!("keep {f}")).unwrap();
            }
            let layout = VenvLayout { root: dir.join("venv"), marker: dir.join(".venv-ready"), requirements };
            Home { dir, layout }
        }

        /// What an earlier successful install wrote.
        fn write_matching_marker(&self) {
            std::fs::copy(&self.layout.requirements, &self.layout.marker).unwrap();
        }

        /// `ensure_venv_in` with no network: pip's self-upgrade fails (as it does
        /// offline, non-fatally); `pip install -r` stands in core modules when
        /// `index_up`, else fails like an unreachable index. Everything else
        /// (venv, ensurepip) runs for real. Returns the result and the events.
        fn ensure(&self, python: &str, index_up: bool) -> (Result<(), String>, Vec<String>) {
            let events = std::cell::RefCell::new(Vec::new());
            let publish = |event: &str, _: serde_json::Value, _: &'static str, _: &str, _: &str| events.borrow_mut().push(event.to_string());
            let result = ensure_venv_in(&self.layout, || Ok(python.to_string()), &mut |phase, cmd| match phase {
                "upgrading-pip" => Err("WARNING: Retrying ... NewConnectionError".into()),
                "installing-deps" if index_up => {
                    let site = Command::new(cmd.get_program())
                        .args(["-c", "import sysconfig; print(sysconfig.get_paths()['purelib'])"])
                        .output()
                        .unwrap();
                    let site = PathBuf::from(String::from_utf8_lossy(&site.stdout).trim());
                    for module in ["numpy", "faster_whisper", "onnxruntime", "fastapi", "uvicorn", "scipy", "kaldi_native_fbank"] {
                        std::fs::write(site.join(format!("{module}.py")), "").unwrap();
                    }
                    Ok(())
                }
                "installing-deps" => Err("ERROR: Could not find a version that satisfies the requirement numpy<2 (from versions: none)".into()),
                _ => run_quiet(phase, cmd),
            }, &publish);
            (result, events.into_inner())
        }

        fn assert_user_files_kept(&self) {
            for f in USER_FILES {
                assert_eq!(std::fs::read_to_string(self.dir.join(f)).unwrap(), format!("keep {f}"), "{f} touched");
            }
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn matching_marker_next_to_a_venv_without_pip_is_not_ready() {
        let Some(python) = system_python() else { return };
        let home = Home::new("marker-nopip");
        assert!(Command::new(&python).args(["-m", "venv", "--without-pip"]).arg(&home.layout.root).status().unwrap().success());
        home.write_matching_marker();

        // Offline: no ready, and the marker no longer vouches for this venv.
        let (result, events) = home.ensure(&python, false);
        assert!(result.unwrap_err().contains("PyPI"));
        assert!(!events.iter().any(|e| e == "venv-setup-ready"), "{events:?}");
        assert!(!home.layout.marker.exists());
        assert_eq!(venv_health(&home.layout.root), VenvHealth::Usable, "pip restored in place");

        // Next launch with the index reachable completes the install.
        let (result, events) = home.ensure(&python, true);
        result.unwrap();
        assert_eq!(events.first().map(String::as_str), Some("venv-setup-started"));
        assert_eq!(events.last().map(String::as_str), Some("venv-setup-done"));
        assert!(!events.iter().any(|e| e == "venv-setup-ready"), "{events:?}");
        assert_eq!(std::fs::read_to_string(&home.layout.marker).unwrap(), std::fs::read_to_string(&home.layout.requirements).unwrap());

        // And only then is the fast path taken.
        let (result, events) = home.ensure(&python, false);
        result.unwrap();
        assert_eq!(events, ["venv-setup-ready"]);
        home.assert_user_files_kept();
    }

    #[test]
    fn marker_left_by_an_interrupted_rebuild_without_core_modules_is_not_ready() {
        let Some(python) = system_python() else { return };
        let home = Home::new("marker-nodeps");
        // A rebuild by an older PA, interrupted after pip and before the deps.
        assert!(Command::new(&python).args(["-m", "venv"]).arg(&home.layout.root).status().unwrap().success());
        assert_eq!(venv_health(&home.layout.root), VenvHealth::Usable);
        home.write_matching_marker();

        // Offline: no "keep the previous install" either, since there is none.
        let (result, events) = home.ensure(&python, false);
        assert!(result.unwrap_err().contains("PyPI"));
        assert!(!events.iter().any(|e| e == "venv-setup-ready"), "{events:?}");
        assert!(!home.layout.marker.exists());

        let (result, events) = home.ensure(&python, true);
        result.unwrap();
        assert_eq!(events.last().map(String::as_str), Some("venv-setup-done"));
        assert!(core_deps_importable(&venv_python_in(&home.layout.root)));
        home.assert_user_files_kept();
    }

    #[test]
    fn rebuild_drops_the_marker_before_clearing_the_venv() {
        let Some(python) = system_python() else { return };
        let home = Home::new("rebuild-interrupted");
        // A complete install whose pip broke: the core modules still import, so
        // the marker still counts as a previous install until the rebuild.
        home.ensure(&python, true).0.unwrap();
        let site = home.layout.root.join("lib");
        for entry in walk(&site) {
            let name = entry.file_name().unwrap().to_string_lossy().into_owned();
            if name == "pip" || name.starts_with("pip-") {
                std::fs::remove_dir_all(&entry).unwrap();
            }
        }
        assert_eq!(venv_health(&home.layout.root), VenvHealth::NoPip);
        assert!(home.layout.marker.exists());

        // ensurepip fails, so the venv is rebuilt; the rebuild is killed once
        // the interpreter exists, before pip.
        let events = std::cell::RefCell::new(Vec::new());
        let publish = |event: &str, _: serde_json::Value, _: &'static str, _: &str, _: &str| events.borrow_mut().push(event.to_string());
        let result = ensure_venv_in(&home.layout, || Ok(python.clone()), &mut |phase, mut cmd| {
            if cmd.get_args().any(|a| a == "ensurepip") {
                return Err("ensurepip failed".into());
            }
            if cmd.get_args().any(|a| a == "--clear") {
                assert!(!home.layout.marker.exists(), "marker still present when the venv is cleared");
                cmd.arg("--without-pip");
                run_quiet(phase, cmd)?;
                return Err("signal: 9 (SIGKILL)".into());
            }
            run_quiet(phase, cmd)
        }, &publish);
        assert!(result.is_err());
        assert!(!events.into_inner().iter().any(|e| e == "venv-setup-ready"));
        assert!(!home.layout.marker.exists());
        assert_eq!(venv_health(&home.layout.root), VenvHealth::NoPip);

        // Next launch repairs it through the normal path.
        let (result, events) = home.ensure(&python, true);
        result.unwrap();
        assert_eq!(events.last().map(String::as_str), Some("venv-setup-done"));
        assert!(!events.iter().any(|e| e == "venv-setup-ready"), "{events:?}");
        home.assert_user_files_kept();
    }

    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                found.extend(walk(&path));
            }
            found.push(path);
        }
        found
    }

    #[test]
    fn update_failure_still_keeps_a_complete_previous_install() {
        let Some(python) = system_python() else { return };
        let home = Home::new("marker-update");
        home.ensure(&python, true).0.unwrap();
        // New requirements, index unreachable: the old install keeps serving.
        std::fs::write(&home.layout.requirements, "numpy<2\nfaster-whisper\nedge-tts>=7\n").unwrap();
        let (result, events) = home.ensure(&python, false);
        result.unwrap();
        assert_eq!(events.last().map(String::as_str), Some("venv-setup-ready"));
        assert!(home.layout.marker.exists(), "kept for the next launch's retry");
        home.assert_user_files_kept();
    }
}
