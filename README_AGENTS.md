# Pocket Agent — Agent Installation Guide

> This document is for AI agents. Follow these steps to install and run Pocket Agent.

## Overview

Pocket Agent (PA) is a macOS desktop voice companion. It connects to a Hermes agent gateway (the only supported backend; OpenClaw is no longer supported) and provides push-to-talk voice interaction. macOS only — there is no Windows build yet.

- **GitHub**: https://github.com/kevin-ping/pocket-agent
- **Stack**: Tauri 2 + Svelte 5 + Rust
- **Platform**: macOS 12+ (UI built for the macOS 12.0 WebKit; verified only on Apple Silicon, macOS 26; macOS 12–15 and Intel not yet verified)
- **Current version**: 0.3.0

---

## Step 1: Install Prerequisites

Check and install each tool. Skip any that are already present.

### 1a. Xcode Command Line Tools

Verify: `xcode-select -p` prints a path and `/usr/bin/python3 --version` prints a Python from 3.9 to 3.12 (the tools ship 3.9).

If missing: `xcode-select --install`, then wait for the installer to finish before continuing. The tools provide the compiler/SDK for the Rust build and the Python PA builds its voice environment from.

### 1b. Homebrew

Verify: `brew --version`

If missing, install Homebrew using the official method from their GitHub repository.

### 1c. Node.js 18+

Verify: `node --version`

If missing: `brew install node`

### 1d. Rust toolchain

Verify: `rustc --version` and `cargo --version`

If missing, install Rust via rustup using the official installer.

### 1e. Python: nothing to install by hand

Do **not** `pip install` / `pipx install` voice packages. On first launch PA creates `~/.pocket-agent/venv` and installs `src-tauri/resources/requirements-stt.txt` into it, which needs network access to PyPI; the speech models then download from HuggingFace.

PA builds the venv from the first **Python 3.9–3.12** (with the `venv` module) among: `/usr/bin/python3` (Command Line Tools), `python3` on PATH, `/opt/homebrew/bin/python3.{12,11,10}`, `/usr/local/bin/python3.{12,11,10}`, then `/opt/homebrew/bin/python3`, `/usr/local/bin/python3`. With the Command Line Tools from 1a this is already satisfied.

---

## Step 2: Clone and Build

```bash
git clone https://github.com/kevin-ping/pocket-agent.git
cd pocket-agent
npm install
```

Verify build:

```bash
cd src-tauri && cargo check
cd .. && npm run build
```

Both must succeed with zero errors.

---

## Step 3: Configure Environment

Create .env in the project root (for dev mode):

```bash
[ -f .env ] || cp .env.example .env    # never overwrite an existing .env
```

Then edit .env with the correct values for your setup. If a `.env` already exists, keep its gateway and key unless the user asks to change them.

