# 新机器安装审核与 Claude 修复交接

审核日期：2026-11-04  
项目版本：0.3.0  
审核目标：一台没有 Pocket Agent、Hermes/OpenClaw、Python 语音环境及模型缓存的 Mac，能否仅按项目文档完成安装、启动和基本语音对话。

## 审核结论与范围

审核时的文档和代码尚不足以保证新机器安装顺畅。发现了打包资源查找、首次启动等待、Python 选择等代码问题，以及安装文档缺失或与实现不一致的问题。

本次做了文档与代码静态核对、现有本地 `.app` 资源目录检查，以及当前开发环境的构建检查。没有在全新机器或干净 macOS 用户环境中进行端到端安装。下述运行影响来自代码路径分析，不应当表述为全部经过换机实测。

**交接时注意：** 创建本文档时，工作区的 `.env.example`、`README.md`、`README_AGENTS.md`、`android/README.md`、`docs/computer-interaction.md` 已有未提交改动，其中部分出现在最初审核之后。下面记录的是本次审核时发现的问题；Claude 应先核对最新文件，确认哪些已经修复，保留现有改动，不要直接覆盖或回退。文档行号会变化，以函数名和描述定位为准。

## 给 Claude 的执行要求

请逐项核查并修复以下问题，完成对应验证；已修复的项目注明证据，仍无法验证的项目明确保留为待验收。只处理安装、启动和相关文档，不顺带重构无关功能。

- 保留现有配置、凭据、数据库、模型缓存和其他未提交改动。
- 不将当前机器已有依赖或缓存下的成功视为新机器安装成功。
- 不直接执行文档中可能过期的后端命令。先核对所支持版本的官方文档或 CLI 帮助。
- 如需清理环境或更换解释器，优先使用隔离目录或测试用户，避免删除真实用户的 `~/.pocket-agent`。
- 最终报告修复文件、验证结果、尚未验证的系统/架构和剩余限制。

## 1. [P1] 打包资源目录与读取路径不一致

**位置**

- `src-tauri/tauri.conf.json`：`bundle.resources`
- `src-tauri/src/voice/venv.rs`：`resource_path()`、`marker_expected()`、`ensure_venv()`
- `src-tauri/src/voice/stt.rs`：`helper_path()`、`server_path()`

**证据与影响**

审核时打包映射将文件放在 `Contents/Resources/resources/`，但 `resource_path()` 首先查找 `Contents/Resources/<filename>`，随后回退到开发环境或相对源码路径，没有查找实际打包子目录。

本地已有 `.app` 中，`stt-server.py`、`stt-helper`、`requirements-stt.txt` 确实位于 `Contents/Resources/resources/`。此产物不是本次重新构建的安装包，但其布局与当前配置一致。

新机器没有源码目录可供回退时，可能报 `requirements-stt.txt 缺失`，无法初始化语音环境；语音脚本也存在相同查找问题。当前配置中的 `wake_kws.py` 同样需要检查打包交付。

**建议修复**

统一打包布局和资源解析，优先使用适合 Tauri 打包资源的路径解析机制，同时保留正常开发模式支持。

**验收标准**

- 重新构建 `.app`，检查四个资源文件均存在。
- 从与仓库无关的工作目录启动，且测试条件不能通过源码路径兜底。
- 验证首次创建语音环境、启动常驻服务和一次转写；不能仅以 `cargo check` 代替。

## 2. [P1] 首次模型下载超过健康检查期限，主窗口可能一直隐藏

**位置**

- `src-tauri/tauri.conf.json`：主窗口 `visible: false`
- `src-tauri/src/voice/stt.rs`：`ensure_stt_server()`、`probe_health()`、`mark_app_ready()`、`show_main_window()`
- `src/App.svelte`：`onMount` 中等待 `is_app_ready` / `app-ready` 后显示窗口
- `src-tauri/resources/stt-server.py`：`main()`、`_startup_selfcheck()`、`_download_speaker_model()`

**证据与影响**

后端健康检查最多等待 30 秒；超时只打印警告，没有继续检测并在稍后健康时显示主窗口的路径。前端也在等待 `app-ready` 才显示窗口。

