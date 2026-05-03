# Pocket Agent

桌面 AI 助手 — 电子宠物 Widget 形态，常驻桌面，通过 Hermes Agent API 对话，支持 fn 键语音输入。

## 快速开始

```bash
cd /var/www/pocket-agent
npm run tauri dev
```

首次编译 Rust 依赖约 3-5 分钟，之后增量编译约 20 秒。

## 架构

```
Pocket Agent (Tauri 2.0 + Svelte 5)
  → Rust: Hermes API 客户端 (reqwest + SSE 流式)
    → Hermes Agent API (localhost:8642)
      → LLM (GLM-5 via Z.AI)
```

## 核心功能

- **文字对话**：SSE 流式打字机效果
- **fn 键语音输入**：按住 fn 录音，松开自动 STT → 发给 agent → TTS 播放回复
- **Session 记忆**：固定 session ID，对话跨回合保持上下文
- **TTS 语音**：edge-tts (zh-CN-XiaoxiaoNeural) + rodio 播放
- **皮肤系统**：CSS 角色皮肤 + 对话框皮肤（bubble/tv/terminal）

## 语音输入（fn 键）

技术栈与 voice-input 相同：

| 组件 | 实现 |
|------|------|
| fn 键捕获 | rdev（底层 CGEventTap，需 Accessibility 权限） |
| 麦克风录音 | cpal + hound（底层 CoreAudio） |
| 语音识别 STT | Swift helper CLI（SFSpeechRecognizer，离线可用） |

首次使用前需构建 Swift helper（在开发机执行一次即可）：

```bash
bash src-tauri/swift-stt/build.sh
```

首次启动需授权三项权限：
1. **辅助功能（Accessibility）** — 系统设置 → 隐私与安全性 → 辅助功能，添加 Pocket Agent 后**重启 app**
2. **麦克风** — 系统自动弹窗，点允许
3. **语音识别** — 系统自动弹窗，点允许

## 配置

- API 地址：`http://localhost:8642`（可在设置里改）
- Session：自动维持，通过 `X-Hermes-Session-Id` header
- 认证：携带 `Authorization: Bearer <API_SERVER_KEY>`（硬编码于源码）

## 项目结构

```
pocket-agent/
├── src-tauri/
│   ├── swift-stt/
│   │   ├── main.swift       # Swift STT helper CLI (SFSpeechRecognizer)
│   │   └── build.sh         # 构建脚本 → resources/stt-helper
│   ├── resources/
│   │   └── stt-helper       # 编译后的 STT 二进制（需先运行 build.sh）
│   └── src/
│       ├── lib.rs            # AppState + 窗口初始化 + 热键线程启动
│       ├── api/client.rs     # Hermes API 客户端 (chat_stream)
│       ├── commands/
│       │   ├── chat.rs       # send_message: API → TTS → SSE emit
│       │   ├── config.rs     # 设置持久化 + API key
│       │   └── voice.rs      # start/stop_voice_recording Tauri commands
│       └── voice/
│           ├── hotkey.rs     # rdev 全局热键监听（独立 OS 线程）
│           ├── record.rs     # cpal 录音（独立录音线程，mpsc channel 通信）
│           └── stt.rs        # 调用 stt-helper CLI，返回识别文字
├── src/
│   ├── App.svelte            # 主容器 + 事件监听
│   └── lib/components/
│       ├── Character.svelte  # 角色动画 (idle/listening/thinking/speaking)
│       └── DialogBox.svelte  # 对话框 (打字机效果)
└── DESIGN.md                 # 技术方案 v2
```

## 开发

```bash
# 编译检查
cd src-tauri && cargo check 2>&1

# 查看编译错误
cargo build 2>&1

# 检查进程
ps aux | grep pocket-agent
```
