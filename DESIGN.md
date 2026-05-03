# Pocket Agent — 技术方案 v2

> 桌面 AI 助手客户端，以可拖拽电子宠物 Widget 形式常驻桌面，通过 Hermes Agent API 与 LLM 交互，支持 fn 键语音对话（STT + TTS）。

## 1. 项目概述

Pocket Agent 是一个轻量级桌面 Widget 应用，作为 Hermes Agent 的前端界面。与传统聊天窗口不同，它以**电子宠物**的形式常驻桌面——上方是可互动的角色，下方是对话框，可自由拖拽到屏幕任意位置。

**核心功能：**
- 桌面可拖拽 Widget（角色 + 对话框，类电子宠物）
- fn 键按住说话，松开自动识别并获取 AI 回复，语音播出
- 文字对话（流式打字机效果）
- 皮肤系统（角色皮肤 + 对话框皮肤，先简单后扩展）
- 对话历史由 Hermes session 管理，客户端无需存储

**不是什么：**
- 不是独立 AI，没有自己的推理能力
- 不是传统聊天窗口应用
- 不做浏览器功能
- 不是 Electron 臃肿应用

## 2. 技术选型

### 2.1 框架：Tauri 2.0

| 维度 | Tauri 2.0 | Electron | Flutter |
|------|-----------|----------|---------|
| 打包体积 | ~5-10MB | ~150MB+ | ~40MB |
| 内存占用 | ~30-50MB | ~200MB+ | ~80MB |
| 启动速度 | <1s | 2-5s | ~1s |
| 透明窗口 | ✅ | ✅ | ⚠️ 有限 |
| 原生能力 | Rust 直接调 | Node.js | Dart FFI |
| 常驻托盘 | ✅ | ✅ | ⚠️ 需插件 |

**选择 Tauri 的理由：**
- 常驻桌面 Widget 必须轻量，目标 <50MB 内存
- Rust 后端直接调 CGEventTap（fn 键）、cpal（麦克风）、rodio（音频），性能极佳
- 透明无边框窗口原生支持，电子宠物必须

### 2.2 前端：Svelte 5 + TypeScript

| 维度 | Svelte | React | Vue |
|------|--------|-------|-----|
| 包体积 | 最小 | 较大 | 中等 |
| 运行时 | 无（编译时框架） | 有 | 有 |
| 动画/过渡 | 内置，优雅 | 需库 | 需库 |

**选择 Svelte 的理由：**
- 编译时框架，零运行时开销，配合 Tauri 最省资源
- 内置 `fly`/`fade` transition，适合气泡弹出、角色状态切换
- TypeScript 支持完善

### 2.3 语音方案

**TTS（语音合成）：**
- 来源：Hermes Agent 的 `POST /v1/audio/speech`
- 播放：Rust 后端用 `rodio` crate 播放（支持 mp3/ogg/wav）

**STT（语音识别）：**
- 方案：Rust 后端用 `cpal` 采集麦克风音频，发送给 Hermes `POST /v1/audio/transcribe`
- **不用** Web Speech API：Widget 应用窗口随时失焦，浏览器 API 不可靠
- 优点：全链路走 Hermes，无平台依赖，无需 Apple Speech.framework 权限

**fn 键触发：**
- Rust 后端用 `rdev` crate 全局监听键盘事件（底层基于 macOS CGEventTap）
- fn 按下 → 开始录音，fn 松开 → 停止录音并触发识别流程

## 3. Widget UI 设计

### 3.1 窗口配置

```json
// tauri.conf.json
"windows": [{
  "transparent": true,
  "decorations": false,
  "alwaysOnTop": true,
  "skipTaskbar": true,
  "resizable": false,
  "width": 220,
  "height": 360
}]
```

### 3.2 布局结构

```
┌─────────────────────┐  ← 透明窗口（220×360）
│                     │
│   ┌───────────┐     │
│   │  角色区域  │     │  ← 上半部分（~160px）
│   │  (可拖拽)  │     │    角色动画，4 种状态
│   └───────────┘     │
│                     │
│  ┌─────────────────┐│
│  │ ╔═════════════╗ ││
│  │ ║  对话框区域  ║ ││  ← 下半部分（~180px）
│  │ ║  AI 说的话  ║ ││    可换皮肤（气泡/电视机/终端）
│  │ ║  流式打字   ║ ││
│  │ ╚═════════════╝ ││
│  └─────────────────┘│
└─────────────────────┘

        ┌──────────────────┐
        │ 🎤  正在聆听...   │  ← 录音时显示的胶囊浮层
        └──────────────────┘    （类 voice-input 风格）
```

