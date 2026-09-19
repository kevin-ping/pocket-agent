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
6. 启动 Pocket Agent，展开聊天框，点击 **ⓘ 检查电脑工具**。检查不会截图、读取窗口正文或申请系统权限。它依次核对 `/v1/capabilities` 的任务/审批/停止支持、`/v1/toolsets` 的电脑工具启用及配置状态，并在本机常见安装路径探测驱动权限。
7. 首先用文字输入「看看当前窗口，告诉我正在看什么，只查看」，或点击 **▣ 看当前窗口**。成功后再试「帮我在浏览器搜索……」。语音提出相同请求走同一条后端链路；若正在用其他应用语音通话，先用文字测试，稍后再测麦克风/唤醒/播报。

## 主模型可以只有文字能力

优先读取辅助功能（AX）文本，不需要把图片交给主模型。图片、画布或无有效 AX 文本的界面需要截图时，Hermes 的 `computer_use` 可以将分析交给 `auxiliary.vision`，再把文字结果给主模型。

按所用 Hermes 版本配置独立视觉提供商/模型及其凭据。`vision` 工具处于 enabled/configured 状态**不证明视觉请求已成功**。先用 Hermes 实测视觉模型；此版本 Pocket Agent 不代管视觉密钥，不自动换主模型，也不把缺少视觉服务隐藏成“已看懂”。如果只需要可访问的界面文字，可以先不做截图测试。

## 实际运行与权限边界

- 只有当前请求涉及桌面时才读取；没有持续屏幕监视，也没有定时抓屏。
- 本机且能力齐备时使用 Hermes `/v1/runs`，通过事件流显示回复/工具进度。具体操作要求审批时，弹窗显示 Hermes 的操作说明，只提供「本次允许」和「拒绝」，不创建永久或整会话授权。Hermes 自己已有的审批策略仍生效，因此并非每个操作必定弹窗。
- 窗口和网页内容只是资料，不能成为操作授权。查看不意味着可以输入/发送/购买。提示模型先读目标、执行有界动作、再读目标验证；这个策略由 Hermes 工具执行，并非 Pocket Agent 的独立沙箱。
- 打断请求会调用后台停止接口；停止只能阻止后续工作，不能撤销已执行动作。如果停止失败或连接丢失，会提示后台状态不确定，不应盲目重复点击/输入。创建任务的响应丢失时也不会自动重发。
- 本机 Hermes UI 对话不执行旧 `[CMD:...]` shell 标签；其他既有网关/bridge 行为保留。
- 点击 Pocket Agent 可能使其成为前台。当前窗口若确实是 Pocket Agent，助手会询问目标应用，不假装记得点击前的窗口。可以直接说「查看 Safari 当前页面」或在目标应用保持前台时用语音请求。

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

远程地址及 OpenClaw 等已有聊天兼容性保留，但 **本版本的本机电脑入口不适用于远程 Hermes**。电脑工具操作驱动所在的机器，远程 Hermes 不会自动看到 PA 客户端。SSH 转发、代理或远程驱动即使用 localhost 地址，也不证明是同机；当前检测不是主机身份认证，请按同机方式部署。本功能不新建远程控制服务。

## 验证记录（2026-09-19）

- 项目：前端生产构建、Rust 单元测试；具体测试数量以交付报告为准。
- 本机环境：Hermes 0.21.3，CuaDriver 0.28.2；为现有 xingli profile 的 api_server 启用 computer_use。这是该电脑的配置变更，不包含在可分发应用中。
- 通过 PA 相同 Hermes API 和桌面提示：一次 AX 当前窗口读取成功；新 `/v1/runs` 中「读取 → 设置同一应用后台目标（不切前台）→ 再读」成功。
- 用户指定浏览器验收：真实 Chrome 导航 Apple Newsroom 并读取新闻稿；首次因多窗口/地址栏焦点发生多次失败。收窄流程后，新标签页中 cmd+l → 输入完整 Google 搜索 URL → 回车 → 等待 → AX 重读成功，读到 Apple 官方搜索结果。完整官方文章 URL 另经网页核验补齐，没有把该网页核验算成电脑操作测试。
- 浏览器自动化依然依赖 Hermes 模型与驱动的目标定位能力。提示词现在要求保留旧标签、全选地址后输入、同一步两次失败即停；这是交互策略，不能保证所有界面可靠。
- 不等于完整 Pocket Agent UI/麦克风/唤醒/TTS 端到端实测。截图辅助视觉、真实审批弹窗交互和换机安装需各自验收；不能从 AX 成功推断它们通过。
