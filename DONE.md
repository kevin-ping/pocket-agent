# DONE — Pocket Agent 完成记录

## 语音输入实现 (2026-05-02)

**功能**：按住 fn 键录音，松开后 STT 转文字，自动发给 agent，agent 回复 + TTS 播放。

**技术方案**：与 /var/www/voice-input 相同技术栈（CGEventTap + CoreAudio + SFSpeechRecognizer），从 Swift 移植到 Rust + Swift Helper 混合架构。

**新增文件**：
- `src-tauri/swift-stt/main.swift`：Swift CLI，接收 WAV 路径，调用 `SFSpeechURLRecognitionRequest`，识别结果输出到 stdout
- `src-tauri/swift-stt/build.sh`：编译脚本，产物放到 `resources/stt-helper`
- `src-tauri/src/voice/hotkey.rs`：rdev 全局热键监听，独立 OS 线程，`catch_unwind` 保护防止权限缺失时 panic 崩溃进程
- `src-tauri/src/voice/record.rs`：cpal 录音，独立录音线程持有 `cpal::Stream`，用 `mpsc::channel` 通信（避免 `unsafe impl Send` + 跨线程 drop 的 UB）
- `src-tauri/src/voice/stt.rs`：调用 `stt-helper` CLI，返回识别文字
- `src-tauri/src/commands/voice.rs`：`start_voice_recording` / `stop_voice_recording` Tauri commands，阻塞调用包进 `spawn_blocking`

**修改文件**：
- `Cargo.toml`：启用 `rdev = "0.5"`, `cpal = "0.15"`, `hound = "3.5"`
- `src-tauri/src/lib.rs`：注册 `RecordingState`，setup 中启动热键线程
- `src-tauri/src/commands/mod.rs`：加入 `voice` 模块
- `src-tauri/Info.plist`：加入 `NSSpeechRecognitionUsageDescription`
- `src-tauri/tauri.conf.json`：bundle.resources 加入 `stt-helper`
- `src/App.svelte`：fn-key-down/up 回调加录音 invoke，stt-result 改为自动 `handleSendMessage`

**崩溃修复**（首版有 bug）：
- 原因：`unsafe impl Send for RecordingHandle` 让 `cpal::Stream`（`!Send`）被 tokio 线程 drop，触发 UB 导致 macOS 强制终止进程
- 修复：录音线程独立，`Stream` 从创建到 drop 全程在同一线程；`RecordingHandle` 改为只持有 `mpsc::Sender/Receiver`（自动 `Send`）

---

## Session 记忆修复 (2026-05-02)

**问题**：每次对话都创建新 session，Pocket Agent 无法记住上下文。

**根本原因**：两个 bug 叠加：
1. `client.rs` 把 session_id 放进了 HTTP body JSON，但 Hermes api_server 只认 `X-Hermes-Session-Id` HTTP header
2. `~/.hermes/.env` 缺少 `API_SERVER_KEY`，api_server 的 session continuity 功能返回 403

**修复**：
- `src-tauri/src/api/client.rs`：session_id 从 body 移到 `X-Hermes-Session-Id` header；加上 `Authorization: Bearer <key>` header
- `src-tauri/src/commands/config.rs`：硬编码 `API_SERVER_KEY` 常量
- `~/.hermes/.env`：添加 `API_SERVER_KEY=9ad8fac9...`

---

## Pocket Agent 项目进度

### Phase 1：Widget 骨架 ✅
- [x] Tauri + Svelte 项目初始化
- [x] 透明无边框窗口配置
- [x] CSS 几何角色（idle/listening/thinking/speaking 四状态）
- [x] 对话框区域（流式打字机效果）
- [x] 角色区域拖拽（`startDragging()` 方案）
- [x] 文字输入 + 流式对话
- [x] Tauri 2.0 capabilities 声明
- [x] Session 记忆维持 ✅ 2026-05-02

### Phase 2：fn 键语音链路 ✅
- [x] TTS：edge-tts + rodio 播放
- [x] 录音胶囊 UI（fly transition 出入场）
- [x] 情绪检测 + 打字机速度映射
- [x] fn 键捕获：rdev + CGEventTap ✅ 2026-05-02
- [x] 麦克风录音：cpal + hound ✅ 2026-05-02
- [x] STT：Swift helper + SFSpeechRecognizer ✅ 2026-05-02

### Phase 3：体验打磨 🚧 进行中
- [ ] 皮肤系统
- [ ] 电视机/终端对话框皮肤
- [ ] 设置面板优化
- [ ] 右键菜单完善
- [ ] 系统托盘

---

## 历史 Bug 修复记录

| 日期 | Bug | 修复方案 |
|------|-----|---------|
| 2026-05-02 | fn 键触发 app 强制退出 | 录音线程独立持有 Stream，移除 unsafe impl Send |
| 2026-05-02 | session_id 放错位置（body 而非 header） | 改为 X-Hermes-Session-Id header |
| 2026-05-02 | API_SERVER_KEY 缺失 | 硬编码进 config.rs + 添加到 .env |
| 2026-05-02 | Bearer 认证缺失 | client.rs 加 Authorization header |
| 2026-04 | TTS 声音从系统层面播放而非 App 内 | 改用 rodio Rust 端播放，绕过 WebView |
| 2026-04 | `asset:default` 权限在 Tauri 2.0 不存在 | 放弃 convertFileSrc，改用 rodio |
| 2026-04 | macOS 透明窗口白屏 | 加 `macos-private-api` feature |
| 2026-04 | Drag handler 吞掉 select 元素 | mousedown handler 排除列表加 `select` |
| 2026-04 | Session ID 每次新生成 | AppState + State<> 传固定 session_id |