### 3.3 拖拽设计

角色区域整体可拖拽，对话框区域不加拖拽属性（允许文字选择/复制）：

```html
<div data-tauri-drag-region class="character-zone">
  <Character />
</div>
<div class="dialog-zone">
  <DialogBox />
</div>
```

### 3.4 右键菜单

右键角色区域弹出操作菜单（替代独立设置窗口入口）：
- 设置（API 地址、音量、皮肤）
- 清空对话
- 静音 / 取消静音
- 退出

## 4. 皮肤架构

### 4.1 角色皮肤

**Phase 1（当前）**：纯 CSS 几何角色，圆形脑袋 + 简单五官，通过 CSS animation 驱动 4 种状态：

| 状态 | CSS class | 触发时机 |
|------|-----------|---------|
| idle | `.state-idle` | 默认，呼吸 + 随机眨眼 |
| listening | `.state-listening` | fn 按下，竖耳/注意 |
| thinking | `.state-thinking` | fn 松开到回复开始，思考动作 |
| speaking | `.state-speaking` | TTS 播放中，嘴型动画 |

**扩展口**：`Character.svelte` 接受 `skinType: CharacterSkin` prop，
后续加入 Rive/Lottie 皮肤时无需修改外部调用逻辑：

```typescript
type CharacterSkin = "default-css" | "rive" | "lottie" | "custom";
```

### 4.2 对话框皮肤

`DialogBox.svelte` 接受 `dialogStyle` prop，不同皮肤只是 CSS class 或 SVG 边框装饰：

| 皮肤 | 样式 |
|------|------|
| `bubble` | 默认圆角气泡 |
| `tv` | 像素电视机边框 |
| `terminal` | 终端/命令行风格 |

## 5. 架构设计

```
┌─────────────────────────────────────────────────────────┐
│                  Pocket Agent (Desktop)                  │
│                                                         │
│  ┌──────────────────────────────────────────────────┐   │
│  │               Svelte 5 Frontend                  │   │
│  │  ┌──────────────┐    ┌──────────────────────┐   │   │
│  │  │  Character   │    │      DialogBox        │   │   │
│  │  │  .svelte     │    │      .svelte          │   │   │
│  │  │  (拖拽/动画) │    │  (流式文字/皮肤)      │   │   │
│  │  └──────┬───────┘    └──────────┬────────────┘   │   │
│  │         │  listen("fn-key-*")   │ listen("chat-stream") │
│  └─────────┼───────────────────────┼────────────────┘   │
│            │  Tauri Events (单向推送)                    │
│  ┌─────────┼───────────────────────┼────────────────┐   │
│  │         │   Rust Backend        │                │   │
│  │  ┌──────┴──────┐  ┌────────┐  ┌┴───────────┐   │   │
│  │  │  hotkey.rs  │  │record  │  │  chat.rs   │   │   │
│  │  │  (rdev)     │  │.rs     │  │  (SSE emit)│   │   │
│  │  │  fn 键监听  │  │(cpal)  │  └─────┬──────┘   │   │
│  │  └─────────────┘  └───┬────┘        │           │   │
│  │                       │    ┌─────────┴──────┐   │   │
│  │                  ┌────┴────┤  api/client.rs │   │   │
│  │                  │         │  (reqwest)     │   │   │
│  │             音频 buffer    └────────┬───────┘   │   │
│  │                              ┌──────┴──────┐    │   │
│  │                              │  audio.rs   │    │   │
│  │                              │  (rodio)    │    │   │
│  └──────────────────────────────┴─────────────┘    │   │
└──────────────────────────────────────────────────────┘
                              │ HTTP (localhost:8642)
                              ▼
              ┌───────────────────────────┐
              │      Hermes Agent API     │
              │                           │
              │  POST /v1/chat/completions│ ← 对话（SSE 流式）
              │  POST /v1/audio/speech    │ ← TTS
              │  POST /v1/audio/transcribe│ ← STT
              │  GET  /v1/models          │ ← 模型列表
              └─────────────┬─────────────┘
                            │
                            ▼
                    ┌───────────────┐
                    │  LLM Provider │
                    │  (Z.AI/其他)  │
                    └───────────────┘
```

