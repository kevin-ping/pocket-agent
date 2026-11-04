# 新机器安装：R1–R7 修复与 A01–A20 验证记录

> 本文件前半部分为 Claude 的交付记录；“Codex 独立复核”指出 R3、R5 各有一个恢复/取消缺口；最后一节“R3 / R5 残留修复”记录这两个缺口的修复与验证。下方 R3、R5、A06、A09 四行已按该节更新，其余行保持第一轮结论。

日期：2026-11-04
依据：[fresh-install-requirements.md](fresh-install-requirements.md)
代码：基线 `f33272a` 加未提交改动（本批未提交，未打 tag）。未提交改动里还包括前两批 BUG-INST5 / BUG-DEV1 / BUG-DEV2，本批已保留。

## 验证环境

| 项 | 值 |
|---|---|
| 机器 | Apple Silicon（arm64）MacBook Air，**已有开发依赖的开发机，不是全新 Mac** |
| 系统 | macOS 26.6.2（25G83） |
| Python | `/usr/bin/python3` 3.9.6（Command Line Tools） |
| Node / npm | 22.22.2 / 10.9.7 |
| Rust / cargo | 1.95.0 / 1.95.0 |
| Hermes | v0.22.5（本批没有向它发送请求） |
| 新构建产物 | `npm run tauri build` → `Pocket Agent.app` + `Pocket Agent_0.3.0_aarch64.dmg`（`LSMinimumSystemVersion` 12.0） |

测试分三类，下文逐项标注：
- **隔离测试**：真实程序，`HOME` 指向临时目录，从与仓库无关的目录启动。开发机已经装好的系统依赖（CLT、Rust、Node）仍然存在，所以不能算“从未装过依赖”。
- **模拟测试**：故障注入、假模块或单元测试。
- **真实硬件测试**：本机真实麦克风或真实模型。

## R1–R7 状态