新机器需要下载声纹模型和 Whisper 模型，服务在下载和加载完成后才监听端口。若耗时超过 30 秒，即使服务之后正常启动，当前探测也已经退出，主窗口可能持续隐藏。Python 依赖安装期间同样缺少可见主窗口，已有进度事件未必能让用户看见安装进展。

另一个独立失败条件是：声纹模型下载失败被列为 fatal，导致整个常驻语音服务退出，即使用户没有启用声纹验证。声纹下载调用也没有显式超时参数。

**建议修复**

让安装/下载状态和错误可见；为服务较晚就绪、启动失败提供明确的恢复或重试路径。评估将声纹模型作为可选能力加载，避免不使用声纹的基础语音功能被其下载阻断。不要只把 30 秒改成另一个固定长等待就视为完成。

**验收标准**

- 模拟首次下载超过 30 秒，用户能看到状态，服务就绪后能正常使用。
- 模拟 PyPI 或模型站点不可达，显示具体失败及恢复办法，不只隐藏窗口或输出终端日志。
- 若实现声纹降级，验证未启用声纹时基础按键转写仍可使用，声纹功能正确显示未就绪。
- 保留正常启动、服务重载和手机接管时的既有窗口行为。

## 3. [P1] Python 安装说明与应用自建虚拟环境机制不一致

**位置**

- `README.md`：Prerequisites、packaged app 的 Python 说明
- `README_AGENTS.md`：Python packages、排错说明
- `.env.example`：`STT_PYTHON` 注释
- `src-tauri/src/lib.rs`：调用 `ensure_venv()` 的初始化线程
- `src-tauri/src/voice/venv.rs`、`src-tauri/resources/requirements-stt.txt`
- `src-tauri/src/voice/stt.rs`：`resolve_python()`

**证据与影响**

审核时文档要求 `pipx install edge-tts` 和 `pip install faster-whisper`，并称打包应用依赖全局 Python 包。实际启动会自动创建 `~/.pocket-agent/venv`，安装完整 requirements；默认语音运行优先使用应用自己的环境。

因此全局包安装既重复，也不保证应用环境可用。裸 `pip` 可能不存在或指向不同解释器；Homebrew 当前 Python 的 externally managed 限制还可能直接阻止全局安装。

**建议修复与验收标准**

- 明确一种默认支持路径，解释自动安装、所需解释器和失败恢复办法。
- 将手动准备环境和 `STT_PYTHON` 明确标注为高级覆盖路径，并核实覆盖环境需要的完整依赖及初始化限制。
- README、Agent 指南和 `.env.example` 保持一致。
- 验证默认路径不需要用户给系统 Python 全局装包。