## 6. 项目结构

```
pocket-agent/
├── src-tauri/
│   ├── capabilities/
│   │   └── default.json         # Tauri 2.0 权限声明（必须）
│   ├── Info.plist                # macOS 权限描述文字
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs               # Tauri 入口，启动 fn 键监听
│       ├── commands/
│       │   ├── mod.rs
│       │   ├── chat.rs           # 对话：调 API + SSE emit 到前端
│       │   ├── audio.rs          # 音频播放：rodio
│       │   ├── record.rs         # 录音：cpal 采集麦克风
│       │   ├── hotkey.rs         # fn 键：rdev 全局监听
│       │   └── config.rs         # 设置读写：tauri-plugin-store
│       └── api/
│           ├── mod.rs
│           └── client.rs         # Hermes API 客户端（chat/tts/stt）
├── src/
│   ├── App.svelte                # 主容器（透明背景，组装布局）
│   ├── lib/
│   │   ├── components/
│   │   │   ├── Character.svelte        # 角色（可拖拽，皮肤切换）
│   │   │   ├── DialogBox.svelte        # 对话框（流式文字，皮肤切换）
│   │   │   ├── RecordingCapsule.svelte # 录音状态胶囊浮层
│   │   │   ├── ContextMenu.svelte      # 右键菜单
│   │   │   └── SettingsPanel.svelte    # 设置面板
│   │   └── stores/
│   │       ├── chat.ts           # 当前对话内容（流式追加）
│   │       ├── character.ts      # 角色状态机（idle/listening/thinking/speaking）
│   │       └── settings.ts       # 皮肤、API 地址、音量等配置
│   └── main.ts
├── assets/
│   └── skins/                    # 皮肤资源（预留）
│       └── default/
│           └── character.css     # 默认 CSS 角色样式
├── package.json
├── svelte.config.js
├── vite.config.ts
└── tsconfig.json
```

## 7. 核心模块设计

### 7.1 fn 键监听 (`commands/hotkey.rs`)

```rust
use rdev::{listen, EventType, Key};

pub fn start_fn_key_listener(app_handle: tauri::AppHandle) {
    std::thread::spawn(move || {
        let _ = listen(move |event| match event.event_type {
            EventType::KeyPress(Key::Function) => {
                app_handle.emit("fn-key-down", ()).ok();
            }
            EventType::KeyRelease(Key::Function) => {
                app_handle.emit("fn-key-up", ()).ok();
            }
            _ => {}
        });
    });
}
```

- 独立线程运行，不阻塞主线程
- macOS 首次运行会弹辅助功能权限请求（与 voice-input 项目一致）
- Windows 端 fn 键 keycode 需测试确认

### 7.2 麦克风录音 (`commands/record.rs`)

```rust
// cpal 采集音频，保存到内存 buffer
- start_recording() → 开始采集，Mutex<Vec<f32>> 缓存原始 PCM
- stop_recording()  → 停止采集，编码为 WAV，返回 bytes
```

- 采样率 16000Hz（Whisper 最优输入）
- 单声道，16-bit PCM
- 音频数据不落盘，内存处理后直接发 HTTP

### 7.3 Hermes API 客户端 (`api/client.rs`)

```rust
// 三个核心接口
- POST /v1/audio/transcribe  → STT：上传 wav bytes，返回文字
- POST /v1/chat/completions  → 对话（SSE 流式）：逐块 emit 到前端
- POST /v1/audio/speech      → TTS：返回音频 bytes 给 rodio 播放
```

关键细节：
- SSE 流式用 `eventsource-stream` 解析，每个 delta 通过 `window.emit("chat-stream", delta)` 推送
- 音频上传用 `reqwest` multipart
- API 地址可配置（默认 `http://localhost:8642`），支持 LAN 远程连接

### 7.4 流式输出事件协议

