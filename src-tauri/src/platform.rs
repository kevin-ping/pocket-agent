//! Small helpers that differ between macOS and Windows.

use std::path::PathBuf;
use std::process::Command;

/// The user's home directory (`$HOME` on macOS, `%USERPROFILE%` on Windows).
/// PA's data stays in `~/.pocket-agent` on every platform, as the Python side
/// (`Path.home()`) expects.
pub fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_default()
}

/// Hermes' data directory: `HERMES_HOME` if set, else `%LOCALAPPDATA%\hermes`
/// on Windows (where the Hermes installer puts it) and `~/.hermes` elsewhere.
pub fn hermes_home() -> PathBuf {
    if let Some(h) = std::env::var_os("HERMES_HOME").filter(|h| !h.is_empty()) {
        return PathBuf::from(h);
    }
    #[cfg(windows)]
    if let Some(local) = dirs::data_local_dir() {
        return local.join("hermes");
    }
    home_dir().join(".hermes")
}

/// How to refer to Hermes' `.env` in messages.
pub fn hermes_env_display() -> String {
    if std::env::var_os("HERMES_HOME").is_some_and(|h| !h.is_empty()) {
        hermes_home().join(".env").display().to_string()
    } else if cfg!(windows) {
        r"%LOCALAPPDATA%\hermes\.env".to_string()
    } else {
        "~/.hermes/.env".to_string()
    }
}

/// The machine PA runs on, as named in prompts sent to Hermes. A macro so
/// `concat!` can build constant prompts from it.
#[cfg(windows)]
macro_rules! machine_name {
    () => {
        "Windows PC"
    };
}
#[cfg(not(windows))]
macro_rules! machine_name {
    () => {
        "Mac"
    };
}
pub(crate) use machine_name;
pub const MACHINE: &str = machine_name!();

/// Keep a console child (python, pip, edge-tts) from opening its own console
/// window on Windows. Only when PA itself has no console (the GUI build): a
/// child of a console PA (`npm run tauri dev`) shares that console, opens no
/// window, and its stderr still reaches the PA log. No-op elsewhere.
pub fn no_window(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if !has_console() {
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
    }
    cmd
}

/// Kill `child` and everything it started. On Windows a venv's `python.exe`
/// and pip's console scripts (`edge-tts.exe`) are launchers that run the real
/// interpreter as a child process, so `Child::kill` alone would orphan it.
/// The caller still reaps `child` with `wait`.
pub fn kill_tree(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        let _ = no_window(&mut Command::new("taskkill"))
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    let _ = child.kill();
}

/// Keep a listening socket out of child processes. On Windows the socket
/// tokio creates is inheritable, so stt-server held PA's port 8650 after PA
/// died: the port stayed bound, nobody answered, and the next PA could not
/// bind it. No-op elsewhere (Rust sockets are close-on-exec there).
#[cfg(windows)]
pub fn not_inherited(socket: &impl std::os::windows::io::AsRawSocket) {
    extern "system" {
        fn SetHandleInformation(handle: *mut std::ffi::c_void, mask: u32, flags: u32) -> i32;
    }
    const HANDLE_FLAG_INHERIT: u32 = 1;
    // SAFETY: the socket is open for the duration of the call; only its
    // inherit flag changes.
    if unsafe { SetHandleInformation(socket.as_raw_socket() as _, HANDLE_FLAG_INHERIT, 0) } == 0 {
        eprintln!("[platform] could not mark socket non-inheritable: {}", std::io::Error::last_os_error());
    }
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(windows)]
fn has_console() -> bool {
    extern "system" {
        fn GetConsoleWindow() -> *mut std::ffi::c_void;
    }
    // SAFETY: no arguments; returns NULL when the process has no console.
    unsafe { !GetConsoleWindow().is_null() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hermes_home_honours_the_override() {
        let _serial = crate::voice::stt::test_serial();
        let old = std::env::var_os("HERMES_HOME");
        std::env::set_var("HERMES_HOME", "/custom/hermes");
        assert_eq!(hermes_home(), PathBuf::from("/custom/hermes"));
        std::env::remove_var("HERMES_HOME");
        let expected = if cfg!(windows) {
            dirs::data_local_dir().unwrap().join("hermes")
        } else {
            home_dir().join(".hermes")
        };
        assert_eq!(hermes_home(), expected);
        if let Some(v) = old {
            std::env::set_var("HERMES_HOME", v);
        }
    }

    #[test]
    fn no_window_leaves_the_command_runnable() {
        let mut cmd = Command::new(if cfg!(windows) { "cmd" } else { "true" });
        if cfg!(windows) {
            cmd.args(["/C", "exit 0"]);
        }
        assert!(no_window(&mut cmd).status().unwrap().success());
    }
}
