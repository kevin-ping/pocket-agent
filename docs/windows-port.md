# PA Windows 移植设计(第一阶段)

> 给 Windows 机上的 Claude Code:`.auto-work/` 不进 git,所以这份文档是交接的唯一来源。开始前:
> 1. 把文末「auto-work 条目」里的 bugs 部分写进 `.auto-work/bugs.md`,features 部分写进 `.auto-work/features.md`(文件头分别是 `# Bugs` / `# Features`)。
> 2. 运行 `auto-work`。
> 3. 文中 `file:line` 是 2026-11-05 在 Mac 上盘点时的行号，动手前先核对。

## Context
用户会准备一台 Windows 机器并在上面原生安装 Hermes(Hermes README 写明原生 Windows 完全支持,`computer_use`/cua-driver 也有 Windows 分支)。需要一套能配合**本机 Windows Hermes** 的 PA。PA 目前只支持 macOS(README.md:9)。三路只读盘点结论:Windows 下**编译不通过**(4 处),另有十余处运行时会坏(HOME、/tmp、venv 布局、资源路径、控制台窗口、热键键码等)。

关于 agent-desktop:Windows 上 `computer_use` 已能用，第一阶段**不接 agent-desktop**;它保持「可选第二后端」,等实机对比后再定(Mac 上实测 agent-desktop 比 computer_use 慢一倍)。

已定:
- 分阶段：本批 = 能编译 + 文字对话 + 热键录音转写 + TTS 播报 + 桌面操作提示词，在 `npm run tauri dev` 下跑通。第二批 = 唤醒词/声纹/连续对话/安装包(MSI/NSIS)/文档。
- 开发方式：在 Windows 机上直接用 Claude Code 开发、编译、实测。第一批随 0.3.1 发布(tag `v0.3.1`);之后的改动放在当前 release 分支(现为 `release/0.3.3`),不另开分支。Windows 端 pull `release/0.3.3` 后运行 `auto-work`。

## 本批改动(按依赖顺序)

### 1. 能编译(BUG-WIN1)
- `src-tauri/src/lib.rs:196-197`:`start_capture`/`poll_capture` 只在 macOS 存在却无条件注册。给非 macOS 提供同名实现(见第 5 步热键)，不做 cfg 分叉注册，前端 `SettingsApp.svelte:292,300` 无需改。
- `voice/hotkey.rs:536-537`:`keycode_name()` 非 macOS 缺失 → 补跨平台版本。
- `voice/hotkey.rs:585`:`key as i64` 不能编译 → 由第 5 步替换。
- 测试:`voice/venv.rs:478,483,515,654` 的 `PermissionsExt` 加 `#[cfg(unix)]`;`stt.rs:825,838,1035` 用到 `/bin/sh` `/bin/kill` 的测试加 `#[cfg(unix)]`。
- `lib.rs:94-108` `app.set_menu` 加 `#[cfg(target_os = "macos")]`(否则每个窗口出现菜单栏)。
- 删除 `src-tauri/.cargo/config.toml` 里的 `SHERPA_ONNX_LIB_DIR`(写死 Mac 绝对路径、无 crate 使用)。

### 2. 路径(BUG-WIN2)
- `HOME` → `dirs::home_dir()`:`voice/venv.rs:14`、`commands/config.rs:149`、`commands/chat.rs:1242`。数据目录保持 `~/.pocket-agent`(Python 侧 `Path.home()` 一致，不改到 `%APPDATA%`)。
- `/tmp` → `std::env::temp_dir()`:`commands/chat.rs:613-614`、`voice/conversation.rs:678`。
- venv 布局：在 `venv.rs` 加一个 `venv_bin(name)` 辅助(Windows → `Scripts\<name>.exe`,其他 → `bin/<name>`),替换 `venv.rs:22,40,95,120,382` 与 `chat.rs:410` 的 edge-tts 路径。
- Python 候选(`venv.rs:198-212`):Windows 用 `py -3.12/-3.11/-3.10`、`%LOCALAPPDATA%\Programs\Python\Python31x\python.exe`、`python`;跳过 `WindowsApps` 下的商店占位程序。错误提示改为 winget/python.org。
- 资源路径(`venv.rs:171-196`):打包后改用 Tauri `resource_dir()`;dev 模式保留 `CARGO_MANIFEST_DIR`。
- `config.rs:107-114` `is_app_bundle_exe`:Windows 判断「不在 target\debug|release 下」视为安装版(第二批装包时才真正用到，本批只保证不误判 dev)。
- `stt.rs:282` stt-helper 回退：用 venv 的 python 执行，不依赖 shebang。