```
Rust → 前端 事件：

"chat-stream"      payload: { delta: "文字片段" }   每个 SSE chunk
"chat-stream-end"  payload: null                    流结束
"fn-key-down"      payload: null                    fn 按下
"fn-key-up"        payload: null                    fn 松开
"stt-result"       payload: { text: "识别文字" }    STT 完成
"tts-start"        payload: null                    开始播放
"tts-end"          payload: null                    播放结束
```

前端 Svelte 用 `listen()` 订阅这些事件，驱动角色状态和对话框内容。

### 7.5 角色状态机 (`stores/character.ts`)

```typescript
type CharacterState = "idle" | "listening" | "thinking" | "speaking";

// 状态转换：
// idle → listening   (fn-key-down)
// listening → thinking (fn-key-up)
// thinking → speaking  (tts-start)
// speaking → idle      (tts-end)
```

### 7.6 设置持久化 (`commands/config.rs`)

使用 `tauri-plugin-store`，存储到 `app_config_dir()/settings.json`：

```json
{
  "api_url": "http://localhost:8642",
  "volume": 0.8,
  "character_skin": "default-css",
  "dialog_style": "bubble",
  "window_x": 1200,
  "window_y": 400
}
```

## 8. 数据流

### 8.1 文字对话

```
用户在对话框输入文字 → Enter 发送
  → invoke("send_message", { text })
    → Rust: POST /v1/chat/completions (stream=true)
      → 逐块解析 SSE
        → window.emit("chat-stream", { delta })
          → 前端 listen → 追加到对话框（打字机效果）
            → SSE 结束 → emit("chat-stream-end")
              → 角色: thinking → idle
```

### 8.2 语音对话（fn 键）

```
用户按住 fn 键
  → rdev 检测 KeyPress(Function)
    → emit("fn-key-down")
      → 前端: 角色 idle → listening
      → Rust: cpal 开始录音（PCM buffer）
      → 前端: 显示录音胶囊「🎤 正在聆听...」

用户松开 fn 键
  → rdev 检测 KeyRelease(Function)
    → emit("fn-key-up")
      → 前端: 角色 listening → thinking
      → 前端: 胶囊改为「💭 正在思考...」
      → Rust: cpal 停止录音
        → 编码为 WAV
          → POST /v1/audio/transcribe
            → 获取识别文字
              → emit("stt-result", { text })
                → 前端: 对话框显示识别文字
                → Rust: POST /v1/chat/completions (stream)
                  → 流式回复 → 对话框打字机效果
                    → 回复完成 → POST /v1/audio/speech
                      → 获取音频
                        → emit("tts-start")
                          → rodio 播放音频
                            → 前端: 角色 thinking → speaking
                              → 播放结束 → emit("tts-end")
                                → 角色 speaking → idle
                                  → 胶囊消失
```

## 9. 依赖清单

### Rust (Cargo.toml)

```toml
[dependencies]
tauri = { version = "2", features = ["tray-icon"] }
tauri-plugin-shell = "2"
tauri-plugin-global-shortcut = "2"
tauri-plugin-store = "2"

reqwest = { version = "0.12", features = ["stream", "json", "multipart"] }
rodio = { version = "0.19", features = ["mp3", "vorbis"] }
rdev = "0.5"
cpal = "0.15"

tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
futures-util = "0.3"
eventsource-stream = "0.2"
hound = "3.5"    # PCM → WAV 编码
```

### Frontend (package.json)

```json
{
  "dependencies": {
    "@tauri-apps/api": "^2"
  },
  "devDependencies": {
    "svelte": "^5",
    "@sveltejs/vite-plugin-svelte": "^4",
    "typescript": "^5",
    "vite": "^6"
  }
}
```

> Phase 3 加 Rive 皮肤时再引入 `@rive-app/canvas`，按需添加。

## 10. 权限清单（macOS）

| 权限 | 用途 | 配置位置 |
|------|------|---------|
| 麦克风 | cpal 录音 | `src-tauri/Info.plist` → `NSMicrophoneUsageDescription` |
| 辅助功能 | rdev fn 键全局监听 | 系统偏好 → 首次运行自动弹请求 |

> **无需** Apple Speech.framework 语音识别权限（STT 走 Hermes API）

