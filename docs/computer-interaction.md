# Mac 电脑交互：每台电脑独立安装

Pocket Agent 的受支持部署方式是 **Pocket Agent、Hermes 和 CuaDriver 安装在同一台 Mac**。Pocket Agent 提供语音/文字入口及审批界面，Hermes 执行电脑工具，CuaDriver 读取和操作这台 Mac。安装 Pocket Agent 本身不会自动安装或配置 Hermes，也不会继承另一台电脑的模型密钥、工具设置或 macOS 权限。

## 新电脑安装顺序

1. 安装 Hermes，先确保文字对话可用，配置自己可用的主模型。这里的电脑接入已在 Hermes 0.21.3、CuaDriver 0.28.2 上验证；程序以实际接口能力判断，不仅比较版本号。
2. 安装驱动：`hermes computer-use install`。运行 `hermes computer-use doctor` 检查。驱动安装/下载由 Hermes 管理，Pocket Agent 不静默运行远程安装脚本。
3. **为网关正在使用的 profile 和 api_server 平台启用工具**。只给 cli 启用不够：

   ```sh
   hermes --profile YOUR_PROFILE tools enable computer_use vision --platform api_server
   ```

   未使用 profile 的安装省略 `--profile YOUR_PROFILE`。不要直接照抄其他电脑的 profile 名称。先在 Hermes 配好 API 网关，再启动网关（已有网关无需另起一个）。检查 `hermes tools list --platform api_server`；使用 profile 时同样加上 `--profile` 参数。

4. macOS「系统设置 → 隐私与安全性」中，为 **CuaDriver** 授予辅助功能权限（读取界面、输入）和屏幕录制权限（截图）。可运行 `cua-driver permissions grant` 打开其权限流程；若终端找不到命令，通常为 `~/.local/bin/cua-driver`。权限绑定实际驱动身份，不是给 Pocket Agent 勾选后就全部满足。Pocket Agent 自己的麦克风/热键权限仍按语音安装文档处理。
5. 为 Pocket Agent 配置本机 Hermes 地址和匹配的认证密钥。打包应用读取 `~/.pocket-agent/.env`，开发模式也可读取项目 `.env`：

   ```dotenv
   API_SERVER=http://localhost:8642
   API_SERVER_KEY=YOUR_GATEWAY_KEY
   API_AGENT=
   ```

   网关端的 API 服务、端口与认证必须先配置正确。不要把密钥写入项目提交。电脑工具没有通过 Pocket Agent 的 8650 推送端口暴露；不需要为本功能开放新的网络端口。
6. 启动 Pocket Agent，展开聊天框，看底部状态条上的 **电脑工具** 状态灯（绿=可用，灰=不可用，鼠标悬停显示说明；展开聊天框和保存设置时会重新检查）。检查不会截图、读取窗口正文或申请系统权限。它依次核对 `/v1/capabilities` 的任务/审批/停止支持、`/v1/toolsets` 的电脑工具启用及配置状态，并在本机常见安装路径探测驱动权限。
7. 首先用语音说「看看当前窗口，告诉我正在看什么，只查看」。成功后再试「帮我在浏览器搜索……」。若正在用其他应用语音通话，稍后再测麦克风/唤醒/播报。

## 主模型可以只有文字能力

优先读取辅助功能（AX）文本，不需要把图片交给主模型。图片、画布或无有效 AX 文本的界面需要截图时，Hermes 的 `computer_use` 可以将分析交给 `auxiliary.vision`，再把文字结果给主模型。

按所用 Hermes 版本配置独立视觉提供商/模型及其凭据。`vision` 工具处于 enabled/configured 状态**不证明视觉请求已成功**。先用 Hermes 实测视觉模型；此版本 Pocket Agent 不代管视觉密钥，不自动换主模型，也不把缺少视觉服务隐藏成“已看懂”。如果只需要可访问的界面文字，可以先不做截图测试。

## 实际运行与权限边界

