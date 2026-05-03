# Pocket Agent

A lightweight desktop AI voice companion. Press fn to talk, release to send. AI responds with voice.

## Architecture

```
User speaks
  -> fn key trigger (CGEventTap)
  -> cpal recording (WAV)
  -> Whisper STT with auto language detection
  -> LLM backend (OpenAI-compatible API)
  -> Streaming text response
  -> edge-tts voice generation + rodio playback
  -> Svelte 5 UI shows text in sync
```

Pocket Agent is a fully standalone desktop app. It communicates with the LLM backend via standard OpenAI-compatible API (`/v1/chat/completions` SSE streaming). It does not depend on any specific backend - any OpenAI-compatible API server works.

## Dependencies

### Compiled into the app
- Hotkey listener, audio recording, playback, UI - all bundled in the .app
- Build requires: Rust toolchain + Node.js + npm

### External Python tools (called via shell)
- `stt-helper` - Speech-to-text using faster-whisper (small model)
- `edge-tts` - Text-to-speech using Microsoft Edge TTS service (requires internet)

### LLM Backend
- Any OpenAI-compatible API server (e.g. Hermes Agent gateway, LocalAI, Ollama with OpenAI compatibility)
- Default: `http://localhost:8642`

## Quick Start

### 1. Install build tools

```bash
# Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"

# Node.js
brew install node

# Python 3.12 (not 3.14 - onnxruntime has no 3.14 wheel)
brew install python@3.12
```

### 2. Install Python dependencies

```bash
pip3.12 install faster-whisper edge-tts
```

Whisper will automatically download the `small` model (~500MB) on first STT run.

### 3. Build

```bash
cd pocket-agent
npm install
npm run tauri dev      # development
npm run tauri build    # production -> src-tauri/target/release/bundle/macos/Pocket Agent.app
```

### 4. Start the LLM backend

Make sure your OpenAI-compatible API server is running on the configured port (default: `localhost:8642`).

```bash
# Verify it is running
curl http://localhost:8642/v1/models
```

### 5. macOS Permissions

On first launch, macOS will prompt for **Accessibility** permission.
This is required for fn key monitoring: System Preferences -> Privacy and Security -> Accessibility -> enable Pocket Agent.

### 6. Usage

- Press **fn** to start recording, press again to stop and send
- Press **Escape** during recording to cancel
- AI responds with voice + text display
- Click the gear icon to open settings

## Settings

| Setting | Description |
|---------|-------------|
| API URL | Backend server address, default `http://localhost:8642` |
| Volume | TTS playback volume |
| Dialog style | Bubble / TV / Terminal |
| Audio format | WAV (lossless, zero latency) / MP3 (smaller) |
| Primary voice | Default TTS voice for responses |
| Aux language 1 | Optional - auto-matched when responding in this language |
| Aux language 2 | Optional - auto-matched when responding in this language |

## Multi-language Voice System

Three-layer language coordination:

1. **Whisper STT** - Auto-detects spoken language (no hardcoding), returns text + language code
2. **LLM** - System prompt explicitly states the detected user language, LLM follows accordingly
3. **TTS** - Detects language from the **first 80 characters** of the response, selects the matching voice

Supported languages: Chinese, Japanese, English, Korean, Cantonese, Taiwanese, French, German, Spanish.

Chinese voices (e.g. XiaoxiaoNeural) have built-in English pronunciation - mixed Chinese-English works naturally.

## Deploying for Other Users

### Same Mac

```bash
# 1. Copy the .app
cp -r src-tauri/target/release/bundle/macos/Pocket\ Agent.app /Users/<user>/Desktop/

# 2. Install Python deps for that user
sudo -u <user> pip3.12 install faster-whisper edge-tts

# 3. Make sure the LLM backend is running (shared)

# 4. First launch -> macOS prompts for Accessibility permission -> allow

# 5. Configure API URL and key in settings
```

### Remote Machine

1. Set API URL to `http://<IP>:8642` in PA settings
2. Backend must listen on `0.0.0.0` (not `127.0.0.1`)
3. Open firewall for port 8642
4. Remote machine needs Python 3.12 + faster-whisper + edge-tts

## Tech Stack

- **Backend**: Rust (Tauri 2.0)
- **Frontend**: Svelte 5 (runes mode)
- **Hotkey**: macOS CGEventTap FFI
- **Recording**: cpal
- **STT**: faster-whisper (Python, small model)
- **TTS**: edge-tts (Python)
- **Playback**: rodio
- **LLM API**: OpenAI-compatible SSE streaming
- **Window**: Tauri 220x360 fixed size

## Project Structure

```
pocket-agent/
+-- src/                          # Svelte 5 frontend
|   +-- App.svelte                # Main app, event hub
|   +-- lib/
|       +-- components/
|       |   +-- Character.svelte      # Character UI
|       |   +-- DialogBox.svelte      # Dialog display
|       |   +-- RecordingCapsule.svelte # Recording indicator
|       |   +-- SettingsPanel.svelte  # Settings panel (General / Voice tabs)
|       +-- stores/
|           +-- chat.ts              # Chat state + typewriter effect
|           +-- character.ts         # Character state
|           +-- settings.ts          # Settings persistence
+-- src-tauri/
|   +-- src/
|   |   +-- lib.rs                # App entry, command registration
|   |   +-- api/
|   |   |   +-- client.rs         # HTTP client, SSE stream parsing
|   |   +-- voice/
|   |   |   +-- hotkey.rs         # fn key listener + Escape cancel
|   |   |   +-- record.rs         # cpal recording + pre-start mechanism
|   |   |   +-- stt.rs            # Calls stt-helper, parses JSON result
|   |   +-- commands/
|   |   |   +-- chat.rs           # Chat + TTS + emotion detection + local commands
|   |   |   +-- config.rs         # Settings + dynamic system prompt
|   |   |   +-- voice.rs          # Recording control + timeout + cancel
|   |   +-- AppState.rs           # Global state
|   +-- resources/
|   |   +-- stt-helper            # Python Whisper STT script
|   +-- Cargo.toml
+-- INTRO.md
```
