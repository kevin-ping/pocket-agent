# Pocket Agent — Agent Installation Guide

> This document is for AI agents. Follow these steps to install and run Pocket Agent.

## Overview

Pocket Agent (PA) is a macOS desktop voice companion. It connects to an AI agent gateway (Hermes or OpenClaw) and provides push-to-talk voice interaction.

- **GitHub**: https://github.com/kevin-ping/pocket-agent
- **Stack**: Tauri 2 + Svelte 5 + Rust
- **Platform**: macOS 12+ (Intel and Apple Silicon)
- **Current version**: 0.2.3

---

## Step 1: Install Prerequisites

Check and install each tool. Skip any that are already present.

### 1a. Homebrew

Verify: `brew --version`

If missing, install Homebrew using the official method from their GitHub repository.

### 1b. Node.js 18+

Verify: `node --version`

If missing: `brew install node`

### 1c. Rust toolchain

Verify: `rustc --version` and `cargo --version`

If missing, install Rust via rustup using the official installer.

### 1d. Python 3.10+

Verify: `python3 --version`

If missing: `brew install python@3.11`

### 1e. Python packages

Install both packages:

```bash
pipx install edge-tts
pip install faster-whisper
```

If pipx is missing: `brew install pipx && pipx ensurepath`

Verify: `which edge-tts` and `python3 -c "import faster_whisper"`

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
cp .env.example .env
```

Then edit .env with the correct values for your setup.

**Required fields:**
- `API_SERVER` — backend gateway URL (e.g. http://localhost:8642 for Hermes)
- `API_SERVER_KEY` — authentication key, must match gateway config (leave empty for no auth)
- `API_AGENT` — your agent name (e.g. my-agent, agent-2)

**Optional fields:**
- `EDGE_TTS_BIN` — path to edge-tts binary (auto-detected from PATH if omitted)
- `STT_PYTHON` — path to python3 with faster-whisper (auto-detected if omitted)
- `ENABLE_LOCAL_COMMANDS` — set to `true` to enable [CMD:...] local command execution

**Config file locations:**
- Development (tauri dev): project root `.env`
- Packaged app (.dmg): `~/.pocket-agent/.env`

Use `which edge-tts` and `which python3` to find binary paths if needed.

---

## Step 4: Start Backend Gateway

PA needs a running AI gateway. Start the appropriate one before launching PA.

**Hermes Agent** — default port 8642
- Start: `hermes gateway` or `hermes serve`
- Verify gateway is responding on its health endpoint

**OpenClaw** — default port 18789
- Start: `openclaw serve`
- Verify gateway is responding on its health endpoint

Ensure `API_SERVER` in .env matches the running gateway URL.

---

## Step 5: Run Pocket Agent

```bash
cd pocket-agent
npm run tauri dev
```

First build compiles Rust (~3-5 min). Subsequent starts take ~20s.

### macOS Permissions

On first launch, grant these in System Settings > Privacy and Security:

1. **Accessibility** — required for global hotkey capture. Enable for Pocket Agent, then restart the app.
2. **Microphone** — prompted automatically on first recording.

### Verify Everything Works

1. PA window appears on screen
2. Press the hotkey (default: fn key) — recording indicator appears
3. Speak, release hotkey — STT transcribes, LLM responds with voice
4. Settings panel opens from menu bar tray icon

---

## Troubleshooting

- **No hotkey response** — Accessibility permission granted? Restart PA after enabling.
- **STT fails** — faster-whisper installed? STT_PYTHON path correct?
- **No voice output** — edge-tts in PATH? EDGE_TTS_BIN correct?
- **Cannot connect to backend** — Gateway running? API_SERVER and API_SERVER_KEY match?
- **Build fails** — cargo check and npm run build both pass? Rust and Node versions current?

---

## Architecture Summary

```
PA (Tauri + Svelte)
  ├── Hotkey (CGEventTap, global)
  ├── Recording (cpal → WAV)
  ├── STT (faster-whisper, local)
  ├── Chat (HTTP/SSE to gateway)
  ├── TTS (edge-tts → rodio playback)
  └── Push Server (port 8650)
       ↓
  Hermes (:8642) or OpenClaw (:18789)
```
