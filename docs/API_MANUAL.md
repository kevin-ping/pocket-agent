# Pocket Agent API 手册

PA 在本地 `8650` 端口运行 HTTP 服务。`/health`、`/push`、`/bridge/send` 需 Bearer token 认证（`API_SERVER_KEY`）；手机终端的两个 WebSocket 用 query 参数带 token，见文末「手机终端」。

---

## GET /health

健康检查。

**响应**：`200 OK` → `"ok"`

---

## POST /push

向 PA 推送消息，触发 TTS 播报和文字展示。

**URL**：`http://127.0.0.1:8650/push`
**Auth**：`Authorization: Bearer {API_SERVER_KEY}`

### 请求体

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| text | string | ✅ | 要播报和展示的文字 |
| emotion | string | ❌ | 情绪语气：friendly, cheerful, calm, serious, sad, whisper, excited, angry（默认 friendly） |
| voice | string | ❌ | 覆盖 TTS 声音，如 "zh-CN-XiaoxiaoNeural" |

### 响应

**200 OK**：
```json
{"ok": true, "message": "pushed"}
```

**400 Bad Request**：text 为空
**429 Too Many Requests**：音频队列满（最多 3 个并发）

### 示例

```bash
curl -X POST http://127.0.0.1:8650/push \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"text": "你好！任务完成了！", "emotion": "cheerful"}'
```

---

## POST /bridge/send

第三方 app 桥接入口。外部 app 通过 PA 转发消息给 Hermes，PA 负责 thinking 动画和最终播报。

**架构链路**：
```
第三方 app → PA /bridge/send → Hermes 流式返回 → PA push_to_self → TTS 播报 → 用户
```

**角色分工**：
- **PA**：前台交互壳层（thinking 动画、TTS 播报、展示文字）
- **Hermes**：唯一大脑（思考、调业务 API、决定回复内容）

**URL**：`http://127.0.0.1:8650/bridge/send`
**Auth**：`Authorization: Bearer {API_SERVER_KEY}`

### 请求体

| 字段 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| source | string | ✅ | - | 来源 app 标识（如 "chess-app"） |
| session_id | string | ✅ | - | 来源 app 的会话 ID |
| text | string | ✅ | - | 发送给 Hermes 的消息内容 |
| user_language | string | ❌ | null | 用户语言偏好 |
| context | string | ❌ | null | 附加上下文（如棋局状态） |
| show_thinking | bool | ❌ | true | 是否在 PA 显示 thinking 动画；为 false 时思考内容也不写入思考面板。思考面板是否显示还取决于用户设置「显示思考过程」(默认关) |

### 响应

**202 Accepted**（请求已被接受，异步转发 Hermes）：
```json
{
  "ok": true,
  "accepted": true,
  "source": "chess-app",
  "session_id": "game-001",
  "bridge_session": "bridge:chess-app:game-001",
  "message": "accepted for Hermes dispatch"
}
```

**400 Bad Request**（参数校验失败）：
```json
{"ok": false, "error": "source is empty"}
```

**401 Unauthorized**（API key 错误或缺失）

### Session 隔离

Bridge 使用独立命名空间：`bridge:{source}:{session_id}`

示例：
- `source: "chess"`, `session_id: "game-001"` → `bridge:chess:game-001`
- `source: "weather"`, `session_id: "daily-042"` → `bridge:weather:daily-042`

确保：
- Bridge session 和 PA 日常聊天 session 完全隔离
- 不同 source app 之间互不干扰
- Hermes 侧能看到来源标识，便于匹配 system prompt

### 和 PA 日常聊天的区别

| 行为 | PA 日常聊天 | Bridge |
|------|------------|--------|
| 入口 | send_message() | /bridge/send |
| Session | pocket-agent-YYYY-MM-DD | bridge:{source}:{session_id} |
| Daily summary | ✅ 注入 | ❌ 不注入 |
| 语言 suffix | ✅ 自动追加 | ❌ 不追加 |
| TTS 播报 | push_to_self 自播 | push_to_self 自播（同 UI 模式） |
| 聊天历史 | ✅ 保存 | ❌ 不保存 |
| 中间事件 | chat-thinking / chat-tool-call | bridge-thinking / bridge-tool-call |
| Thinking fallback | 无 | 30s 超时自动退出 |

### 事件流