### 3. 子进程不弹黑窗(BUG-WIN3)
- 新增一个 `fn no_window(cmd: &mut Command)`(Windows 设 `CREATE_NO_WINDOW` 0x08000000,其他平台空操作),在 `stt.rs:461`、`venv.rs:64,75,221` 的 pip/探测、`chat.rs:668` edge-tts 处调用。

### 4. stt-server 旧进程(BUG-WIN4)
- `stt.rs:570-605`:Windows 用 `netstat -ano` 找 8651 监听 pid(按端口和状态列解析，不匹配英文 `LISTENING`,避免本地化问题)+ `taskkill /PID <pid>`;仍保留「`/health` 自报 pid 与监听 pid 双重确认」的原规则(decision-20261105-074702)。错误提示文字随平台变化。

### 5. 全局热键(ENH-WIN5)
- 非 macOS 的 `rdev_impl`(`hotkey.rs:560-613`)只处理按下、不处理松开/修饰键/双击/Esc/捕获。改为：用 `rdev::listen` 同时处理 KeyPress/KeyRelease,实现按住说话、双击模式、Esc 取消、设置页按键捕获(`start_capture`/`poll_capture` 的非 macOS 版)。
- 键码：设置里存的是 macOS 虚拟键码(默认 60=RightShift)。加一张 macOS 键码 ↔ `rdev::Key` 映射表，存储格式不变(保持与已有设置文件兼容);`keycode_name()` 共用此表。`settings.ts:74` 默认值不改。
- 设置页在 Windows 隐藏 `fn` 键选项(Windows 看不到 fn)。
- 已知限制写进文档：管理员权限窗口在前台时，非提权的钩子收不到按键。

### 6. 麦克风(ENH-WIN6)
- `record.rs:270-321`:Windows 没有授权查询 API,保持「未确定」;流打开失败或连续静音时，提示「设置 → 隐私和安全性 → 麦克风 → 允许桌面应用访问」。macOS 文案不动。

### 7. 桌面操作提示词按平台(ENH-WIN7)
- `commands/computer.rs:6-14` `COMPUTER_HINT` 的「same Mac / this Mac」改为按编译平台填「Mac / Windows PC」,同步测试 `computer.rs:210,212`;`chat.rs:1216`、`computer.rs:70` 同样处理。
- `computer.rs:126-166` `permission_note()`:Windows 不查 macOS 路径，返回「桌面能力由 Hermes computer_use 提供」;`env_clear` 后在 Windows 需补 `SystemRoot`/`PATH`(或 Windows 直接跳过该探测)。
- `skills/pocket-agent/SKILL.md:10,22,27` 与 `HERMES.md:67`:「same Mac」改中性;`open <path>` 补 Windows 写法 `start "" <path>`。
- `history.rs:580-602` Windows 分支改成 `cmd /C start "" "<path>"`(路径含空格时会坏)。
- PA 只传话：只改平台措辞，不替 Hermes 判断用哪个工具。

### 8. Python 侧(BUG-WIN8)
- `resources/wake_kws.py:226-231`:`NamedTemporaryFile` 打开时被原生库再次打开，Windows 会 Permission denied → `delete=False` + 关闭 + `finally` 删除。(唤醒词本身第二批验收，但这个 bug 修起来便宜，先修。)
- `resources/stt-server.py:516`:`rename` → `os.replace`。
- `stt-server.py:889` 的 brew 提示按平台给文案。

### 9. 前端小项(ENH-WIN9)
- `SettingsApp.svelte:162` `⌘` 图标 → `⚙`。
- 字体栈加 `"Microsoft YaHei UI"`(`App.svelte:1476`、`SettingsApp.svelte:663`)。
- `layout.ts:37-44` 与 `App.svelte:1242-1250`:贴边判断用 `workArea.position.x + width`(任务栏在左/多显示器时才对)。

### 10. 开发脚本(BUG-WIN10)
- `scripts/clear-dev-port.js:39` 不匹配英文 `LISTENING`,改按状态列/端口解析。