参考：[Homebrew Python 环境说明](https://docs.brew.sh/Language-Runtimes-and-Packages)。

## 4. [P1] Python 自动发现没有校验版本，也没有保证选中新安装的解释器

**位置**

- `src-tauri/src/voice/venv.rs`：`find_system_python3()`
- `README_AGENTS.md`：Python 前置条件

**证据与影响**

文档要求 Python 3.10+，但代码只检查候选解释器执行 `--version` 是否成功，不解析版本，也不验证创建 venv 的能力。第一个可执行的旧 Python 可能被选中。

Agent 指南审核时只写“缺失则安装 `python@3.11`”，没有处理已安装但版本不足的情况，也没有保证版本化 Homebrew Python 能以 `python3` 被当前发现逻辑找到。仅设置 `STT_PYTHON` 不应被默认视为已经修复 bootstrap 选择，因为初始化使用的是 `find_system_python3()`。

**建议修复与验收标准**

- 明确实际验证过的 Python 版本范围，兼顾 `numpy<2`、`av<13` 等原生包约束，不能只假定版本越新越好。
- 选择时校验版本和 venv 能力，正确支持文档推荐的解释器位置。
- 覆盖“旧 Python 在 PATH 前面，新 Python 也已安装”的场景，以及没有任何合适解释器的场景。
- 提供可操作的错误提示，说明实际选中路径与版本。

## 5. [P1] 后端从零配置流程不完整，OpenClaw 缺少必需接口启用步骤

**位置**

- `README_AGENTS.md`：Step 4
- `README.md`：Backend gateway、Getting Started
- `HERMES.md`
- `docs/computer-interaction.md`

**证据与影响**

审核时安装指南基本从“启动网关”开始，缺少完整的后端安装、主模型与凭据、API 服务启用、端口与认证配置、请求验收流程。`HERMES.md` 主要是调优、skill 安装和局部补丁说明，不能替代新用户初始化指南。

OpenClaw 指南只给出 `openclaw serve`，需要核对支持版本的实际命令。更明确的缺失是：OpenClaw 官方文档说明 `/v1/chat/completions` 默认关闭，必须配置 `gateway.http.endpoints.chatCompletions.enabled: true`。只启动网关不保证 PA 能使用该接口。

`OPENCLAW.md` 在审核时尚不存在，其他文档仍引用“once it exists”。

**建议修复与验收标准**

- 以一个清晰的默认后端完成从零安装路径，其他后端提供独立且完整的补充指南。
- 给出适用版本、官方安装入口、模型配置、API 启用和认证匹配方式。
- Hermes 说明实际 profile、独立网关和 `API_AGENT` 留空要求；桌面控制作为单独验收能力。
- OpenClaw 修正启动命令并补齐接口启用步骤。
- 先完成一条真实的流式文字请求，再连接 PA；端口监听或 health 成功不能替代模型请求验收。
- 不把 `HERMES.md` 中的性能优化、模型专属参数和本地补丁全部变成基础安装必选项。

参考：[OpenClaw Gateway CLI](https://docs.openclaw.ai/cli/gateway)、[Chat Completions 启用说明](https://docs.openclaw.ai/gateway/openai-http-api)。

## 6. [P2] README 克隆地址仍是占位符

**位置**：`README.md` 的 Setup 克隆命令。

审核时命令为 `git clone https://github.com/YOUR_USERNAME/pocket-agent.git`，无法照抄。Git remote 和 Agent 指南中的地址为 `https://github.com/kevin-ping/pocket-agent`。

**验收标准**：所有安装入口统一真实仓库地址，检查相邻的 `cd` 命令和后续工作目录是否连续正确。

## 7. [P2] 源码安装缺少明确的 Xcode Command Line Tools 安装与验证

**位置**：`README.md`、`README_AGENTS.md` 的前置条件。

审核时没有单独明确要求安装并验证 macOS 编译工具。Rust 本身不等于系统链接器、SDK 等原生构建条件已经满足。Homebrew 安装可能引导安装，但 README 路径不应依赖这项隐含行为。

**验收标准**：补充桌面构建所需 Command Line Tools 或 Xcode 的步骤及验证方式，说明安装完成后再继续构建，不要求仅运行桌面源码项目的用户安装无关移动端工具。

参考：[Tauri macOS 前置条件](https://v2.tauri.app/start/prerequisites/#macos)，包括 `xcode-select --install`。

## 8. [P2] 文档未完整区分安装路线、首次启动阶段和分层验收

**位置**：`README.md`、`README_AGENTS.md` 及相关安装指南。

审核时主要提示首次 Rust 编译约 3–5 分钟，没有完整说明首次启动还需安装 Python 依赖、下载模型及加载服务。Agent 指南的当前版本仍为 0.2.7，而项目为 0.3.0。

**建议修复与验收标准**

- 将“源码开发安装”和“DMG 用户安装”分开，分别给出连续可执行的步骤。
- DMG 路径说明实际可获得的安装包、Intel/Apple Silicon 架构选择、配置文件创建位置及必要运行依赖。签名或首次打开说明应以实际产物为依据。
- 解释首次编译、依赖安装、模型下载、健康检查各阶段；不要承诺未经全新环境验证的固定总耗时。
- 给出可见的准备就绪信号和失败恢复办法，分别验收：文字聊天、录音转写、TTS、打断、可选唤醒与声纹、可选电脑工具。
- 将终端 Input Monitoring 权限放进开发安装的正常步骤，不只放在排错章节。
- 明确需要访问的依赖/模型下载站点；不要把本地 STT 推断等同于首次安装无需联网。

## 本次实际验证记录

| 检查 | 结果 | 能证明的范围 |
| --- | --- | --- |
| `npm run build` | 通过，167 个模块，Vite 6.4.2 | 当前已安装前端依赖下可构建 |
| `cargo check --locked --offline`（`src-tauri`） | 通过 | 当前已有 Rust 缓存与工具链下可通过编译检查 |
| 本地已有 `.app` 资源目录 | 确认语音资源位于 `Contents/Resources/resources/` | 支持资源路径错位发现；不是本次新打包验收 |
| 全新机器安装、真实麦克风/TTS 完整流程 | 未执行 | 仍需独立验收 |

本次没有重装依赖、删除缓存、启动后端、修改用户配置或执行电脑控制操作。创建本文档之外，没有为修复以上问题修改实现。

## 建议完成顺序

1. 核对当前未提交改动，标记已经解决的问题。
2. 修复打包资源解析和首次启动可见性/恢复路径。
3. 明确并验证 Python 解释器与依赖安装方案。
4. 补齐默认后端从零配置，统一人类与 Agent 安装文档。
5. 使用无应用环境和模型缓存的隔离条件验收；有条件时分别覆盖 Apple Silicon 与 Intel，并记录实际 macOS/Python 版本。
6. 回填每项状态和验证证据，不将未执行的端到端测试写成通过。

## 第二轮复审（2026-11-04）

**结论：当前工作区尚未修复安装阻断问题，不能通过新机器安装验收。** 本轮看到的更新主要是明确只支持 macOS/Hermes、移除 OpenClaw 安装入口和标注 Android 为实验功能；相关 Rust/Python 初始化代码及打包配置仍保留上述问题。

| 原问题 | 复审状态 | 当前证据 |
| --- | --- | --- |
| 1. 打包资源路径错位 | 未修复，隔离复现 | `bundle.resources` 仍写入 `Resources/resources/`；`resource_path()` 仍查找 `Resources/<filename>` |
| 2. 首次启动等待与失败恢复 | 未修复 | 健康探测仍以 30 秒为期限，超时仅输出日志；声纹模型下载失败仍进入 fatal |
| 3. Python 安装说明不一致 | 未修复 | README 仍要求全局/pipx 包；Agent 指南仍要求裸 `pip install faster-whisper` |
| 4. Python 选择不校验版本 | 未修复，隔离复现 | `find_system_python3()` 仍只判断 `--version` 是否成功 |
| 5. 后端从零配置不足 | 部分范围调整，Hermes 部分未修复 | OpenClaw 已明确不再支持，因此不再要求补齐该路径；Hermes 仍缺少从零安装、模型凭据、API 启用和真实请求验收的完整流程 |
| 6. README 克隆占位符 | 未修复 | 仍为 `https://github.com/YOUR_USERNAME/pocket-agent.git` |
| 7. Xcode Command Line Tools | 未修复 | 两个主安装入口均未加入明确安装与验证步骤 |
| 8. 安装路线与首次启动验收 | 未修复，平台范围更清楚 | 尚未拆分完整源码/DMG 安装路径，仍仅提示 Rust 编译耗时；Agent 指南版本仍为 0.2.7 |

### 本轮验证证据

从当前 `venv.rs` 提取原样的 `resource_path()` 和 `find_system_python3()`，在临时目录编译一个仅调用这两个函数的 Rust 验证程序，没有修改项目实现或用户环境。

1. 将程序放在临时 `.app/Contents/MacOS/`，将真实测试资源放在 `.app/Contents/Resources/resources/requirements-stt.txt`；从无源码的临时工作目录运行，移除子进程的 `CARGO_MANIFEST_DIR`。结果：资源实际存在，但函数返回 `src-tauri/resources/requirements-stt.txt`，`exists=false`。这验证了打包布局下的查找失败，不代表运行了完整 `.app`。
2. 在子进程 PATH 中放置一个模拟解释器，其 `--version` 输出 `Python 3.9.6` 并成功退出。结果：函数返回 `Ok("python3")`，没有拒绝低于文档要求的版本。此测试仅验证选择逻辑，不安装 Python 依赖。
3. 本轮未重复前端构建与 Rust 项目编译检查，因为本轮观察到的更新仅涉及文档和配置模板，没有对应实现变化；上一轮构建通过记录仍只适用于原有环境。本轮未进行全新 macOS 端到端安装。

请 Claude 优先处理第 1、2、4 项代码，再更新第 3、5、6、7、8 项文档与验收记录。若修复位于其他分支、工作树或尚未保存的编辑器内容中，需要先让修复进入本项目当前工作区，再进行复审。

## 修复状态（2026-11-04，Claude）

实测环境：macOS 26.6.2，Apple Silicon（arm64），Command Line Tools 27.0，`/usr/bin/python3` 3.9.6，rustc 1.95.0，Node 22.22.2。以下「隔离」指：临时 `HOME`（不碰真实 `~/.pocket-agent`）、工作目录 `/tmp`、不设 `CARGO_MANIFEST_DIR` 和 `STT_PYTHON`，直接运行新打包的 `Pocket Agent.app/Contents/MacOS/pocket-agent`。

| 原问题 | 状态 | 证据 |
| --- | --- | --- |
| 1. 打包资源路径错位 | 已修复，隔离实测通过 | `venv.rs::resource_path_from()` 先查 `Contents/Resources/resources/<file>`，再查旧布局 `Contents/Resources/<file>`，保留开发模式回退；单测 `bundled_resources_are_found_in_the_app`。`npm run tauri build` 后 `Resources/resources/` 下有 `requirements-stt.txt`、`stt-helper`、`stt-server.py`、`wake_kws.py`。隔离运行：从 `Resources/resources/requirements-stt.txt` 建 venv（CLT Python 3.9，cp39 轮子）→ stt-server 健康 → `/transcribe` 返回「你好 今天天气怎么样?」 |
| 2. 首次启动等待与失败恢复 | 已修复，隔离实测通过 | `stt.rs::reveal_main_window()` 在 venv 安装开始、`ensure_venv` 失败、服务脚本缺失 / spawn 失败、健康探测超 30 秒时显示窗口（手机接管时不显示，不发 `app-ready`）；超时后发 `voice-service-status {starting}` 并继续探测，直到健康、被重载取代或子进程退出（退出时发 `{failed, message}`，含恢复办法）。前端写进现有安装状态面板，并在这些状态下自动展开窗口（状态面板只在展开布局中显示）。`stt-server.py` 声纹模型下载失败 / 损坏改为 warning，下载用 `urlopen(timeout=30)`。实测：①全新 HOME 首次安装时窗口立即出现（CGWindowList 确认在屏）并显示进度；②`STT_MODEL=large-v3` + 空缓存，30 秒后面板显示「starting voice service」，就绪后面板清除、转写成功（medium 同样验证）；③`https_proxy=http://127.0.0.1:9` 模拟 PyPI 不通，窗口出现并显示 pip 的错误与「检查网络或代理后重启」；④代理阻断 HuggingFace 且删除声纹模型，服务照常启动、`/transcribe` 成功，`/speaker/embed` 返回明确错误、服务保持健康 |
| 3. Python 安装说明不一致 | 已修复（文档） | README / README_AGENTS 删除手动 pipx / pip，说明自动建 `~/.pocket-agent/venv`、需访问 PyPI 和 HuggingFace；`STT_PYTHON` 在 README、README_AGENTS、`.env.example` 中统一标为只影响运行时的高级覆盖。隔离实测确认默认路径不需要给系统 Python 装任何包 |
| 4. Python 选择不校验版本 | 已修复 | `find_system_python3()` 对每个候选执行 `import sys, venv` 并读版本，只接受 3.9–3.12；顺序 `/usr/bin/python3` → `python3` → Homebrew / `/usr/local` 的 `python3.{12,11,10}` → 其 `python3`。都不合格时错误列出每个候选的路径与版本（或「不存在」/「无法运行或缺少 venv 模块」）并提示 `xcode-select --install`，经 `venv-setup-error` 显示在窗口里。已有 venv 不重建。单测 `python_pick_skips_unsupported_versions`（旧版在前、全部不合格、`import venv` 失败） |
| 5. 后端从零配置不足 | Hermes 部分已修复（文档）；OpenClaw 不再支持 | README 新增「Set up Hermes from scratch」，README_AGENTS Step 4 同步：官方安装命令 → `hermes setup` → `~/.hermes/.env` 的 `API_SERVER_ENABLED` / `API_SERVER_KEY` → `hermes gateway run` 或 `install` + `start` → curl 流式请求验收 → PA `.env` key 一致、`API_AGENT` 留空；删除不存在的 `hermes serve`。命令与本机 Hermes v0.22.5 的 `hermes --help`、`hermes gateway --help` 及其 API Server 文档核对过。本次没有在新机器上从零安装 Hermes |
| 6. README 克隆占位符 | 已修复 | 统一为 `https://github.com/kevin-ping/pocket-agent.git`，后续 `cd pocket-agent` 连贯 |
| 7. Xcode Command Line Tools | 已修复（文档） | 两个安装入口都加入 `xcode-select --install` 与 `xcode-select -p`、`/usr/bin/python3 --version` 验证，并说明 CLT 同时提供 PA 默认使用的 Python |
| 8. 安装路线与首次启动验收 | 已修复（文档） | README 拆成「Route A — from source」和「Route B — .app / DMG」（DMG 只写自行 `npm run tauri build`、产物路径、本机架构、未签名未公证、`~/.pocket-agent/.env`）；终端 Input Monitoring 放进正常步骤；首次启动阶段表（编译 → venv 安装 → 模型下载 → 健康检查 → 就绪）列出联网站点和失败恢复，不写固定耗时；分层验收清单（文字 → 转写 → TTS → 打断 → 可选唤醒 / 声纹 → 可选电脑工具）。README_AGENTS 版本改为 0.3.0 |

### 待验收

- 真正的全新 Mac（新用户、从未装过 CLT、没有 Homebrew）上按文档从零安装：未执行。本次隔离只替换了 `HOME`，系统里已有 CLT、Rust、Node 和 Hermes。
- Intel Mac：未执行。
- macOS 12 / 13（< 13.4 时的 sherpa 兼容库路径）：未执行。
- 隔离环境下的真实麦克风、TTS 播放、打断与唤醒：未执行（转写用合成 wav 走 `/transcribe`）。
- 服务重载（`restart_when_idle`，只在 macOS < 13.4 触发）与手机接管时窗口不弹出：由单测和代码路径保证（`reveal_main_window` 与原 `show_main_window` 用同一个手机接管判断），没有在本次实机复现。

## 第三轮独立复审（2026-11-04，Codex）

**结论：本轮已有实质修复，上轮主要代码缺陷和安装文档缺口已得到处理；仍有以下 3 项需要收尾，且不能将当前开发机验证等同于真正的新机器端到端验收。** 本节是对上方 Claude 修复记录的独立代码复审，不代表重复执行了其全部隔离实机实验。

### 已确认的修复

- 资源解析增加 `Resources/resources/<filename>`，兼容旧的扁平目录；对应单测通过。
- 语音服务首次健康检查超时后会显示窗口、发送启动状态并继续探测；子进程退出会报告失败，对应单测通过。
- 声纹模型下载失败和文件损坏改为 warning，预加载异常也被捕获；下载增加了 socket 超时，不再直接终止基础语音服务。
- Python 选择检查版本和 `venv` 模块，明确当前范围为 3.9–3.12，并识别版本化 Homebrew 路径；对应单测通过。范围调整后不能继续用上一轮“3.9 低于文档要求”的结论评价当前实现。
- 安装文档已统一自动创建语音 venv，补齐 Hermes 初始化、API 配置、真实流式请求、CLT 安装、真实仓库地址、源码和打包路线、首次启动阶段及分层验收。
- Hermes API 配置与官方 API Server 文档中的环境变量、端口和请求格式一致：https://hermes-agent.nousresearch.com/docs/user-guide/features/api-server/ 。

### 剩余发现 A：[P2] 初始化状态仍可能在前端订阅前丢失

**位置**：`src-tauri/src/lib.rs` 的 `venv-bootstrap` 线程；`src-tauri/src/voice/venv.rs::ensure_venv()`；`src/App.svelte` 的 `onMount()`、`setupListeners()` 和 `venv-setup-progress` 回调。

Rust 在应用 `setup` 阶段就启动语音环境初始化并发送 `venv-setup-started` / `venv-setup-error`。前端则先等待 `is_app_ready`、加载设置和恢复窗口位置，再调用 `setupListeners()`。初始化状态仅通过事件传递，没有保存为可查询的快照供前端补读。

如果前端较慢，或“无合适 Python / 资源缺失”等失败很快，事件可能发生在订阅之前。此时虽然后台会显示主窗口，前端仍可能保持 `voiceSetupState='idle'`，无法展示具体错误。若只漏掉 `venv-setup-started` 而收到后续 progress，progress 回调虽更新 installing 状态，却没有调用 `showSetupStatus()`；窗口可能仍保持收起状态，安装阶段提示不可见。

这是根据真实初始化顺序确认的时序缺口；本轮未做带前端加载延迟的完整 GUI 复现，不应写成“每次必现”。

**建议修复**：后台保留最新安装/服务状态，提供查询接口；前端先订阅，再读取并同步状态，并处理读快照与新事件之间的顺序。progress 状态也应保证面板可见。不要仅移动前端订阅位置后就认为 Rust 发事件前必定已经完成订阅。

**验收**：人为延迟前端初始化，分别覆盖安装中、快速失败、服务已经就绪；前端完成加载后必须能恢复准确状态，并显示错误和恢复办法。

### 剩余发现 B：[P2] Agent 安装指南重复进入仓库目录

**位置**：`README_AGENTS.md` Step 2 和 Step 5。

Step 2 已 `cd pocket-agent`；构建验证中的 `cd src-tauri && cargo check`、`cd .. && npm run build` 最后仍停在仓库根目录。后续配置步骤没有离开该目录，Step 5 却再次执行 `cd pocket-agent`，顺序照抄会尝试进入不存在的 `pocket-agent/pocket-agent` 并报错。

**建议修复**：去掉 Step 5 多余的 `cd`，或明确新终端的起始位置并使用真实仓库路径。

**验收**：在同一个 shell 按文档顺序检查工作目录，Step 5 应可直接运行 `npm run tauri dev`。

### 剩余发现 C：[P2] README 的默认录音键仍互相矛盾

**位置**：`README.md` 的 Voice Pipeline、Configuration / Record Key 与 Verify Everything Works；`src-tauri/src/commands/config.rs` 的默认设置。

当前代码默认 `hotkey_code: 60`，安装验收步骤也写 `RightShift`，但 Voice Pipeline 和 Record Key 说明仍写默认 `fn`。新用户若从功能说明开始按 fn，可能误判权限或录音失效。

**建议修复**：统一首次安装默认键为 RightShift，并将 fn 说明为可配置键或历史版本行为；保留已有用户自定义键设置。

**验收**：全新设置下 UI 显示的录音键与文档默认键一致；搜索当前使用说明中的 `default: fn` 等旧描述。

### 本轮独立验证结果

| 验证 | 结果与限制 |
| --- | --- |
| `npm run build` | 通过，167 模块；仅证明当前环境前端可构建，不包含 Svelte 类型检查 |
| `cargo test --locked --offline --lib` | 沙箱内 128 通过、4 项本地测试端口绑定报 `Operation not permitted`；获准在沙箱外重跑后 **132 通过、0 失败**。这 4 项不应计为产品回归 |
| 新增资源/Python/服务探测测试 | 包含于上述 132 项，通过 |
| 本地打包资源检查 | 当前 `.app/Contents/Resources/resources/` 已包含四个语音资源；本轮没有重新打包或运行该 `.app` |
| 真正全新 Mac、Intel、旧版 macOS、真实录音/TTS/唤醒 | 本轮未执行，继续保留为待验收 |

本轮只追加本审核记录，未修改产品实现或用户配置。