```
1. 第三方 POST /bridge/send
2. PA 返回 202 Accepted
3. PA 进入 thinking 状态（bridge-thinking-start）
4. Hermes 流式返回中间推理：
   - bridge-thinking（reasoning 片段，追加到状态栏下方的思考面板；面板仅在设置「显示思考过程」开启且状态栏可见时显示）
   - bridge-tool-call（工具调用通知，载荷 `{id, name, preview}`；`preview` 是 Hermes runs 通道 `tool.started` 自带的参数摘要，如 terminal 的命令，状态栏显示前约 60 字，不播报；chat-completions 通道为空串）
5. Hermes 处理完毕，PA 拿到完整回复
6. PA 通过 push_to_self 调内部 /push 触发 TTS + 展示
7. PA 退出 thinking 状态
```

### 退出条件

| 事件 | 行为 |
|------|------|
| bridge-push-received | 完整 UI 收尾，不弹 error（由 push_to_self 触发） |
| bridge-turn-finished | 仅在回复为空时触发，退出 thinking |
| bridge-turn-error | 退出并显示错误 |
| 30s 超时 | 自动退出并提示 |

### 示例

基本测试：
```bash
curl -X POST http://127.0.0.1:8650/bridge/send \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "source": "test-app",
    "session_id": "test-001",
    "text": "你好，这是一条桥接测试消息"
  }'
```

完整参数（chess 场景）：
```bash
curl -X POST http://127.0.0.1:8650/bridge/send \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "source": "chess-app",
    "session_id": "game-001",
    "text": "当前棋局状态：白方 e4，请分析下一步",
    "context": "这是一场国际象棋对局，白方先行",
    "show_thinking": true
  }'
```

校验拦截测试：
```bash
curl -X POST http://127.0.0.1:8650/bridge/send \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"source": "   ", "session_id": "x", "text": "x"}'
# 期望: 400 {"ok": false, "error": "source is empty"}
```

---

## 手机终端(`/ui/`、`/ui/ws`、`/audio`)

把 PA 的界面、收音和播放搬到手机(安卓壳见 `android/`)。实现：`src-tauri/src/api/remote.rs`、`src-tauri/src/voice/remote.rs`、`src/lib/ipc.ts`。

**鉴权**:两个 WebSocket 都必须带 `?token={API_SERVER_KEY}`;没配 key 时一律拒绝(与 `/push` 的「没配 key 就放行」不同)。错误或缺失 → 升级前返回 401。8650 只在局域网用，不要做端口映射:`/ui` 能批准电脑操作。

**单一主控**:`/ui/ws` 与 `/audio` 各只保留一个客户端，新连接顶掉旧连接(被顶掉的 `/ui` 收到关闭码 `4001`,页面不再重连;被顶掉的手机从下一帧起被拒收)。有 `/ui` 客户端时桌面主窗口隐藏，桌面前端重载成空壳(不监听、不发命令),全部断开后恢复。每次主控变化(包括一部手机顶掉另一部)都会停掉播放和连续对话，新主控从干净状态开始;待处理的电脑审批不受影响，新主控挂载后用 `pending_computer_approvals` 取回并重新显示。

**手机待命**(FEAT-DEV1):Hermes 调 `switch_device(desktop)` 后，或在手机主界面点左上角电脑图标(「移到电脑」,发 `park`,立即生效，不等回合结束;FEAT-DEV4)后，手机进入待命：保持 `/ui` 和 `/audio` 连接，但桌面是主控和音频源(本地麦克风 / 喇叭)。待命期间:
- 主界面事件不再发给手机，手机发来的 PCM 和播放回报被忽略;
- 手机页只显示「PA 在电脑上」和「切回手机」按钮，只能发 `is_ui_parked` 和 `take_over`,其他命令返回 `command not allowed: <cmd>`;
- 手机重连(App 回到前台、页面重载)仍是待命，不会自动接管;
- 结束待命只有两种方式:`switch_device(phone)`(要求手机页在前台:`/ui` 和 `/audio` 都在线)或点「切回手机」(`take_over`,立即生效)。

待命状态只在内存里,PA 重启后清零(重启后手机页没重连时，桌面是主控，声音也在桌面)。

### GET /ui/

PA 主界面的前端(打包进 app 的静态资源，不含 key);远程模式下用竖屏手机布局(`src/lib/components/mobile/`)。手机 WebView 打开 `http://<Mac IP>:8650/ui/?token=...`,页面从自己的 URL 读 token 去连 `/ui/ws`。桌面浏览器同样可以打开调试。

### WS /ui/ws