| 编号 | 状态 | 实现 | 主要证据 |
|---|---|---|---|
| R1 | 已修复（模拟验证） | ChatPanel / DialogBox / StatusPanel 的 `.at(-1)` 改为下标取值；设置页头像预览补上 `MESH_SUPPORTED` 判断（验证中新发现：预览在缺 API 时仍会创建 `<mesh-avatar>` 并抛错）；`scripts/test_webkit_compat.mjs` 静态守卫 | 无头 Chrome 删除 8 个 Safari 15.4+ API 并模拟 Tauri IPC，加载构建产物：12/12 通过（主界面、头像降级、安装中、错误、思考步骤、回复、设置入口、6 个设置页、无未捕获异常）。同一测试跑 HEAD 版组件：展开即抛 `messages.at is not a function`，设置页抛 `i.at is not a function` |
| R2 | 已修复 | `publish_then_spawn`：先发布 DAEMON，再把 `&'static Daemon` 作为参数传给 worker；`alive` 标记 worker 退出 / 启动失败；`ready_daemon` 在录音时重试 prewarm；无设备 / 权限被拒（AVFoundation 授权状态）/ 配置失败给出原因和恢复步骤 | 单测：worker 线程在 `publish_then_spawn` 返回前跑完（最不利的调度）仍拿到已连通的 receiver；启动失败标记 `alive=false` 并报错；并发只启动一个 worker。真实麦克风：prewarm → 预建流 → 快速启动 → 两轮各 1.36 秒录音 → 每轮后重新预建，预热语义不变 |
| R3 | 已修复（第二轮补齐；模拟 + 临时目录真实 Python，打包版未复测新场景） | 第二轮：就绪快速路径除 marker 外还要求 pip 可用、核心模块可导入；破坏性重建前先删 marker；遗留的“marker + 缺核心模块”在入口处被识别并删掉 marker（见末节）。第一轮：`prepare_venv`：解释器不能运行 → `venv --clear` 重建（只清 PA venv）；缺 pip → `ensurepip`，失败再重建；重建后仍缺 pip → “语音环境不完整”与具体步骤；依赖改用 `python -m pip`；pip 错误区分网络与非网络 | 单测（真实 Python 3.9.6，不联网）：`--without-pip` venv 原地修复；不可执行的解释器被重建，同级 `.env` / `settings.db` 不变；创建后仍缺 pip 报“没有可用的 pip”且不提网络。隔离测试：打包版在 pip 安装中途被 `kill -9`，重启后继续安装、写入 marker、启动语音服务 |
| R4 | 行为契约已明确并实现 | 打包版只读 `~/.pocket-agent/.env`；开发版读仓库 `.env`（与 cwd 无关），不存在才读用户文件；整份文件二选一；进程环境优先；启动日志 `[config]` 写明来源、网关、key 是否设置；解析错误只报行号；连不上 / 401 / 403 带网关与来源 | 单测 6 项。真实二进制 6 种组合（开发版无关目录、进程环境覆盖、打包布局从仓库目录启动、打包布局覆盖 key、语法错误含密钥、无用户文件）全部符合契约，日志中密钥原文出现 0 次。新 `.app` 隔离启动同样读到用户文件 |
| R5 | 已修复（第二轮补齐；模拟） | 第二轮：取消代际在转写派发时取一次（`CancelToken`），HTTP 前、重试等待中、helper 启动前检查，并传给运行中的 helper（见末节）。第一轮：`stt_model()` / `lang_prompt()` 服务与回退共用；helper 接收 `--model` / `--lang-prompt`，`local_files_only`；回退有时限（30 s + 3×录音时长）和取消（对话 worker 退出时），超时 / 取消 kill 并回收 | 单测：卡住的子进程按时限结束并回收、取消在 3 秒内结束、helper 参数契约（假 faster_whisper 记录 model / local_files_only / initial_prompt）、未缓存模型返回 2。真实模型：临时 HF 缓存只放 base，`--model base` 2.5 s 转写成功；`--model tiny` 1 s 内报“不在本机缓存中”，没有下载 |
| R6 | 已修复 | `checkPort` 区分空闲 / 占用 / 无法确认；macOS 用 `/usr/sbin/lsof`、`/bin/ps` 绝对路径；无法确认时非零退出；仍不结束任何进程 | node 测试 8 项（真实 IPv4-only / IPv6-only 监听、ENOENT、异常退出、ps 失败、Windows 解析）。真实 1420：空闲 0；IPv4 / IPv6 占用退出 1、进程保留；PATH 不含 `/usr/sbin` 且端口被占：新脚本退出 1，HEAD 版退出 0 |
| R7 | 已完成 | README / README_AGENTS / `.env.example` / Info.plist / Python 版本提示 | 见下方“文档变更” |

## A01–A20