## 第一批实施记录(2026-11-06,Windows 10 机上)
未用 auto-work,本会话按上面顺序直接实现。与设计的差异和新发现:
- **Hermes home**:Windows 上 Hermes 装在 `%LOCALAPPDATA%\hermes`,不是 `~/.hermes`。新增 `platform::hermes_home()`(优先 `HERMES_HOME`),用于 pa-summaries 路径和 401 提示;HERMES.md 补了 Windows 技能目录。
- **cargo test 起不来**:测试 exe 链接了 tauri,缺 Common Controls v6 manifest 时报 `STATUS_ENTRYPOINT_NOT_FOUND`。`build.rs` 改为自己给所有目标嵌入 `windows-app-manifest.xml`(内容同 tauri-build 默认),并让 tauri-build 不再嵌入。
- **端口探测**:Windows 上连接本机未监听端口约 2 秒后才被拒绝，原 2 秒 HTTP 超时会把「空闲」误判成「被占用」。超时后再用直接 TCP 连接确认。
- **Python 子进程编码**:管道默认是 ANSI 代码页，中文会出错;所有 Python 子进程设 `PYTHONUTF8=1`。
- **黑窗**:`no_window` 只在 PA 自己没有控制台时设 `CREATE_NO_WINDOW`;dev 模式下子进程共用终端，不弹窗且日志仍可见。
- **热键语义**:与 macOS 一致(按一下开始、再按一下结束，不是按住说话);忽略 Windows 的按键自动重复。设置页本来就没有 fn 选项，无需隐藏。
- **麦克风**:Windows 隐私设置挡住时可能是打开失败，也可能录到全零;两种情况都给出设置路径。
- 提交按条目拆分，但 WIN1–WIN5 的 Rust 改动交织，合并为一个提交。

实机运行后又发现并修复(均已提交):
- **启动器进程**:venv 的 `python.exe` 和 pip 生成的 `edge-tts.exe` 在 Windows 上是启动器，真正的解释器是它的子进程。`Child::kill` 只杀启动器，stt-server 会变孤儿 → `platform::kill_tree`(`taskkill /T /F`)。
- **8650 被子进程继承**:tokio 的监听 socket 在 Windows 上可继承,PA 被强杀后 stt-server 一直占着 8650,下次启动 API 绑不上 → 绑定后 `SetHandleInformation` 去掉继承标志。
- **VAD 全部失效**(macOS 新装同样受影响):silero-vad 6 不再带 torchaudio,`read_audio` 报错 → 用服务自己的 WAV 读取器。
- **唤醒词「仅本人」核对失败**:`_kws_verify` 的 `NamedTemporaryFile` 打开状态下被再次打开 → 先关闭、用完删除。
- **静音录音**:默认输入设备是 Steam Streaming Microphone(纯零)。声纹录制报成通用的「唤醒服务出错」→ 新错误码 `wake_mic_silent`;麦克风提示按平台给设置路径。
- **头像模糊**:DPR 1 的屏幕上画布只有 96 物理像素 → DPR < 2 时按 2/DPR 倍布局再缩回。
- **dev 模式改 `resources/` 不生效**:运行的是构建时复制到 `target\debug\resources` 的脚本，改 Python 后需要触发一次 Rust 重新构建。

实测结果(Windows 10 22H2,Hermes v0.22.7):`cargo test` 153 项通过;`npm run tauri dev` 启动、建 venv、语音服务 `/health`;文字对话经本机 Hermes 返回并播报;中文转写准确;热键录音与转写;唤醒词与声纹录制、测试;`computer_use` 启用后 doctor 全绿;强杀 PA 后重开，旧 stt-server 被替换、8650 正常绑定。未测：手机终端连 Windows、Mac 回归。

安装文档：[docs/windows-install.md](windows-install.md)。

## 第二批(本批不做，写入 roadmap)

新用户装机时的问题大多在配置和环境层面，用户身边没有大模型帮忙排查，所以第二批优先做:
- **锁定语音环境依赖版本**(两个平台分别验证过的锁定清单)，避免 silero-vad 这类上游变化。
- **CI**:Windows 和 macOS runner 上从零建 venv,实际调用 `/transcribe`、`/vad` 和唤醒词核对。
- **自动准备 Python**(如 uv 下载固定版本),不依赖用户机器上已有的 Python。
- **体检页**:检查 Hermes 是否可达、key 是否匹配、API 是否开启、`computer_use` 是否启用、技能是否安装、麦克风电平、语音服务、唤醒模型;能修的给一键修复。
- **PA 内选麦克风**并显示实时电平;识别虚拟设备。
- **日志落文件 +「导出诊断」按钮**。
- 手机终端：防火墙只对专用网络放行 8650。

原计划项:
唤醒词与声纹实机验收、连续对话、`tauri.conf.json` 加 `bundle.windows`(WebView2 安装方式、NSIS)、托盘彩色图标、日志落文件(GUI 无控制台)、Windows 防火墙放行 8650(手机终端)、README/README_AGENTS/HERMES 的 Windows 安装章节、`silero-vad` 拉进 torch 导致安装体积大的问题、agent-desktop 与 computer_use 在 Windows 上的对照实测。