### Tauri 2.0 capabilities (`src-tauri/capabilities/default.json`)

```json
{
  "identifier": "default",
  "description": "Default capabilities",
  "platforms": ["macOS", "windows"],
  "permissions": [
    "tauri:default",
    "tray-icon:allow-new",
    "tray-icon:allow-set-icon",
    "global-shortcut:allow-register",
    "global-shortcut:allow-unregister",
    "store:allow-get",
    "store:allow-set",
    "store:allow-save"
  ]
}
```

## 11. 开发计划

### Phase 1：Widget 骨架（2 天）

- [ ] Tauri + Svelte 项目初始化
- [ ] 透明无边框窗口配置（transparent + decorations: false）
- [ ] CSS 几何角色占位（圆形 + 简单五官）
- [ ] 对话框区域（默认气泡样式）
- [ ] 角色区域拖拽（data-tauri-drag-region）
- [ ] 文字输入 + 流式对话（修正为 Tauri Event 推送）
- [ ] Tauri 2.0 capabilities 声明

**验证：** 透明窗口显示在桌面，可拖拽，文字对话流式可用

### Phase 2：fn 键语音链路（2 天）

- [ ] rdev fn 键全局监听（+ 辅助功能权限引导）
- [ ] cpal 麦克风录音（PCM 采集）
- [ ] hound 编码 WAV
- [ ] Hermes STT（/v1/audio/transcribe）
- [ ] Hermes TTS（/v1/audio/speech）+ rodio 播放
- [ ] 录音状态胶囊 UI（fly transition 出入场）
- [ ] 角色 4 状态 CSS 动画

**验证：** 按住 fn → 录音 → 松开 → 识别 → AI 回复流式显示 → 语音播放

### Phase 3：体验打磨（1-2 天）

- [ ] 对话框电视机皮肤
- [ ] 设置面板（API 地址、音量、皮肤选择）
- [ ] 右键菜单
- [ ] 系统托盘（静音/退出入口）
- [ ] 窗口位置记忆（tauri-plugin-store）
- [ ] macOS Info.plist 权限文字

**验证：** 设置关闭后重开仍保留，皮肤切换即时生效

### Phase 4：跨平台（1 天）

- [ ] Windows fn 键 keycode 确认（rdev 在 Windows 的映射）
- [ ] Windows 透明窗口兼容性测试
- [ ] GitHub Actions 自动构建（macOS + Windows）
- [ ] 自动更新（Tauri 内置 updater）

**验证：** macOS + Windows 均可正常使用全部功能

## 12. Hermes Agent 配置要求

Pocket Agent 依赖 Hermes 的 `api_server` 平台。需在 `~/.hermes/config.yaml` 启用：

```yaml
platforms:
  api_server:
    enabled: true
    port: 8642
    host: "0.0.0.0"   # 0.0.0.0 允许 LAN 访问
```

TTS 情绪功能依赖 `tts_tool.py` 的 emotion preset patch（见 `tts-patch` skill）。

## 13. 与 voice-input 的关系

pocket-agent 的 fn 键语音机制参考 voice-input 项目，但有以下区别：

| 维度 | voice-input (Swift) | pocket-agent (Tauri/Rust) |
|------|--------------------|--------------------|
| fn 键监听 | Swift CGEventTap | Rust rdev（底层同样基于 CGEventTap） |
| STT | Apple Speech.framework（本地） | Hermes /v1/audio/transcribe（API） |
| TTS | 无 | Hermes /v1/audio/speech + rodio |
| 结果处理 | 注入到当前光标 | 显示在对话框 + 语音播放 |
| 平台 | macOS 专属 | 跨平台（macOS + Windows） |

两者可以**共存**：voice-input 负责全局任意输入框的语音填充，pocket-agent 负责与 AI 的专属对话。

## 14. 设计原则

1. **轻量优先** — 常驻进程目标 <50MB 内存
2. **Hermes 为后端** — 所有 AI/语音能力来自 Hermes API，客户端只做展示和交互
3. **渐进增强** — 先跑通文字对话，再加语音，再加皮肤
4. **皮肤可扩展** — 现在简单，接口留好，未来换皮不改逻辑
5. **茶哥的代码风格** — 无硬编码路径，配置可调，不留垃圾文件