| 编号 | 结果 | 类型 | 证据 / 原因 |
|---|---|---|---|
| A01 | 未执行 | — | 没有干净的 Mac 或新用户账户；本机已装 CLT、Rust、Node、Hermes。替代证据见 A02（隔离 HOME 下的打包版首次启动） |
| A02 | 通过 | 隔离 | 新构建 `.app` 复制到与仓库无关的目录，隔离 HOME 从无关 cwd 启动：`[config] 打包版` 读用户文件；`Contents/Resources/resources/` 四个资源被找到；用 `/usr/bin/python3` 建 venv、pip 安装、下载 Whisper base 与声纹模型、8651 健康、窗口显示；对其服务做转写请求成功 |
| A03 | 通过（模拟） / 真实未执行 | 模拟 | Chrome 删除 `.at` / `hasOwn` / `structuredClone` / `findLast(Index)` / `randomUUID` 后核心 UI 与设置可用、头像降级（R1）。**未在真实 macOS 12.0 WebKit 上运行**：仍声明支持 12.0 的依据只有构建目标与此模拟 |
| A04 | 通过 | 模拟 + 真实硬件 | R2 单测（强制 worker 先运行）；真实麦克风两轮录音 |
| A05 | 部分 | 模拟 | 无设备、权限被拒、配置失败的提示与恢复步骤已实现，授权状态读取在本机验证（返回 3，与 Swift 一致）。**未实测**拔掉麦克风或拒绝权限：需要改动本机 TCC 授权，没有执行 |
| A06 | 通过（新增场景为单元级） | 模拟 + 隔离 | 第一轮：R3 单测（缺 pip、解释器失效、重建仍缺 pip、找不到 Python）；打包版 pip 中断后重启恢复。第二轮：经 `ensure_venv_in()`（`ensure_venv()` 的全部逻辑）验证“匹配旧 marker + 缺 pip”“重建中断后 marker 残留 + 缺核心模块”“重建在解释器建好后被杀”三种状态均不发 ready、下次启动恢复、同级用户文件不变（见末节）。**新增场景没有在打包版 `.app` 上用 kill 复测** |
| A07 | 通过 | 隔离 | R4 六种组合 + 新 `.app`；不输出密钥 |
| A08 | 通过 | 真实模型 | R5：只有 base 缓存时回退用 base，tiny 未缓存直接报错不下载。整个应用里“常驻服务停止后走回退”这条链路没有端到端运行 |
| A09 | 部分（四个取消时机单元级通过，界面端到端未执行） | 模拟 | 第一轮：卡住 / 取消的子进程被结束并回收。第二轮：HTTP 进行中、重试等待、helper 启动前、helper 运行中取消，均不在取消后启动 helper，运行中的被结束并回收，随后新请求正常完成（见末节）。“取消后开始下一轮对话”仍没有在界面上端到端验证 |
| A10 | 通过 | 真实 + 模拟 | R6 |
| A11 | 通过（回归） | 模拟 | 上一批实现；本批 `cargo test` 中 `port_probe_tells_free_ours_and_foreign_apart` 等仍通过。本批没有重新做界面实测 |
| A12 | 部分 | 隔离 | 打包版首次启动：安装期间窗口可见、状态上屏，服务约 30 s 后就绪。“超过 30 s 继续等待”由既有测试 `probe_keeps_waiting_past_the_first_deadline_until_healthy` 覆盖；本批没有人为制造慢网络 |
| A13 | 部分 | 模拟 | pip 网络 / 非网络错误分类有单测；声纹失败不阻塞基础转写为既有行为。没有真实断开 PyPI / HuggingFace |
| A14 | 部分 | 真实模型 | helper 只用本地缓存已验证；TTS 依赖在线服务已写入文档。没有真实断网测试 |
| A15 | 部分 | 真实硬件 + 隔离 | 真实麦克风录音（A04）、合成 WAV 经新 `.app` 的服务转写成功。**未执行**：Hermes SSE 回合、TTS 播放、打断、Esc（需要界面操作和网关请求；合成 WAV 不能替代麦克风验收） |
| A16 | 通过 | 模拟 | 既有 seq / 快照测试仍通过；R1 测试中依次注入安装中、错误、就绪事件，界面状态正确 |
| A17 | 部分 | 隔离 | 所有测试都用隔离 HOME，真实 `.env`、数据库、`server.token` 未改动；venv 重建不影响同级文件（单测）。已知：用 SIGTERM 结束 PA 时语音服务不会随之退出（既有行为，测试中按 PID 清理）；没有验证托盘正常退出 |
| A18 | 未执行 | — | 可选功能（唤醒 / 声纹 / 旧系统兼容库），本批未改动 |
| A19 | 未执行 | — | 可选功能（电脑工具），本批未改动 |
| A20 | 未执行 | — | 可选功能（手机 / MCP，实验性），本批未改动 |

## 自动化检查（本批执行）