## 关键文件
`src-tauri/src/lib.rs`、`voice/hotkey.rs`、`voice/venv.rs`、`voice/stt.rs`、`voice/record.rs`、`voice/conversation.rs`、`commands/config.rs`、`commands/chat.rs`、`commands/computer.rs`、`commands/history.rs`、`resources/wake_kws.py`、`resources/stt-server.py`、`src/SettingsApp.svelte`、`src/lib/stores/layout.ts`、`scripts/clear-dev-port.js`、`skills/pocket-agent/SKILL.md`。

## 验证
1. Windows 前置：装好 Hermes(网关 8642)、Rust(MSVC)、Node LTS、Python 3.12(python.org 或 winget)、VS Build Tools「使用 C++ 的桌面开发」、WebView2;clone 仓库并 checkout `release/0.3.3`。
2. 回 Mac 验收不回退：每批推送前在 Mac 上跑 `cargo test`、`npm run check`、`npm run build`,并手测热键和语音一轮(cfg 分叉容易误伤 macOS)。
3. Windows 上(Claude Code 直接执行 + 用户在机器前操作):
   - `cargo test`(Windows 上)通过。
   - `npm run tauri dev` 启动：不弹黑窗;venv 建在 `%USERPROFILE%\.pocket-agent\venv`,stt-server 在 8651 起来;`curl http://127.0.0.1:8651/health` 返回 pid。
   - 文字对话经本机 Hermes(8642)返回并播报语音。
   - 按住 RightShift 录音 → 松开出转写;Esc 取消;设置页按键捕获能记录新键;双击模式可用。
   - 「打开记事本写一句话」:Hermes 走 computer_use 完成，提示词里出现 Windows 措辞。
   - 结束 PA 后重开:8651 上的旧 stt-server 被识别并替换。
4. 流程:Mac 已把本文件提交到 `release/0.3.3`。Windows 上按开头三步把条目写进 `.auto-work/` 后运行 `auto-work`。

## auto-work 条目

### bugs(写入 `.auto-work/bugs.md`)

#### [BUG-WIN1] Windows 下编译不通过

**Design:** `docs/windows-port.md`(第 1 节)

- `lib.rs:196-197` 无条件注册仅 macOS 存在的 `start_capture`/`poll_capture`;`hotkey.rs:536-537` 调用仅 macOS 的 `keycode_name()`;`hotkey.rs:585` `key as i64` 无法编译;`venv.rs`/`stt.rs` 测试用了 Unix 专属 API。
- `lib.rs:94-108` `app.set_menu` 只在 macOS 执行;删除 `src-tauri/.cargo/config.toml` 里写死 Mac 路径的 `SHERPA_ONNX_LIB_DIR`。

验收:Windows 上 `cargo build` 与 `cargo test` 通过;Mac 上 `cargo test` 仍通过;Windows 窗口不出现菜单栏。

#### [BUG-WIN2] Windows 路径：HOME、/tmp、venv 布局、Python 查找、资源目录

**Design:** `docs/windows-port.md`(第 2 节)

- `HOME` → `dirs::home_dir()`(`venv.rs:14`、`config.rs:149`、`chat.rs:1242`);`/tmp` → `std::env::temp_dir()`(`chat.rs:613-614`、`conversation.rs:678`)。
- venv 用 `venv_bin(name)`:Windows `Scripts\<name>.exe`,其他 `bin/<name>`(含 edge-tts,`chat.rs:410`)。
- Windows Python 候选:`py -3.12/-3.11/-3.10`、`%LOCALAPPDATA%\Programs\Python\Python31x\python.exe`、`python`;跳过 `WindowsApps` 商店占位程序;提示文案改 winget/python.org。
- 打包后资源用 Tauri `resource_dir()`;`is_app_bundle_exe` 不误判 Windows dev;stt-helper 回退用 venv python 执行。

验收:Windows 上首次启动在 `%USERPROFILE%\.pocket-agent\venv` 建好 venv,`~/.pocket-agent/.env` 被读到;语音播报和转写的临时文件落在系统临时目录;Mac 行为不变。

#### [BUG-WIN3] Windows 下每个子进程都弹黑色控制台窗口

**Design:** `docs/windows-port.md`(第 3 节)

新增 `no_window(cmd)`(Windows 设 `CREATE_NO_WINDOW`),用于 stt-server、pip/探测、edge-tts。

验收:`npm run tauri dev` 启动、建 venv、对话播报全程不出现控制台窗口。