- 下行事件：`{"type":"event","event":"chat-stream","payload":...}`,事件名单见 `UI_EVENTS`(待命时不发)。
- 只发给手机页的事件(桌面窗口收不到):
  - `phone-camera` — `{"action":"open","facing":"front"|"back"}` 或 `{"action":"close"}`。open:相机没开就按 `facing` 打开;已开且朝向不同就翻转。由 `phone_camera_open` / `phone_camera_close` 触发。
  - `ui-parked` — `{"parked":true|false}`,待命状态变化;页面随即重载进(或退出)待命界面。页面启动时用 `is_ui_parked` 查当前状态。
- 上行命令：`{"id":1,"cmd":"send_message","args":{...}}`(参数名与前端 `invoke` 相同，驼峰);回 `{"type":"result","id":1,"ok":...}` 或 `{"type":"result","id":1,"err":"..."}`。
- 只接白名单 `UI_COMMANDS`;设置窗口、退出、窗口位置、聊天记录窗口等返回 `command not allowed: <cmd>`。
- 手机专用命令:`is_ui_parked`(返回是否待命)、`take_over`(结束待命并立即接管，同时取消还没执行的 `switch_device`)、`park`(手机主界面「移到电脑」按钮：立即进入待命，桌面接管主控和音频源，同时取消还没执行的 `switch_device`;待命时不可用，返回 `command not allowed: park`)。`take_over` 和 `park` 只认当前的 `/ui` 连接：页面已被新连接顶掉后才执行到的命令返回 `err`,不改待命状态、不取消新页面那边的预约。电脑端没有对应按钮(注册多台设备时电脑不知道该移去哪台)。

### WS /audio

- 上行二进制：PCM16LE,16 kHz,单声道，持续发送(建议 20 ms 一帧)。
- 下行：`{"type":"play","seq":N}` 紧跟一帧二进制(该句 mp3);`{"type":"clear"}` = 立即停止并清空队列。
- 上行文本：开始播放 `{"type":"playing","seq":N}`,播完 `{"type":"done","seq":N}`。与当前句子不符的回报被忽略。
- 相机(FEAT-CAM1):手机上报 `{"type":"camera","on":true,"facing":"front"|"back","switching":bool}` 或 `{"type":"camera","on":false}`。`facing` 是实际绑定的镜头;已开的相机开始换镜头时先报 `switching:true`(此时不出帧),绑好后报 `switching:false`,绑定失败则关闭并报 `on:false`。`switching` 期间 PA 不取图，直接回「手机相机正在切换镜头，稍后再取画面」;PA 下发 `{"type":"snapshot","id":N}` 取当前帧，手机回 `{"type":"snapshot","id":N,"jpeg":"<base64>"}`(相机不可用时省略 `jpeg`)。5 秒内没回 → 超时;被顶掉的连接的回包作废;手机断开 / 被顶掉时相机状态清零。帧只在内存里。
- 音频源跟着主控走：只有手机页正在当主控(`/ui` 连着且未待命)并且 `/audio` 连着时，唤醒词、连续对话、打断才改用手机音频，Mac 不开麦克风、不用喇叭;否则(只有 `/audio`、没有 `/ui`,例如 PA 重启后只有录音服务重连上;或手机待命)用 Mac 的麦克风和喇叭。`/ui` 连上 / 断开 / 顶替、待命变化、`/audio` 连上 / 断开后都会重新收敛，与到达顺序无关;切回本地时进行中的对话按中断结束，本地唤醒自动恢复。按 Home / 锁屏时 WebView 仍保持 `/ui`,手机仍是主控，声音仍在手机。

测试：`scripts/phone_sim.py hello.wav [--barge-in stop.wav]` 把 wav 当手机麦克风推上去，收到的 mp3 存到 `phone_sim_out/`。`--camera test.jpg` 模拟相机已打开，对每个 snapshot 请求回这张图。

## POST /mcp(Hermes 用的 MCP 服务端)

最小的 MCP Streamable HTTP 服务端(`src-tauri/src/api/mcp.rs`)。什么时候开相机、拍照、切设备由 Hermes 决定;PA 只执行，并用文字如实返回结果(从不返回 JSON-RPC 错误)。相机开着时 PA 在该轮附一行事实提示。本次 PA 运行中手机 app 连过以后，每个 UI 回合还附一句设备事实：此刻(本条消息时)PA 在手机上 / 在电脑上、手机待命 / 在电脑上、手机 app 未连接，并注明这是实时状态、可能在两轮之间变化、以此为准(对话里更早的设备说法可能已过时);从没连过手机时不附(`commands/chat.rs::phone_device_hint`)。