| 命令 | 结果 |
|---|---|
| `cd src-tauri && cargo test --lib` | 156 通过，0 失败 |
| `cd src-tauri && cargo build` | 0 警告 |
| `node --test scripts/test_*.mjs` | 22 通过，0 失败（含新增 `test_webkit_compat.mjs`、`test_dev_port.mjs`） |
| `./node_modules/.bin/tsc --noEmit -p tsconfig.json` | 通过（不覆盖 `.svelte`） |
| `npx vite build` | 168 模块，通过 |
| `npm run tauri build` | `.app` 与 DMG 生成成功 |

## 文档变更（R7）

- 支持范围：README / README_AGENTS 写明 UI 面向 macOS 12.0 的 WebKit，已验证只有 Apple Silicon + macOS 26，macOS 12–15 与 Intel 未验证；bundle 仍是 `LSMinimumSystemVersion` 12.0。
- Python：统一写 3.9–3.12；找不到可用 Python 的提示补上“已安装但版本不在范围 → 安装 Python 3.12”。
- 联网：README 新增“Network use”表，区分安装期（PyPI、HuggingFace、GitHub）和日常使用（edge-tts 依赖 Microsoft 在线服务，模型是否联网取决于 Hermes 后端）；TTS 与本地 STT 分开排查。
- 配置：README 新增“Where PA reads its configuration”；README_AGENTS、`.env.example` 同步；复制 `.env` 的命令改为不覆盖已有文件。
- 恢复：venv 自动修复、手动重建只删 venv 与 marker、回退 helper 行为、端口检查“无法确认”的情况。
- 权限文案：Info.plist 不再写“按住 fn 键”，改为“录音快捷键 / 全局录音快捷键”。
- 基础与可选：README 写明基础安装是文字、听写、播报、打断；唤醒 / 声纹、电脑工具、手机端分别验收；HERMES.md 的调优和补丁标为可选。

## 未覆盖与剩余阻碍

- 没有干净的 Mac、没有 macOS 12.x 或 Intel 机器：A01 未执行，A03 只有模拟证据。
- 麦克风拒绝 / 无设备、真实断网、Hermes 回合、TTS、打断、Esc 没有在本批实测。
- O1–O3 按要求未处理。

## Codex 独立复核（2026-11-04）

### 结论与范围

本轮严格按既有 R1–R7 和相关回归检查，没有增加新的功能或可选优化要求。实现整体有明显进展，但 **R3、R5 尚未完全关闭**；R1 的真实最低系统验收、A01 和若干音频端到端项目仍保持未验证。

| 项目 | 独立结论 |
| --- | --- |
| R1 | 已核对核心组件替换及设置预览降级，静态兼容守卫通过。真实 macOS 12.0 WebKit 仍未验收，不能当作已实测通过 |
| R2 | 初始化顺序已修复：共享状态先发布，worker 直接接收引用；启动失败/退出有 alive 状态。相关并发与启动测试通过。权限和真实打断等范围依旧按矩阵保留 |
| R3 | **部分完成**：缺 pip 的普通路径可修复，但匹配的旧 marker 可绕过整个环境修复，见下文 R3 残留 |
| R4 | 代码与文档的配置契约已一致：打包布局只读用户文件，开发版项目优先，进程环境优先；配置来源诊断与对应测试通过 |
| R5 | **部分完成**：模型/提示词/本地缓存、运行中 helper 超时与取消已实现；取消发生在 HTTP 阶段时仍可随后启动回退，见下文 R5 残留 |
| R6 | 已修复：工具执行失败与正常无监听分开；空闲、IPv4、IPv6、工具失败、进程名不可取等测试通过；无自动终止进程 |
| R7 | 文档与权限文案收尾基本完成；交付记录诚实区分实机、隔离、模拟和未执行。完整安装验收仍未完成 |

### R3 残留 [P2]：旧 marker 匹配时，半重建环境被直接认定为就绪

**位置**：`src-tauri/src/voice/venv.rs:39–50`、`:341–344`、`:360–366`。