#### [BUG-WIN4] Windows 下无法识别和替换 8651 上的旧 stt-server

**Design:** `docs/windows-port.md`(第 4 节)

`stt.rs:570-605` 依赖 `lsof` 和 `/bin/kill`。Windows 用 `netstat -ano` 按端口和状态列找监听 pid(不匹配英文 `LISTENING`),`taskkill /PID` 结束;保留「/health 自报 pid 与监听 pid 双重确认」(decision-20261105-074702)。错误提示按平台给命令。

验收:Windows 上结束 PA(留下孤儿 stt-server)后重开，旧进程被识别并替换，新 `/health` 的 pid 变化。

#### [BUG-WIN8] Python 侧 Windows 兼容

**Design:** `docs/windows-port.md`(第 8 节)

- `wake_kws.py:226-231`:`NamedTemporaryFile` 打开状态下被原生库再次打开，Windows 报 Permission denied → `delete=False` + 关闭 + `finally` 删除。
- `stt-server.py:516` `rename` → `os.replace`;`stt-server.py:889` brew 提示按平台给文案。

验收:Windows 上 `python scripts/test_wake_kws.py` 通过;stt-server 模型下载覆盖已有文件不报错。

#### [BUG-WIN10] clear-dev-port.js 在非英文 Windows 上误判端口空闲

**Design:** `docs/windows-port.md`(第 10 节)

`scripts/clear-dev-port.js:39` 匹配英文 `LISTENING`,改按端口与状态列解析。

验收:Windows(含中文系统)上 1420 被占用时脚本报出占用 pid;相应测试通过。

### features(写入 `.auto-work/features.md`)

#### [ENH-WIN5] Windows 全局热键：按住说话、松开、双击、Esc、按键捕获

**Design:** `docs/windows-port.md`(第 5 节)

非 macOS 的 `rdev_impl`(`hotkey.rs:560-613`)只处理按下。改为 `rdev::listen` 同时处理 KeyPress/KeyRelease,实现按住说话、双击模式、Esc 取消，以及设置页按键捕获(`start_capture`/`poll_capture` 的非 macOS 版)。设置里仍存 macOS 键码(默认 60=RightShift),加一张 macOS 键码 ↔ `rdev::Key` 映射表，`keycode_name()` 共用。Windows 设置页隐藏 fn 键选项。文档注明：管理员权限窗口在前台时收不到按键。

验收:Windows 上按住 RightShift 录音、松开出转写;Esc 取消;双击模式可用;设置页能捕获并保存新键，重启后生效;Mac 热键行为不变。

#### [ENH-WIN6] Windows 麦克风被系统隐私设置挡住时给出正确提示

**Design:** `docs/windows-port.md`(第 6 节)

`record.rs:270-321`:Windows 无授权查询 API,保持「未确定」;流打开失败或持续静音时提示「设置 → 隐私和安全性 → 麦克风 → 允许桌面应用访问你的麦克风」。macOS 文案不动。

验收:Windows 关闭「允许桌面应用访问麦克风」后按热键，界面出现上述提示;打开后录音正常。

#### [ENH-WIN7] 桌面操作提示词和技能文案按平台(Mac / Windows PC)

**Design:** `docs/windows-port.md`(第 7 节)

- `computer.rs:6-14` `COMPUTER_HINT` 和 `chat.rs:1216`、`computer.rs:70` 的「Mac」按编译平台替换，同步测试 `computer.rs:210,212`。
- `computer.rs:126-166` `permission_note()` 在 Windows 不查 macOS 路径。
- `skills/pocket-agent/SKILL.md`、`HERMES.md:67` 的「same Mac」改中性;`open <path>` 补 Windows 写法 `start "" <path>`。
- `history.rs:580-602` Windows 分支改为 `cmd /C start "" "<path>"`。
- 只改平台措辞，不替 Hermes 做工具判断(PA 只传话)。

验收:Windows 上「打开记事本写一句话」经本机 Hermes 的 computer_use 完成;发给 Hermes 的提示词里是 Windows 措辞;路径含空格的历史页能打开;Mac 测试通过。

#### [ENH-WIN9] 前端 Windows 小项

**Design:** `docs/windows-port.md`(第 9 节)

`SettingsApp.svelte:162` 的 `⌘` 图标改 `⚙`;字体栈加 `"Microsoft YaHei UI"`;`layout.ts:37-44` 与 `App.svelte:1242-1250` 贴边判断改用 `workArea.position.x + width`。

验收:Windows 上中文显示为雅黑;任务栏放左侧时窗口贴右边判断正确;`npm run check` 通过。
