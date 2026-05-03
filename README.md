# Pocket Agent

> A minimal desktop AI voice companion that lives on your screen — press a key, speak, get things done.

Pocket Agent is a compact desktop widget built with **Tauri 2.0 + Svelte 5 + Rust**. It connects to a local [Hermes Agent](https://github.com/nousresearch/hermes) gateway via SSE streaming, providing real-time voice conversations with an LLM. Think of it as a desktop pet that actually helps.

**English Demo:**

https://github.com/user-attachments/assets/bitcion_en.mp4

**Chinese Demo:**

https://github.com/user-attachments/assets/bitcoin_cn.mp4

---

## How It Works

```
+-------------------------------------------------------------+
|                      Pocket Agent                            |
|  +----------+  +----------+  +---------------------------+  |
|  |  Avatar   |  |  Chat    |  |  Dynamic Island           |  |
|  |  Widget   |  |  Panel   |  |  (recording indicator)    |  |
|  +----------+  +----------+  +---------------------------+  |
|       |             |                                        |
|  Svelte 5      Svelte 5                                     |
|  (frontend)    (frontend)                                   |
|       |             |                                        |
|  +----+-------------+------------------------------------+  |
|  |              Tauri 2.0 Bridge                          |  |
|  |         (invoke / events / state)                      |  |
|  +----+-------------+------------------------------------+  |
|       |             |                                        |
|  +----+------+  +---+-----------+                            |
|  |  Voice    |  |    Chat       |                            |
|  |  Pipeline |  |    Engine     |                            |
|  |           |  |               |                            |
|  | - hotkey  |  | - SSE stream  |                            |
|  | - record  |  | - TTS play    |                            |
|  | - STT     |  | - lang detect |                            |
|  +----+------+  +---+-----------+                            |
+-------+-------------+----------------------------------------+
        |             |
   WAV audio     HTTP/SSE
        |             |
        v             v
  +----------+   +--------------+
  | Whisper  |   | Hermes Agent |---- LLM (GLM-5)
  | (local)  |   | Gateway      |---- Tools (browser, search...)
  +----------+   | :8642        |---- Session memory
                 +--------------+
```

### Voice Pipeline

1. **Press fn key** — macOS CGEventTap captures the global hotkey
2. **Recording starts** — cpal captures audio via CoreAudio (pre-warmed for zero-latency)
3. **Press fn again** — recording stops, WAV saved to temp file
4. **STT** — faster-whisper transcribes locally (auto language detection)
5. **Send to LLM** — text + voice hint streamed to Hermes gateway via SSE
6. **TTS playback** — edge-tts generates audio, rodio plays it via system speakers

Press **Escape** during recording to cancel. Minimum recording: 1.5s. Maximum: 30s (auto-cutoff).

---

## Features

- **Voice-first interaction** — push-to-talk with local Whisper STT, no cloud dependency for speech recognition
- **Real-time streaming** — SSE streaming from LLM with live text display
- **TTS voice response** — edge-tts with automatic language detection (Chinese, English, Japanese, Korean + more)
- **Session memory** — conversation context persists across interactions via Hermes gateway
- **Local command execution** — LLM can trigger `[CMD:...]` for local automation tasks
- **Multi-language voice** — configure primary + auxiliary TTS voices, auto-switch based on detected language
- **Compact widget** — 220x360px always-on-top window, dark sci-fi aesthetic
- **macOS native** — global hotkey via CGEventTap, CoreAudio recording, menu bar tray

---

## Tech Stack

**Frontend (Svelte 5)**
- Svelte 5 with runes (`$state`, `$derived`, `$effect`, `$props()`)
- TypeScript
- Vite 6

**Desktop (Tauri 2.0)**
- Rust backend with Tokio async runtime
- reqwest for HTTP/SSE streaming
- eventsource-stream for SSE parsing
- cpal + hound for audio recording
- rodio for audio playback
- CGEventTap (macOS FFI) for global hotkey

**AI / Voice**
- [Hermes Agent](https://github.com/nousresearch/hermes) gateway (local, `localhost:8642`)
- [faster-whisper](https://github.com/SYSTRAN/faster-whisper) for local speech-to-text
- [edge-tts](https://github.com/rany2/edge-tts) for text-to-speech
- Any OpenAI-compatible LLM via Hermes (tested with GLM-5)

---

## Backend Compatibility

| Backend | Status | Notes |
|---------|--------|-------|
| **Hermes Agent** | Supported | Primary backend. Requires gateway running on `localhost:8642` |
| **OpenClaw** | In development | Multi-agent orchestration support coming soon |

Pocket Agent communicates with the backend via a simple OpenAI-compatible chat completions API (`/v1/chat/completions`) over SSE. Any server implementing this interface can be used as a drop-in replacement.

---

## Privacy and Security

Pocket Agent connects to a **local gateway** on your machine. Be aware of the following:

- **Local network access** — the gateway binds to `localhost:8642`. Any process on your machine can potentially access it if the API key is exposed.
- **API key in .env** — your `API_SERVER_KEY` is stored in plaintext. Never commit `.env` to version control. The provided `.gitignore` excludes it by default.
- **Global hotkey access** — the fn key listener uses macOS Accessibility APIs (CGEventTap). This grants the app system-level input monitoring capability. Only run builds you trust.
- **Microphone access** — audio is captured via CoreAudio and processed **entirely locally** by faster-whisper. No audio data leaves your machine for STT.
- **Conversation history** — sessions are stored in `~/.hermes/sessions/` by the Hermes gateway. These contain full conversation text including LLM tool call results. Consider disk encryption.
- **Local command execution** — the `[CMD:...]` feature allows the LLM to execute shell commands on your machine. This is powerful but dangerous. Audit your LLM behavior before enabling tool-use heavy workflows.

---

## Getting Started

### Prerequisites

- **macOS** 12+ (Monterey or later)
- **Rust** 1.70+ — [install](https://rustup.rs/)
- **Node.js** 18+ and npm
- **Python 3.10+** with `faster-whisper` and `edge-tts`
- **Hermes Agent** gateway running on `localhost:8642`

### Setup

1. **Clone and install dependencies:**

```bash
git clone https://github.com/kevin-ping/pocket-agent.git
cd pocket-agent
npm install
```

2. **Configure environment:**

```bash
cp .env.example .env
```

Edit `.env` with your values:

```bash
# From ~/.hermes/config.yaml -> api_server.key
API_SERVER_KEY=your-api-server-key-here

# Path to edge-tts binary (pip install edge-tts)
EDGE_TTS_BIN=/path/to/edge-tts

# Hermes session ID for conversation context
SESSION_ID=pocket-agent-session

# Python with faster_whisper installed
STT_PYTHON=/path/to/python3
```

3. **Run in development mode:**

```bash
npm run tauri dev
```

First build takes 3-5 minutes for Rust compilation. Subsequent builds are ~20 seconds.

### macOS Permissions

On first launch, grant these permissions in **System Settings -> Privacy and Security**:

1. **Accessibility** — required for global fn key capture. Add Pocket Agent, then **restart the app**.
2. **Microphone** — prompted automatically on first recording.
3. **Speech Recognition** — prompted automatically (only if using Apple SFSpeech; faster-whisper bypasses this).

---

## Configuration

Open **Settings** from the menu bar tray icon. Available options:

- **API URL** — Hermes gateway address (default: `http://localhost:8642`)
- **Avatar image** — upload a custom character avatar
- **TTS voices** — primary, auxiliary 1, auxiliary 2 (grouped by language)
- **Fixed language mode** — force LLM to always respond in a specific language
- **Mute/unmute** — toggle TTS playback

Settings persist across restarts via Tauri plugin-store.

---

## Airline Enquiry Demo

Pocket Agent can search the web via Hermes tools. Here is an example of asking for cheap flights:

**Query:**

![Airline Enquiry](assets/media/airline_enqiry.jpg)

**Result:**

![Airline Result](assets/media/airline_result.jpg)

---

## Project Structure

```
pocket-agent/
├── src/                          # Svelte 5 frontend
│   ├── App.svelte                # Main container + event orchestration
│   ├── main.ts                   # Entry point
│   └── lib/
│       ├── components/
│       │   ├── AvatarIcon.svelte  # Character avatar + expand button
│       │   ├── ChatPanel.svelte   # Chat input + message display
│       │   ├── ContextMenu.svelte # Right-click context menu
│       │   ├── DialogBox.svelte   # Dialog bubble with typewriter effect
│       │   ├── DynamicIsland.svelte # Recording indicator
│       │   ├── Icon.svelte       # SVG inline icon component (Lucide style)
│       │   ├── RecordingCapsule.svelte # Active recording timer
│       │   └── SettingsPanel.svelte # Tabbed settings (General / Voice)
│       ├── stores/
│       │   ├── chat.ts           # Chat message store
│       │   ├── character.ts      # Character animation state
│       │   ├── layout.ts         # Window layout constants
│       │   └── settings.ts       # Persistent settings store
│       └── i18n.ts               # Language detection utilities
├── src-tauri/                    # Rust backend
│   ├── Cargo.toml                # Rust dependencies
│   ├── tauri.conf.json           # Tauri window + tray config
│   ├── resources/
│   │   └── stt-helper            # Python STT script (faster-whisper)
│   └── src/
│       ├── main.rs               # App entry
│       ├── lib.rs                # State, tray menu, plugin setup
│       ├── api/
│       │   └── client.rs         # Hermes SSE client
│       ├── commands/
│       │   ├── chat.rs           # send_message: SSE -> TTS -> emit
│       │   ├── config.rs         # Settings persistence + voice hints
│       │   └── voice.rs          # Recording lifecycle commands
│       └── voice/
│           ├── hotkey.rs         # Global fn-key capture (CGEventTap)
│           ├── record.rs         # Audio recording (cpal) + pre-warm
│           └── stt.rs            # Whisper transcription wrapper
├── assets/
│   └── media/                    # Demo videos + screenshots
├── .env.example                  # Environment template
└── README.md
```

---

## Development

```bash
# Type check frontend
npx svelte-check

# Check Rust compilation
cd src-tauri && cargo check

# Build for production
npm run tauri build
```

---

## License

MIT