`venv_ready()` 当前只检查解释器能运行和 marker 内容等于 requirements，不检查该 marker 对应的环境是否仍完整。只要匹配，就在 `ensure_venv()` 开头返回，完全跳过新增的 `prepare_venv()`。

可达场景：已有成功安装的 marker；解释器失效触发 `venv --clear` 重建；进程在解释器已创建、pip/依赖尚未完成时中断。旧 marker 要等到 `prepare_venv()` 成功返回才删除，因此该中断窗口里旧 marker 仍在。下次启动，解释器能够运行，marker 也匹配，于是把缺 pip/依赖的环境当成 ready。

本轮隔离验证运行当前原样 venv 模块，仅替换用户目录和 Tauri 窗口/事件接口；临时环境中放置可执行解释器、缺失 pip，以及与当前 requirements 完全匹配的旧 marker。结果：

```text
matching_marker_but_no_pip_ready=true
event=venv-setup-ready
ensure_venv_result=Ok(())
```

测试没有连接 PyPI，没有修改真实用户目录。这是“中断后状态”的故障注入，不是声称本轮对真实 .app 进行了 kill 实测。

**本项修复要求不变**：就绪快速路径不能让残留 marker 绕过实际环境完整性；任何破坏性重建之前先使旧就绪凭据失效，且能处理已经遗留的错误 marker 状态。不要仅对 `prepare_venv()` 增加测试而绕过 `ensure_venv()` 的真实入口。

**补充验收到原 A06**：覆盖“匹配旧 marker + 缺 pip”和“重建中断后 marker 残留 + 缺核心依赖”；经真实 `ensure_venv()` 入口恢复，不误发 ready、不触碰同级用户数据。A06 本轮应记为**部分通过**，不能全关闭。

### R5 残留 [P2]：取消发生在 HTTP 阶段时，回退会重新取 epoch 并继续执行

**位置**：`src-tauri/src/voice/stt.rs:339–342`、`:367–393`；`conversation.rs` 的 worker 退出清理与分离的 `conv-transcribe` 线程。

取消 epoch 直到 `transcribe_subprocess()` 启动 helper 前才读取。一次转写先经历 HTTP、等待与重试；如果对话在这些阶段结束，`cancel_fallback()` 已增加 epoch，但稍后进入回退时读取的是增加后的新值，于是把已取消的请求当作新的有效任务。现有“运行中的 helper 被取消”单测没有覆盖该阶段交界。

本轮隔离验证使用当前原样的 `transcribe()`、`transcribe_subprocess()`、`run_bounded()` 及 epoch 逻辑；HTTP 用同步屏障模拟失败，helper 用只写测试标记并输出 JSON 的临时脚本替代。转写已进入第一个 HTTP 请求后调用真实 `cancel_fallback()`，然后允许 HTTP 失败和重试继续，结果：

```text
cancelled_during_http_helper_started=true
cancelled_transcribe_result=Ok("after cancel")
```

没有访问真实 8651、加载真实模型或操作用户对话。该验证表明已取消转写仍可启动回退；不据此声称 UI 一定显示了取消后的文本。

**本项修复要求不变**：取消标识应属于整次转写/所属会话，从请求开始跨 HTTP 与回退阶段传播；在启动 helper 前和运行过程中都检查原请求是否失效。不要仅在 helper 启动时重新采样全局 epoch。保留对新请求的代际隔离，不能让旧请求的结束误取消下一轮。

**补充验收到原 A09**：取消发生在 HTTP 进行中、重试等待、helper 即将启动和 helper 运行中时，旧请求均不能在取消后新启动回退；随后新请求仍能成功开始。A09 继续记为**部分通过**。

### 本轮独立执行结果