- PA 只做传话筒(2026-10-28 起，见 decisions):提示词只告诉 Hermes 三件事——PA 是同机的语音前端、回复会被 TTS 念出;用户说的「电脑 / 浏览器 / 应用」指这台 Mac 上看得见的真实应用(`browser_exec` / `browser_*` 是用户看不见的无头浏览器);用什么工具、怎么做由 Hermes 自己决定，完成后用一句话说结果或阻碍。PA 不规定前台/后台投递、观察次数或失败后是否换方法，也不做逐轮分类或按路由拦截工具。
- 隐藏、最小化或未启动的应用：由 Hermes 自行处理。`computer_use` 本身不能启动或恢复窗口(见下节),但 Hermes 可以改用 `terminal` 执行 `osascript` / `open -a`(Hermes 自带的 macOS `open` 前置窗口步骤见 `tools/terminal_tool_macos_open.py`),和 Hermes Desktop 的行为一致。是否需要审批，由 Hermes 的审批策略决定。
- 普通聊天、头脑风暴和知识解释不需要桌面访问;没有持续屏幕监视，也没有定时抓屏。何时读取界面由 Hermes 判断。
- 本机且能力齐备时使用 Hermes `/v1/runs`，通过事件流显示回复/工具进度。具体操作要求审批时，弹窗显示 Hermes 的操作说明，并原样提供 Hermes 本次给出的选项(本次允许 / 本会话允许 / 始终允许 / 拒绝)。PA 只接受本次请求提供的选项，同一请求不能重复提交，也不能用于其他任务。Hermes 自己的审批策略决定是否弹窗(例如 `approvals.mode: off` 时不弹)。部分完成的回复会正常播报，包括 `run.failed` 中标记 `partial=true` 或 `completed=false`、无实际错误且未中断的结果；只有完全没有回复时才提示结果未确认。带实际错误的失败、取消和中断仍报错，不会因之前流式输出过文字而当作正常结束。
- 窗口和网页内容只是资料，不能成为操作授权;实际的授权由 Hermes 的审批把关,PA 没有独立沙箱。
- 打断请求会先撤销本地待审批请求，再调用后台停止接口；取消后的迟到审批事件和发送失败回调不能重新启用授权。停止只能阻止后续工作，不能撤销已执行动作。如果停止失败或连接丢失，会提示后台状态不确定，不应盲目重复点击/输入。审批只有在明确连接建立失败、且任务未取消时才允许重新提交；响应丢失或 HTTP 错误不会重新放回待审批列表，避免重复批准可能已执行的操作。创建任务的响应丢失时也不会自动重发。
- Pocket Agent 所有 UI、远程网关和 bridge 路径都会过滤旧 `[CMD:...]` / `[LOCAL_CMD ...]` 标签，不会执行其中的 shell、`open`、AppleScript 或其他本机命令，也不会在界面中伪装为“正在运行命令”。
- 点击 Pocket Agent 可能使其成为前台。说「当前窗口」时可能指向 Pocket Agent 自己，由 Hermes 判断是否追问。可以直接说「查看 Safari 当前页面」或在目标应用保持前台时用语音请求。

## 未启动 / 无窗口的应用

`computer_use` **自身无法冷启动或恢复一个没有在屏窗口的应用**，有以下四条机制约束（均已在本机 Hermes + CuaDriver 0.28.2 上核实）。2026-10-28 起 PA 不再禁止 Hermes 用 `terminal`(`osascript` / `open -a`)绕过这些限制，由 Hermes 自行选择。

- **`focus_app` 只是在屏窗口选择器，故意不启动应用。** `~/.hermes/hermes-agent/tools/computer_use/cua_backend_capture.py:357-379` 在 `list_windows(on_screen_only=True)` 里匹配，匹配不到直接返回 `No on-screen window found for app 'X'.`，不回退到 launch。应用 `running=true` 但零窗口时，重试它必然反复失败。
- **没有 sticky target 就投递不出任何按键。** `cua_backend_input.py:11,29` 要求已捕获的活动窗口，否则一律 `No active window — call capture() first.`。所以「先按 cmd+n 开个窗口」这类思路在无窗口状态下不成立。
- **Dock 无法被捕获。** `list_windows` 只返回 layer-0 顶层窗口，Dock 不在其中；`list_apps` 又明确过滤掉系统 UI agent。`capture()` 的应用解析（`cua_backend_capture.py:165-196,228`）只在这两个来源里匹配，因此 `capture(app='Dock')` 必然返回 `no on-screen window matched`。macOS 上 `capture(app='desktop')` 则优先解析到 Finder 的桌面窗口而非 Dock（`cua_backend_capture.py:31-33`）。
- **`launch_app` 存在但模型不可达。** CuaDriver 本身有 `launch_app(bundle_id/name, urls=[...])`（`cua_backend.py:319-331`，CLI `list-tools` 也列出它），但它既不在 `computer_use` 的 action 枚举（`schema.py:20-35`）也不在 dispatch 表（`tool.py:398-421`）里。因此在 `computer_use` 内部没有冷启动路径;Hermes 可以改用 `terminal`。

### 定稿阶梯与实测结论

以下是 2026-09-30 对 `computer_use` 内部启动路径的实测记录(当时 PA 提示词禁止 terminal),仅作参考：

