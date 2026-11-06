# Pocket Agent on Windows (from source)

> **Status: experimental.** Pocket Agent runs on Windows 10/11 (x64) from source with `npm run tauri dev`. There is no installer yet. Verified on Windows 10 22H2 (19045) with Hermes v0.22.7: text chat, dictation, spoken replies, the voice service, wake word and voiceprint, and desktop control through Hermes `computer_use`. The Android phone terminal has not been tried against a Windows PC yet.
>
> Agents: follow these steps in order; every step says how to check it worked. Run commands in PowerShell unless a step says otherwise.

## 1. Prerequisites

| Tool | Install | Check |
|---|---|---|
| Rust (MSVC toolchain) | `winget install Rustlang.Rustup`, then open a new terminal | `rustc --version` |
| C++ build tools | `winget install Microsoft.VisualStudio.2022.BuildTools --override "--quiet --wait --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"` (asks for administrator approval; several GB) | `C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC` exists |
| Node.js 18+ | `winget install OpenJS.NodeJS.LTS` | `node --version` |
| Python 3.10–3.12, **64-bit** | `winget install Python.Python.3.12` (or python.org) | `py -3.12 -c "import struct; print(struct.calcsize('P')*8)"` prints `64` |
| WebView2 | Preinstalled on Windows 11 and current Windows 10 | — |

Python is only used to build PA's own voice environment; nothing is installed into it by hand. PA looks for an interpreter in this order: `py -3.12`, `py -3.11`, `py -3.10` (64-bit), then `%LOCALAPPDATA%\Programs\Python\Python31x\python.exe`, then `python.exe` on `PATH`. It skips the Microsoft Store placeholder in `WindowsApps` and rejects 32-bit Pythons (the speech libraries have 64-bit wheels only). Python 3.13+ is not supported (`numpy<2`).

## 2. Hermes

Install Hermes natively on Windows with its official installer and run `hermes setup` (see the Hermes docs). Differences from macOS:

- **Hermes home is `%LOCALAPPDATA%\hermes`**, not `~/.hermes` (unless `HERMES_HOME` is set). Its `.env` is `%LOCALAPPDATA%\hermes\.env`.
- Turn on the API server in that `.env`:

  ```
  API_SERVER_ENABLED=true
  API_SERVER_KEY=<a long random string>
  ```

  A random key: `python -c "import secrets; print(secrets.token_hex(24))"`.
- Restart the gateway: `hermes gateway restart` (on Windows it runs as a login item, not a launchd service). Check that a real reply streams back with the `curl` command in the README's *Set up Hermes from scratch*, step 5. In PowerShell call `curl.exe`, not `curl`.

### Optional: desktop control and the Pocket Agent skill

```powershell
hermes tools enable --platform api_server computer_use
hermes gateway restart
hermes computer-use doctor
```

`doctor` should end with `cua-driver ... on win32 — ok`. Windows needs no Accessibility or Screen Recording permission. If `ax_capability` reports that UI Automation is not reachable, run Hermes from your normal desktop session (not a Windows service or an SSH session) and check again.

Install PA's skill into the profile PA uses (default profile shown):

```powershell
New-Item -ItemType Directory -Force "$env:LOCALAPPDATA\hermes\skills\my-skills" | Out-Null
Copy-Item -Recurse <pocket-agent repo>\skills\pocket-agent "$env:LOCALAPPDATA\hermes\skills\my-skills\"
hermes skills list   # pocket-agent is listed as enabled
```

## 3. Pocket Agent

```powershell
git clone https://github.com/kevin-ping/pocket-agent.git
cd pocket-agent
npm install
if (-not (Test-Path .env)) { Copy-Item .env.example .env }
```

In `.env` set `API_SERVER=http://localhost:8642` and `API_SERVER_KEY` to the same key as in `%LOCALAPPDATA%\hermes\.env`. Leave `API_AGENT` unset.

```powershell
npm run tauri dev
```

The first start compiles the Rust side (several minutes), then builds the voice environment in `%USERPROFILE%\.pocket-agent\venv` and downloads the Whisper and voiceprint models. The status panel under the avatar shows each stage. Logs appear in the terminal (`[venv]`, `[stt]`, `[stt-server]`). Check that the voice service is up: `curl.exe http://127.0.0.1:8651/health` returns `{"status":"ok",...,"pid":...}`.

Settings, history, voiceprints and models live in `%USERPROFILE%\.pocket-agent`, as `~/.pocket-agent` does on macOS.

## 4. Microphone

PA records from the **Windows default input device**. Before the first recording, choose the microphone you actually use in **Settings → System → Sound → Input**, then restart PA (the device is picked at start).

Virtual devices such as *Steam Streaming Microphone* record pure silence. PA reports a silent recording with the fix instead of transcribing it. If the stream cannot be opened at all, check **Settings → Privacy → Microphone** (Windows 11: *Privacy & security*) → *Allow desktop apps to access your microphone*.

## 5. Check the install

Same layers as on macOS (README, *Check the install, step by step*):

1. **Text chat**: type a message; Hermes replies.
2. **Dictation**: press **RightShift**, speak, press it again; the words appear and are sent. **Esc** cancels a recording.
3. **Spoken reply**: the reply is read aloud (edge-tts, needs the internet).
4. *(Optional)* **Wake word and voiceprint**: Settings → Wake word; download the model, record the voiceprint, then **Test**.
5. *(Optional)* **Desktop control**: e.g. "open Notepad and write one sentence"; Hermes asks for approval before acting.

## Windows notes

- **Hotkey**: same behaviour as on macOS: one press starts recording, the next stops it; double-click mode works too. Key auto-repeat is ignored. While a window running **as administrator** is in front, Windows does not deliver keys to PA. Click another window first, or run PA elevated too.
- **No console windows**: the voice service, pip and edge-tts run without opening console windows.
- **Port 1420 in use**: `npm run tauri dev` stops and prints the PID. Check with `netstat -ano | findstr :1420` and end it with `taskkill /PID <PID>` if it is safe.
- **Port 8651 in use by another program**: `netstat -ano | findstr :8651` shows it. A voice service left by an earlier PA is replaced automatically.
- **Avatar**: on 100%-scaled displays the avatar is rendered at twice the size and scaled down, so it stays as sharp as on a Retina Mac.
- **Phone terminal**: not tried on Windows yet. The phone connects to port 8650, which Windows Firewall blocks by default. Allow inbound TCP 8650 for the *Private* profile and the local subnet only. Never port-forward it: the phone can approve desktop actions on this PC.
- **Not yet available**: an installer (MSI/NSIS), log files for a build without a terminal, and the colour tray icon. See `docs/windows-port.md`.