| 命令/检查 | 结果 |
| --- | --- |
| `cargo test --locked --offline --lib` | **156 通过、0 失败**（获准在沙箱外运行，包含随机本地端口与临时 venv 测试） |
| `node --test scripts/test_dev_port.mjs scripts/test_webkit_compat.mjs scripts/test_idle_hint.mjs scripts/test_voice_setup.mjs scripts/test_wake_errors.mjs` | **22 通过、0 失败**，含真实随机端口 |
| `npm run build` | 通过，168 模块 |
| `./node_modules/.bin/tsc --noEmit -p tsconfig.json` | 通过；不等于对 `.svelte` 做了类型检查 |
| 上述 R3/R5 阶段边界复现 | 均复现未覆盖缺口；不能把复现程序成功退出计为产品功能通过 |

本轮没有重复 Claude 的真实麦克风、新打包 .app 或无头 Chrome GUI 实验；未对当前用户环境做权限、配置或依赖改动，没有触发真实 Hermes 请求。只追加本复核结果，未修改产品实现。O1–O3 仍不属于本轮必做范围。

## R3 / R5 残留修复（2026-11-04，Claude 第二轮）

范围：只处理复核中的 R3、R5 残留，其余修复保留未动。代码仍未提交。

### R3：旧 marker 不再绕过环境修复

| 改动 | 位置 |
|---|---|
| `venv_ready(root, marker, expected)`：marker 与 requirements 一致，**且** pip 可运行（`venv_health == Usable`）、核心模块可导入，才走快速路径 | `src-tauri/src/voice/venv.rs:33` |
| `previous_install()`：marker 只有在核心模块可导入时才算“之前装好过”；否则视为中断重建的残留，直接删除。依赖更新失败时保留旧安装的回退行为不变 | `venv.rs:47` |
| `prepare_venv()` 在执行 `venv --clear` 之前先删 marker（删不掉就报错，不重建），重建中断不会留下就绪凭据 | `venv.rs:137` |
| `ensure_venv(app)` 只负责绑定 `AppHandle`（事件发布、显示窗口、`run_streaming`），全部流程移入 `ensure_venv_in(layout, find_python, run, publish)`；测试经由它运行 | `venv.rs:351`、`:376` |
| 删除被替代的 `marker_expected()`；README_AGENTS 恢复说明补上“即使留下了 `.venv-ready`” | — |

只操作 `layout.root`（`~/.pocket-agent/venv`）和 `layout.marker`（`~/.pocket-agent/.venv-ready`）。

验证（**模拟 + 临时目录**：本机 `/usr/bin/python3` 3.9.6 真实建 venv、真实 ensurepip；pip 自升级与 `pip install -r` 由测试替身代替，前者模拟断网失败，后者要么模拟断网失败，要么在 site-packages 写入 7 个核心模块的空桩；临时目录里放 `.env`、`settings.db`、`chat_history.db`、`server.token`、`voiceprints/owner.bin`、`models/whisper/model.bin`，不碰真实 HOME，不联网）：

| 测试 | 场景与断言 |
|---|---|
| `matching_marker_next_to_a_venv_without_pip_is_not_ready` | 复核中的复现状态（`--without-pip` venv + 匹配 marker）。断网：返回 PyPI 错误、不发 `venv-setup-ready`、marker 被删、pip 已原地恢复；联网：`started … done`，写回 marker；再次启动才走快速路径（事件只有 `venv-setup-ready`）；用户文件不变 |
| `marker_left_by_an_interrupted_rebuild_without_core_modules_is_not_ready` | 有 pip、缺核心模块、marker 匹配（旧版本重建中断的遗留）。断网：报错、不发 ready、不走“保留旧安装”回退、marker 被删；联网：完成安装、核心模块可导入；用户文件不变 |
| `rebuild_drops_the_marker_before_clearing_the_venv` | 完整安装后删掉 pip、ensurepip 失败 → 触发 `--clear` 重建；重建在解释器建好后模拟被杀。断言执行 `--clear` 时 marker 已不存在、不发 ready；下次启动正常恢复；用户文件不变 |
| `update_failure_still_keeps_a_complete_previous_install` | 回归保护：完整安装后 requirements 变更且断网，仍保留旧安装并发 ready，marker 保留待下次重试 |