| 级别 | 方案 | 实测结论 |
| --- | --- | --- |
| ① | `capture(app='<AppName>', mode='ax')` | **可用，保留。** 应用有在屏窗口时直接进入使用阶段。 |
| ② | `capture(app='Dock', mode='som')` → `click(element=N)` | **不可用，已删除。** 实测 `list_windows` 返回的 88 个窗口全为 layer-0 且无任何 Dock 条目；Dock 也被 `list_apps` 过滤。解析必然失败。 |
| ③ | Finder 建 sticky target → `key cmd+space` (`delivery_mode='foreground'`) 唤 Spotlight | **不可用，已删除。** 驱动 0.28.2 接受 `foreground`（不报 `foreground_unsupported`）并返回 `route: global_input` / `effect: unverifiable`，但两次实测（目标分别为 Finder 与当前前台 iTerm2）Spotlight 均未出现。本机 hotkey 64 未改绑，走系统默认 Cmd+Space，排除绑定干扰。 |
| ④ | 全败即停 | **唯一失败出口。** 一句话说明应用在运行但无窗口、本端无法打开，附工具返回的真实错误文本，请用户手动打开。 |

2026-10-28 起，提示词不再包含「无窗口即停」和观察次数上限，这些判断交给 Hermes。

在 `computer_use` 内部根治，需要 Hermes 把 `launch_app` 暴露为 action，记于 `.auto-work/roadmap.md` 的 `ROAD-CU1`。

## 检查结果的含义

| 提示 | 处理 |
| --- | --- |
| 无法连接 | 启动同机网关，核对端口、地址和 API 密钥。 |
| 能力接口不可用/缺少任务、审批、停止接口 | 升级对应 Hermes 网关；普通对话仍走原聊天接口。 |
| api_server 未启用 computer_use | 对网关实际 profile 运行 tools enable，重新点击检查。 |
| 工具未配置/常见路径没有驱动 | 运行 computer-use install/doctor；使用自定义路径时核对 Hermes 的驱动配置。PA 的本机探测不解析其他进程的环境配置。 |
| 辅助功能/屏幕录制未知 | 不是拒绝。驱动自己身份的守护进程可能未运行，或探测不可用；运行驱动权限流程，之后重查。 |
| 权限未授予 | 在系统设置为实际 CuaDriver 身份授权。AX 文字读取与截图依赖不同能力，应以实际调用结果判断。 |
| 视觉分析失败 | 核对 Hermes auxiliary.vision 和模型凭据，不能把工具 enabled 当作模型已验证。 |

远程地址及 OpenClaw 等已有聊天兼容性保留，但 **PA 的本机 Computer Use 仅在本机 Hermes UI 路径生效**。配置 `API_AGENT` 的 OpenClaw、远程网关和 bridge 不会被当作用户本机桌面入口，也不会回退到旧命令执行。电脑工具操作驱动所在的机器，远程 Hermes 不会自动看到 PA 客户端。SSH 转发、代理或远程驱动即使用 localhost 地址，也不证明是同机；当前检测不是主机身份认证，请按同机方式部署。本功能不新建远程控制服务。

当前 Hermes `/v1/runs` 接口没有每回合工具白名单参数，PA 也不在客户端按路由停止 run；工具选择由提示词引导，执行前的把关依赖 Hermes 自身审批。PA 不修改用户的 Hermes 全局 toolset 配置。

## 验证记录（2026-09-19）

- 项目：前端生产构建、Rust 单元测试；具体测试数量以交付报告为准。
- 本机环境：Hermes 0.21.3，CuaDriver 0.28.2；为现有 xingli profile 的 api_server 启用 computer_use。这是该电脑的配置变更，不包含在可分发应用中。
- 通过 PA 相同 Hermes API 和桌面提示：一次 AX 当前窗口读取成功；新 `/v1/runs` 中「读取 → 设置同一应用后台目标（不切前台）→ 再读」成功。
- 用户指定浏览器验收：真实 Chrome 导航 Apple Newsroom 并读取新闻稿；首次因多窗口/地址栏焦点发生多次失败。收窄流程后，新标签页中 cmd+l → 输入完整 Google 搜索 URL → 回车 → 等待 → AX 重读成功，读到 Apple 官方搜索结果。完整官方文章 URL 另经网页核验补齐，没有把该网页核验算成电脑操作测试。
- 浏览器自动化依然依赖 Hermes 模型与驱动的目标定位能力。提示词现在要求保留旧标签、全选地址后输入、同一步两次失败即停；这是交互策略，不能保证所有界面可靠。
- 不等于完整 Pocket Agent UI/麦克风/唤醒/TTS 端到端实测。截图辅助视觉、真实审批弹窗交互和换机安装需各自验收；不能从 AX 成功推断它们通过。