| 工具 | 参数 | `readOnlyHint` | 返回 |
|---|---|---|---|
| `phone_camera_open` | `facing?`: `"front"` / `"back"` | 是 | 没给 `facing` →「用户没有说明用前置还是后置摄像头」(不开相机);`facing` 是其他值 → 说明只能是 front / back;手机待命 →「手机处于待命,PA 现在在电脑上」;手机页不在 →「手机界面没有在前台打开」;已开且就是该镜头 →「手机相机已经打开，用的就是X摄像头」;已开但镜头不同 → 手机页翻转，等手机报告该镜头绑好(最多 15 秒)→「手机相机已切换到X摄像头」,翻转中相机关闭 →「切换摄像头失败，手机相机已关闭」;打开成功 →「手机相机已打开(X摄像头)」;15 秒内没上报打开 →「等待相机打开超时(可能在等用户授权)」;已开但 15 秒内没确认镜头 →「已通知手机用X摄像头，但手机没有确认镜头已切换好」。工具只在手机确认后才说成功，所以紧接着的 snapshot 拿到的一定是新镜头的画面 |
| `phone_camera_snapshot` | 无 | 是 | 当前一帧(MCP image block,`image/jpeg`);相机没开 / 手机没连 →「用户的手机相机没有打开(phone_camera_open 可以打开)」;5 秒没回 →「手机没有在 5 秒内返回画面」 |
| `phone_camera_close` | 无 | 是 | 「已通知手机关闭相机」/「手机相机本来就没有打开」/ 手机待命或不在时同上 |
| `switch_device` | `target`: `"desktop"` / `"phone"` | 是(见下) | 「将在本轮回复说完后切到电脑 / 手机」/「已经在电脑上」/「已经在手机上」/「手机界面没有在前台打开」 |

`switch_device` 只登记切换：等本轮回合结束(包括被取消或失败)、播放队列放完(回合结束后最多再等 30 秒)并安静 1 秒后才执行，这样确认的话在原设备上说完。执行前再检查一次条件(手机这期间退到后台 → 不切，只记日志);新的 `switch_device`(目标有效即可，包括回「已经在电脑上 / 手机上」或「手机界面没有在前台打开」的那次)、`take_over` 或 `park` 会取消还没执行的那次：以 Hermes 最后听到的答复为准。预约登记、取消和最终提交在同一把锁里，已通过检查的旧预约不会在 `take_over` 之后再把手机送回待命。切到电脑 = 手机待命(见上文「手机待命」)。

- **鉴权**:`Authorization: Bearer {API_SERVER_KEY}`;没配 key 时一律 401(无开放模式)。
- 只处理 `initialize`、`ping`、`tools/list`、`tools/call`;通知(无 `id`)回 202;其他方法 / 未知工具回 JSON-RPC 错误;不支持批量。请求体上限 64 KB。`GET` / `DELETE` → 405(没有服务端推送流和会话)。
- Hermes 注册(`~/.hermes/profiles/xingli/config.yaml`,改完重启网关):

```yaml
mcp_servers:
  pocket_agent:
    url: http://127.0.0.1:8650/mcp
    headers:
      Authorization: Bearer ${API_SERVER_KEY}
    trust: untrusted   # 没标 readOnlyHint 的工具要审批;现有四个工具都标了
```

Hermes 里的工具名是 `mcp__pocket_agent__<工具名>`。**审批**:Hermes 只在服务器配了 `trust: untrusted` 时，对没标 `readOnlyHint` 的工具弹审批(默认 `trust: full` 一律不弹),没有按工具放行的配置。而且 Hermes 的 MCP 审批不会记住「本会话允许 / 始终允许」,每次调用都会再问。所以现在四个工具都标了 `readOnlyHint`,都不审批(`phone_camera_open` 原来要审批，用户撤销了这一条，见 decision-20261104-153000;`switch_device` 实际会改状态，但用户要求不审批，见 decision-20261104-130000);xingli 的 `pocket_agent` 仍配 `trust: untrusted`,给以后加的、没标 `readOnlyHint` 的工具兜底。审批经 `/v1/runs` 的审批事件转发到 PA 的审批界面(PA 兼容两种事件格式：旧的 `request_id`,以及 session authority 模式的 `prompt_id` + `execution_generation`,回应时按原格式带回);`/bridge/send` 回合走 chat/completions,没人能批，Hermes 等审批超时(60 秒)后按未批准处理。

**PA 更新了工具列表后要重启 Hermes 网关**(`launchctl kickstart -k gui/$(id -u)/ai.hermes.gateway-xingli`),Hermes 只在启动时读 `tools/list`。