变异检查（临时改回旧逻辑后跑上述测试，再恢复）：快速路径恢复为“解释器能运行 + marker 匹配”时，前两个测试失败（`unwrap_err()` 拿到 `Ok(())`，即复核中的误判）；去掉重建前删 marker 时，第三个测试在“marker still present when the venv is cleared”处失败。

限制：
- `ensure_venv()` 包装本身（真实 `AppHandle` 事件、显示窗口、`run_streaming`）没有被测试执行；新场景没有在打包版 `.app` 上用 `kill -9` 复测。本机当时有一个 PA 实例在运行，为避免端口 8651、快捷键与托盘冲突，本轮没有启动第二个实例。
- 快速路径每次启动多两次 Python 调用。本机现有 venv 实测：`pip --version` 0.09–0.22 s，核心模块导入 1.03–1.45 s，语音服务就绪相应推迟约 1–1.7 s。

### R5：取消从转写开始一直有效

| 改动 | 位置 |
|---|---|
| `CancelToken`：创建时记下 `FALLBACK_EPOCH`，之后任何 `cancel_fallback()` 都使其失效；之后新建的 token 不受影响 | `src-tauri/src/voice/stt.rs:237` |
| `transcribe_until(wav, token)` → `transcribe_staged()`：第一次 HTTP 前、重试等待期间（每 20 ms 检查）、重试前、启动 helper 前都检查；同一个检查函数传给 `run_bounded()`，运行中的 helper 仍按时限或取消被 kill 并回收。`transcribe()` 保持原签名（单次听写用），在入口处取 token | `stt.rs:387`、`:399` |
| 对话：token 在 worker 线程派发转写时就取（早于声纹检查和等待语音服务重载），会话结束的 `cancel_fallback()` 对之后各阶段都有效 | `src-tauri/src/voice/conversation.rs:673`、`:704` |

已在进行中的 HTTP 请求无法中断，最长等到它自己返回（客户端超时 30 s）；返回后不再重试、不再启动 helper。HTTP 在取消后成功返回的结果仍会交回调用方，由对话 worker 丢弃（与之前相同）。

验证（**模拟**：真实 `CancelToken` / `cancel_fallback()` / `transcribe_staged()` / `run_bounded()`；HTTP 由闭包代替，helper 由写 pid 与启动标记的 `/bin/sh` 脚本代替；不访问 8651、不加载模型）：

| 测试 | 取消时机与断言 |
|---|---|
| `cancel_during_http_never_starts_the_helper` | 第一次 HTTP 阻塞时取消：返回“备用转写已取消”、HTTP 只调用 1 次、helper 未启动；取消后、旧请求结束前新建的 token 不失效 |
| `cancel_during_retry_wait_never_starts_the_helper` | 第一次 HTTP 失败后、1 次重试等待中取消：300 ms 等待内立即结束、不重试、helper 未启动 |
| `cancel_just_before_the_helper_never_starts_it` | 重试失败的同时取消：HTTP 调用 2 次、helper 未启动 |
| `cancel_while_the_helper_runs_kills_and_reaps_it` | helper 运行中（`sleep 30`）取消：3 秒内返回，helper 进程已不存在 |

四个测试结束时都用新 token 再跑一轮（HTTP 失败 → helper），均正常返回 helper 输出。

变异检查：把阶段检查去掉、helper 启动时重新读取全局 epoch（即复核前的行为），前三个测试都失败，返回 `Ok("helper")`，与复核中的 `cancelled_transcribe_result=Ok("after cancel")` 一致；第四个测试在新旧实现下都通过（旧实现本来就能取消运行中的 helper）。

限制：没有在应用里端到端验证“对话结束 → 取消 → 下一轮对话”，也没有让真实语音服务停止后走回退。未改变的既有行为：新的 `start_conversation` 直接接管旧会话时（旧 worker 退出时已不再拥有槽位），不会调用 `cancel_fallback()`，旧会话中正在运行的 helper 只受时限约束。