**Required fields:**
- `API_SERVER` — backend gateway URL (e.g. http://localhost:8642 for Hermes)
- `API_SERVER_KEY` — authentication key, must match gateway config (leave empty for no auth)
- `API_AGENT` — **leave it empty**: when set, PA uses `/v1/chat/completions`, which has no stop command, so an interrupted turn keeps running in Hermes.

**Optional fields:**
- `EDGE_TTS_BIN` — path to edge-tts binary (if omitted: PA's venv copy `~/.pocket-agent/venv/bin/edge-tts`, else PATH)
- `STT_MODEL` — Whisper model: `tiny` | `base` | `small` (default `base`)
- `STT_PYTHON` — advanced override; leave unset. It only changes which interpreter runs the voice service (not which Python builds the venv) and must already have everything in `requirements-stt.txt` installed.

**Config file locations (exactly one file is read, never a mix):**
- Development (`npm run tauri dev`): the repository's `.env`, whatever the cwd; if it does not exist, `~/.pocket-agent/.env`
- Packaged app (.app/.dmg): `~/.pocket-agent/.env` only, whatever the cwd
- Variables already set in the launching environment override the file in both builds
- The startup log line `[config] …` names the file used, the gateway URL and whether a key is set (never the key)

---

## Step 4: Set Up the Hermes Gateway

PA needs a running Hermes gateway with its API server enabled. Commands below were checked against Hermes v0.22.5 (`hermes --help`, Hermes' API server docs); check `hermes --help` on the installed version.

1. If `hermes --version` fails, install Hermes: `curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash`, then open a new shell.
2. Configure a model and credentials: `hermes setup` (interactive — ask the user if you cannot answer its questions).
3. Add to `~/.hermes/.env`: `API_SERVER_ENABLED=true` and `API_SERVER_KEY=<random string>` (e.g. `openssl rand -hex 24`).
4. Start the gateway: `hermes gateway run` (foreground) or `hermes gateway install` then `hermes gateway start` (launchd). It listens on `http://127.0.0.1:8642`.
5. Verify a real streamed reply — an open port is not enough:

   ```bash
   curl -N http://127.0.0.1:8642/v1/chat/completions \
     -H "Authorization: Bearer <API_SERVER_KEY>" \
     -H "Content-Type: application/json" \
     -d '{"model": "hermes-agent", "stream": true, "messages": [{"role": "user", "content": "Say hello in one sentence."}]}'
   ```

   Expect `data: {...}` chunks with reply text and a final `data: [DONE]`. Do not continue until this works.
6. In PA's `.env`: `API_SERVER=http://localhost:8642`, `API_SERVER_KEY` equal to the Hermes key, `API_AGENT` unset.

Then read [HERMES.md](HERMES.md) and carry out its setup steps, such as copying PA's skills into the Hermes profile. Its performance tuning and local patches are optional.

---

## Step 5: Run Pocket Agent

From the repository root (where Step 2 left you):

```bash
npm run tauri dev
```

First launch runs, in order: Rust compile → voice venv creation and `pip install` (PyPI) → Whisper and voiceprint model download (HuggingFace) → voice service health check on `127.0.0.1:8651` → start sound. The window appears as soon as the venv install starts; its status panel shows the stage, and any failure with the fix (network/proxy, or `xcode-select --install` when no usable Python exists). Watch the terminal for `[venv]`, `[stt]`, `[stt-server]` lines. Do not assume a fixed total duration.

### macOS Permissions

Grant these in System Settings > Privacy and Security (the user has to click them):

1. **Input Monitoring** — for the terminal app that runs `npm run tauri dev` (Terminal, iTerm2, …); restart that terminal afterwards. Without it the hotkey does nothing in dev mode.
2. **Accessibility** — required for global hotkey capture. Enable for Pocket Agent, then restart the app.
3. **Microphone** — prompted automatically on first recording.

### Verify Everything Works

Check in this order; stop at the first failure.

1. **Text chat** — the PA window is on screen; a typed message gets a Hermes reply
2. **Dictation** — press the hotkey shown in the chat panel (default `RightShift`), speak, press again — the words are transcribed and sent
3. **TTS** — the reply is spoken
4. **Interrupt** — while PA is speaking, press the hotkey: playback stops immediately and recording begins; Escape during recording cancels cleanly
5. *(Optional)* **Wake word / voiceprint** — Settings → Voice
6. *(Optional)* **Computer tools** — [docs/computer-interaction.md](docs/computer-interaction.md)

---

## Troubleshooting

- **No hotkey response in local dev** — Accessibility alone is not enough. The terminal app that launched `npm run tauri dev` (for example iTerm2) also needs **Input Monitoring** permission. Restart the terminal after granting it.
- **No hotkey response only after changing to a modifier** — ensure you are on v0.2.4+, which fixes the post-capture release event swallowing the first real press.
- **fn works over SSH but double-triggers** — ensure you are on v0.2.4+, which suppresses the duplicate SSH `KEY_DOWN` after the `FLAGS_CHANGED` press path.
- **PA keeps speaking after hotkey press** — ensure you are on v0.2.4+, which synchronously interrupts the active rodio sink instead of waiting for the playback thread queue.
- **Voice setup failed / STT fails** — read the error in PA's status panel and the `[venv]` / `[stt-server]` log lines. "无法从 PyPI 下载依赖": check network or proxy to PyPI, then restart PA (the install continues). Other pip errors: the `[venv:installing-deps!]` lines. "No usable Python": `xcode-select --install`, or a Python 3.12 if the installed ones are outside 3.9–3.12. A half-created venv (no pip, missing core modules, or an interpreter that no longer runs) is repaired automatically on the next launch, even when a `.venv-ready` marker was left next to it. To rebuild the voice environment by hand, quit PA and delete only `~/.pocket-agent/venv` and `~/.pocket-agent/.venv-ready` (keep `.env`, the databases, voiceprints and avatars). If `STT_PYTHON` is set, unset it unless that interpreter has all of `requirements-stt.txt`.
- **Recording fails** — "找不到麦克风": connect one or pick an input in System Settings → Sound → Input, then press the hotkey again. "麦克风权限被拒绝": allow PA (dev mode: the terminal) in Privacy & Security → Microphone, then restart PA.
- **No voice output** — spoken replies use Microsoft's online TTS service, so check the network first; transcription working offline says nothing about TTS. edge-tts in PATH? EDGE_TTS_BIN correct? A WebSocket **403** in the `[TTS]` log means the edge-tts that ran is outdated: `<that python> -m pip install -U edge-tts` for that copy.
- **`npm run tauri dev` stops with "port 1420 is in use"** — another process holds the dev port. Nothing is stopped automatically, not even an old vite from this repo: the script reports PID + command and exits non-zero. Check with `lsof -iTCP:1420 -sTCP:LISTEN`, stop that process yourself (`kill <PID>`), then rerun. "could not check whether port 1420 is free" means the check itself failed (e.g. `lsof`); check by hand.
- **Cannot connect to backend** — the chat error names the gateway address and the config source. Gateway running? API_SERVER and API_SERVER_KEY match `~/.hermes/.env`? Check the `[config]` startup line for which file (or terminal variable) was used.
- **Build fails** — cargo check and npm run build both pass? Rust and Node versions current?

---

## FAQ: local vs SSH hotkey behavior

### Local `tauri dev` needs more than Accessibility
When PA is started from a terminal app, macOS attributes the keyboard-monitoring path to that terminal. Grant **Input Monitoring** to iTerm2 / Terminal.app / your launcher shell or local hotkeys may appear dead even though SSH launches work.

### `fn` does not have one stable macOS keycode
On the same Mac, `fn` may surface as `FLAGS_CHANGED` keycode `63` or `KEY_DOWN` keycode `179` depending on launch/session context. v0.2.4 normalizes both to canonical `179` and handles both event shapes.

### SSH launches can emit duplicate events
In some SSH-launched runs, one physical modifier press produces both `FLAGS_CHANGED` and an immediate `KEY_DOWN`. v0.2.4 suppresses the duplicate `KEY_DOWN` so recording does not start and stop on the same press.

### Interrupting speech must be synchronous
If Stop is only queued onto the audio thread, `rodio::Sink::sleep_until_end()` can keep the current TTS clip playing. v0.2.4 stores the current sink and calls `sink.stop()` synchronously when the hotkey starts a new recording.

---

## Architecture Summary

### On-demand Mac interaction

Install Hermes, CuaDriver and Pocket Agent on each Mac. See [computer interaction setup](docs/computer-interaction.md) for per-profile `api_server` tool enablement, capability checks, macOS permissions, auxiliary vision, and the approval/stop protocol. A remote gateway does not automatically control the Pocket Agent client's Mac.

```
PA (Tauri + Svelte)
  ├── Hotkey (CGEventTap, global)
  ├── Recording (cpal → WAV)
  ├── STT (faster-whisper, local)
  ├── Chat (HTTP/SSE to gateway)
  ├── TTS (edge-tts → rodio playback)
  └── Push Server (port 8650)
       ↓
  Hermes (:8642)
```