### 本轮执行的检查

| 命令 | 结果 |
|---|---|
| `cd src-tauri && cargo test --offline --locked --lib` | 164 通过、0 失败（第一轮 156 + 本轮新增 8）；另外连跑 3 次均 164 通过 |
| `cd src-tauri && cargo build --offline` | 0 警告 |
| `node --test scripts/test_*.mjs` | 22 通过、0 失败 |
| `./node_modules/.bin/tsc --noEmit -p tsconfig.json` | 通过（不覆盖 `.svelte`） |
| `npm run build` | 168 模块，通过 |
| 上述两组变异检查 | 新测试在旧逻辑下按预期失败，恢复后通过 |

“测试全部通过”只说明上述模拟 / 临时目录场景成立。R3 的打包版中断复测、R5 的界面端到端取消，以及第一轮列出的 A01、A03 真实 macOS 12、麦克风权限、断网、Hermes 回合、TTS、打断、Esc 仍未执行。

## 升级路径与旧 macOS 唤醒（2026-11-05，0.3.1）

来源：一台装过旧版 PA 的 macOS 12.6.3 Intel 机器 `git pull` 后，先是 vite 找不到 `mesh-avatar-element`，随后唤醒词 / 声纹不可用（sherpa-onnx 1.12.26 自带的 onnxruntime 需要 macOS 13.4；兜底的自建兼容包从未发布，下载 404）。本批改动见 `.auto-work/docs/changes-20261105-005756.md`：`npm run dev` / `build` 前检查 npm 依赖；macOS < 13.4 改装 PyPI 上的 sherpa-onnx 1.10.46，`wake_kws.py` 自带发音转换；删除兼容包下载与 stt-server 重载。

| 编号 | 结果 | 类型 | 证据 / 原因 |
|---|---|---|---|
| A18 | 部分 | 本机 | 两个 venv（1.10.46 / 1.12.26）经 `WakeKws.create/feed`，`say` 合成的「你好小星」「hello computer」均命中、负样本不命中；自带转换与 1.12.26 `text2token` 对 7 个短语结果相同。**未在真实旧系统上运行** |
| A21 | 部分 | 本机 | npm 部分通过：移走 `node_modules/mesh-avatar-element` 后 `check-deps.js` 与 `npm run dev` 打印包名和 npm install 提示、退出 1，`npm install` 后退出 0。Python 部分只验证了选择规则：新版 packaging 与 pip 21.2.4 内置的 packaging 21.0 对 Darwin 20.6 / 21.6 / 22.4 均选 1.10.46，22.5 / 22.6 / 25.6 / Linux / Windows 均选 1.12.26。**未在装过旧版的机器上实际 `git pull` 启动** |
| A22 | 未执行 | — | 没有 macOS 12 Intel 机器。1.10.46 x86_64 wheel 来自 PyPI，dylib minos 为 10.14 / 11.0（onnxruntime 1.17.1） |

| 命令 | 结果 |
|---|---|
| `cd src-tauri && cargo build` | 0 警告 |
| `cd src-tauri && cargo test --lib` | 151 通过、0 失败（删除兼容包、重载与租约相关的测试） |
| `node --test scripts/*.mjs` | 27 通过、0 失败（新增 `test_check_deps.mjs`；Node 22 的 `node --test` 不接受目录参数） |
| `python scripts/test_wake_kws.py` | 24 通过（含真实模型对照） |
| `python scripts/test_kws_integration.py` | 通过（PA 安装的模型不含 `test_wavs/`，用临时模型目录 + `say` 合成的 `en_0.wav`） |
| `npx tsc --noEmit`、`npm run build` | 通过 |

未执行：`npm run tauri build`；macOS 12.6.3 Intel 实机上重启后 pip 自动降级、设置唤醒词并唤醒、日志无 `[sherpa-compat]`。
