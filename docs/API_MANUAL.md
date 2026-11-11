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

## 手机终端(`/ui/`、`/ui/ws`、`/audio`、`/phone`、`/pair/*`)

把 PA 的界面、收音和播放搬到手机(安卓壳见 `android/`)。一台手机可以配对多台 PA(例如 Mac 和 Windows),在它们之间切换;同一时刻只给一台用。实现：`src-tauri/src/api/remote.rs`、`src-tauri/src/voice/remote.rs`、`src-tauri/src/api/pairing.rs`、`src-tauri/src/api/phone.rs`、`src/lib/ipc.ts`。

**鉴权**:`/ui/ws`、`/audio`、`/phone` 都必须带 `?token=`,值是 `API_SERVER_KEY`(手填方式，旧版 app)或某台已配对手机的设备 token(见「配对」)。没配 key 且没有配对过手机时一律拒绝(与 `/push` 的「没配 key 就放行」不同)。错误或缺失 → 升级前返回 401。在设置 → 设备里移除一台手机后，它用设备 token 开的 `/ui/ws`、`/audio`、`/phone` 立即被断开，旧 token 之后一律 401。8650 只在局域网用，不要做端口映射:`/ui` 能批准电脑操作。

**身份**:`pa_id` 是 PA 首次启动时生成的 UUID,存在 `settings.db` 的 metadata 里，电脑改名、换 IP 都不变。PA 不在局域网里广播自己(没有 mDNS);手机只通过扫描电脑屏幕上的二维码认识它。测试时 `POCKET_AGENT_PORT` 环境变量可以让第二个 PA 用别的端口(默认 8650)。

### 配对(扫码:GET /pair/hello、POST /pair/confirm)

电脑发起：设置 → 设备 →「添加手机」显示二维码，手机扫码。两个接口都不需要鉴权(手机此时还没有凭据);保护来自二维码里的一次性密钥，**密钥本身从不经过网络**(局域网是明文 HTTP,同一 Wi-Fi 下能被嗅探)。

**二维码内容**:`pocketagent://pair?v=1&id=<pa_id>&s=<secret>&h=<ip:port>,<ip:port>…`
- `secret`:32 字节随机数,base64url 无填充(43 个字符)。点「添加手机」时生成,2 分钟有效、只能配对一次;再点一次就换新码，旧码作废;设置页点「取消」也作废。
- `h`:电脑所有非回环的私网 IPv4(10/8、172.16/12、192.168/16),端口是实际监听端口(含 `POCKET_AGENT_PORT`)。

**HMAC**:HMAC-SHA256(RFC 2104),key = `secret` 的 32 个原始字节，消息是 UTF-8 字符串;结果用 base64url 无填充传输。
- `proof = HMAC(secret, "phone|" + nonce + "|" + phone_id)`(手机算)
- `server_proof = HMAC(secret, "pa|" + nonce + "|" + phone_id)`(PA 算)

`nonce` 按 `/pair/hello` 返回的字符串原样参与计算。

1. `GET /pair/hello` → `200 {"ok":true,"v":1,"pa_id","pa_name","nonce"}`。`nonce` 是 16 字节随机数(base64url),只能用一次;每个二维码最多保留 8 个未用的 nonce,更早的作废。**没有二维码在显示时 → `410`**,不返回任何身份信息。
   - 手机按二维码里的地址逐个请求，先试与自己 IP 同一 /24 的;只向 `pa_id` 与二维码一致的地址继续。
2. `POST /pair/confirm {"phone_id":"…","phone_name":"Pixel 2","nonce":"…","proof":"…"}` → `200 {"ok":true,"pa_id","pa_name","token","server_proof"}`。`token` 是 32 字节随机数(64 位十六进制),之后当 `?token=` 用;二维码随即作废。
   - `phone_id`:1–64 位字母、数字、`-`、`_`;`phone_name`:去掉控制字符后 1–64 个字符。不合格 → `400`(不计错误次数)。
   - `nonce` 不认识或已用过 → `403 {"error":"unknown or used nonce"}`。nonce 不论成功失败都只能用一次。
   - `proof` 不对(包括无法解码)→ `403 {"error":"wrong proof","tries_left":N}`;错满 5 次这个二维码作废，之后 → `410`。比较用常量时间;nonce 消费、错误计数、作废在同一把锁内完成。
   - 没有二维码(过期、已用、取消、换了新码、作废)→ `410`。
3. 手机先用常量时间比较校验 `server_proof`,确认对方确实持有同一个密钥(即显示二维码的那台电脑),通过后才保存 token;不通过则什么都不保存。
   - 同一台手机再次配对会换新 token,旧 token 和用它开的连接一起作废;手机端按 `pa_id` 覆盖已有记录(电脑换了 IP,重新扫一次即可)。

PA 只在 `settings.db` 的 `paired_phones(phone_id, name, token_hash, created_at, last_seen_at)` 里存 token 的 SHA-256;日志不打印密钥、proof 和 token。设置 → 设备列出已配对的手机(在线状态、上次在线时间),可以移除。这些设置页命令(`start_pairing`、`cancel_pairing`、`list_paired_phones`、`remove_paired_phone`)只在电脑上的设置窗口可用,`/ui/ws` 不接受。`start_pairing` 返回 `{svg, expires_in, generation}`(二维码 SVG 与它的世代号),没有局域网地址时报错 `no LAN address`;`cancel_pairing(generation)` 只取消同一世代的二维码。二维码结束(配对成功、错 5 次作废、取消)时发 `pairing-ended {generation}`,设置页只关闭世代相同的二维码;列表或在线状态变化发 `pairing-changed`。

**自测**(Mac):不显示二维码时 `curl -i http://127.0.0.1:8650/pair/hello` → `410`;点「添加手机」后 → `200` 带 nonce。

### WS /phone(在线连接与占用)

已配对的手机在前台服务里给**每台**已配对的 PA 各保持一条 `/phone`,不管有没有在用这台。PA 由此知道哪些手机在线、各被谁占着，不需要扫描局域网。

- 手机 → PA:
  - 连上后先发 `{"type":"hello","phone_id","phone_name","occupant":{"pa_id","pa_name"}|null,"foreground":bool}`(10 秒内没发 → 断开)。用设备 token 连接时，`phone_id` 以 token 为准，hello 里的值被忽略;用 `API_SERVER_KEY` 连接时取 hello 里的。
  - 占用或「app 在前台」变化时发 `{"type":"state","occupant":…,"foreground":bool}`。
  - 回应进入请求：`{"type":"enter_reply","req_id":N,"ok":true}`,或 `{"ok":false,"reason":"busy","busy_with":"Mac"}` / `{"ok":false,"reason":"not_foreground"}`。
- PA → 手机:`{"type":"welcome","pa_id","pa_name"}`(回应 hello;手填地址的记录由此补上 `pa_id`)、`{"type":"enter_request","req_id":N}`(这台 PA 想用手机)、`{"type":"release"}`(这台 PA 已回到电脑)。
- 同一台手机的新 `/phone` 连接顶掉旧的;30 秒没有任何帧(含 ping/pong)→ 视为离线。

**占用规则(手机说了算)**:手机持久保存 `occupant`(正在用它的 PA)。被一台 PA 占用时，其他 PA 的进入请求一律回 `busy`,电脑端、手机端都不能抢。释放只有三种情况：
1. 住户 PA 回到电脑：手机页「移到电脑」(`park`)、在那台电脑上说「切回电脑」、Hermes 调 `switch_device(desktop)`。PA 进入待命并发 `release`,手机断开这台 PA 的 `/ui` + `/audio`,清空 occupant,回到设备页(`/phone` 保留)。住户当时不在线的话，它下次 hello 仍报这台 PA 时 PA 再发 `release`,不会因此重新接管。住户的 `/ui` 和 `/phone` 都暂时断开(还在 90 秒租约里)时，`switch_device(desktop)` 同样登记切换，回合结束后待命并记下这笔 `release`,不会回「已经在电脑上」。
2. 手机上断开：通知栏「断开」、从最近任务划掉 app。
3. 住户 PA 离线超过 **90 秒**(租约):短暂断网不会让别的 PA 顶掉住户。

PA 看到自己成为 occupant(hello / state 里的 occupant 从别的值变成自己)时结束待命，相当于 `take_over`(同时取消还没执行的 `switch_device`),手机随后连上 `/ui` + `/audio` 照常当主控。每台 PA 接的是各自的 Hermes,对话历史不共享。

测试:`scripts/phone_sim.py --pa <PA1> --pa <PA2> --scenario --lease 5` 模拟一台手机同时连两台 PA,脚本化跑一遍占用、拒绝、释放(park)、租约，任一项不通过时退出码为 1。

**单一主控**:`/ui/ws` 与 `/audio` 各只保留一个客户端，新连接顶掉旧连接(被顶掉的 `/ui` 收到关闭码 `4001`,页面不再重连;被顶掉的手机从下一帧起被拒收)。有 `/ui` 客户端时桌面主窗口隐藏，桌面前端重载成空壳(不监听、不发命令),全部断开后恢复。每次主控变化(包括一部手机顶掉另一部)都会停掉播放和连续对话，新主控从干净状态开始;待处理的电脑审批不受影响，新主控挂载后用 `pending_computer_approvals` 取回并重新显示。

**手机待命**(FEAT-DEV1;新版 app 待命时直接断开、回到设备页，见上文「占用规则」;以下是保持连接的旧版 app 的行为):Hermes 调 `switch_device(desktop)` 后，或在手机主界面点左上角电脑图标(「移到电脑」,发 `park`,立即生效，不等回合结束;FEAT-DEV4)后，手机进入待命：保持 `/ui` 和 `/audio` 连接，但桌面是主控和音频源(本地麦克风 / 喇叭)。待命期间:
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
- 下行：`{"type":"play","seq":N}` 紧跟一帧二进制(该句 mp3);`{"type":"clear"}` = 立即停止并清空队列;`{"type":"wake"}` = 手机作为音频源时听到主人的唤醒词(声纹通过),手机亮屏并把页面带到前台(锁屏时显示在锁屏之上),因为隐藏的 WebView 页面会被冻结。电脑收音时不发。
- 上行文本：开始播放 `{"type":"playing","seq":N}`,播完 `{"type":"done","seq":N}`。与当前句子不符的回报被忽略。
- 相机(FEAT-CAM1):手机上报 `{"type":"camera","on":true,"facing":"front"|"back","switching":bool}` 或 `{"type":"camera","on":false}`。`facing` 是实际绑定的镜头;已开的相机开始换镜头时先报 `switching:true`(此时不出帧),绑好后报 `switching:false`,绑定失败则关闭并报 `on:false`。`switching` 期间 PA 不取图，直接回「手机相机正在切换镜头，稍后再取画面」;PA 下发 `{"type":"snapshot","id":N}` 取当前帧，手机回 `{"type":"snapshot","id":N,"jpeg":"<base64>"}`(相机不可用时省略 `jpeg`)。5 秒内没回 → 超时;被顶掉的连接的回包作废;手机断开 / 被顶掉时相机状态清零。帧只在内存里。
- 音频源跟着主控走：只有手机页正在当主控(`/ui` 连着且未待命)并且 `/audio` 连着时，唤醒词、连续对话、打断才改用手机音频，Mac 不开麦克风、不用喇叭;否则(只有 `/audio`、没有 `/ui`,例如 PA 重启后只有录音服务重连上;或手机待命)用 Mac 的麦克风和喇叭。`/ui` 连上 / 断开 / 顶替、待命变化、`/audio` 连上 / 断开后都会重新收敛，与到达顺序无关;切回本地时进行中的对话按中断结束，本地唤醒自动恢复。按 Home / 锁屏时 WebView 仍保持 `/ui`,手机仍是主控，声音仍在手机。

测试(`/audio`):`scripts/phone_sim.py hello.wav [--barge-in stop.wav]` 把 wav 当手机麦克风推上去，收到的 mp3 存到 `phone_sim_out/`。`--camera test.jpg` 模拟相机已打开，对每个 snapshot 请求回这张图。

## POST /mcp(Hermes 用的 MCP 服务端)

最小的 MCP Streamable HTTP 服务端(`src-tauri/src/api/mcp.rs`)。什么时候开相机、拍照、切设备由 Hermes 决定;PA 只执行，并用文字如实返回结果(从不返回 JSON-RPC 错误)。相机开着时 PA 在该轮附一行事实提示。本次 PA 运行中手机 app 连过以后，每个 UI 回合还附一句设备事实：此刻(本条消息时)PA 在手机上 / 在电脑上、手机待命 / 在电脑上、手机 app 未连接，并注明这是实时状态、可能在两轮之间变化、以此为准(对话里更早的设备说法可能已过时);从没连过手机时不附(`commands/chat.rs::phone_device_hint`)。这句末尾只在 PA 判定 Hermes 能用 pocket_agent MCP 时写「switch_device 可以切换」;否则(PA 无法确认 Hermes 有这个工具)不提工具，只写手动切换方法：「用户可以在手机 app 的设备列表点这台电脑旁的 Connect(接入)切到手机，在手机页面点「移到电脑」切回」。

**MCP 可用性**(`api/mcp_status.rs`,设置 → 设备显示同一结论):8650 绑定失败 → 本地服务未启动;本机网关时，在 Hermes 主目录和 `profiles/*` 里找 `.env` 有 `API_SERVER_KEY`、`API_SERVER_PORT`(缺省 8642)等于 `API_SERVER` 端口的唯一 profile,读它的 `config.yaml`:`mcp_servers.pocket_agent` 不存在、已关闭(`enabled` 按 Hermes 的规则解释：`false` / `0` / `"false"` / `off` / `no` 等都算关闭;认不出的值 → 无法确认)、url 不是本机的 `http://…:PA 端口/mcp`(协议、端口或路径不对)→ 未接入(优先于本次运行的请求记录);否则本次运行最近一次 `/mcp` 请求带了错的 Bearer → 被拒;带对的 Bearer 或配置已注册 → 正常;远程网关 / 找不到唯一 profile / 文件读不了 / url 指向其他主机 → 无法确认。只读这两个文件的上述字段，错误只记类别，不回显内容。

| 工具 | 参数 | `readOnlyHint` | 返回 |
|---|---|---|---|
| `phone_camera_open` | `facing?`: `"front"` / `"back"` | 是 | 没给 `facing` →「用户没有说明用前置还是后置摄像头」(不开相机);`facing` 是其他值 → 说明只能是 front / back;手机待命 →「手机处于待命,PA 现在在电脑上」;手机页不在 →「手机界面没有在前台打开」;已开且就是该镜头 →「手机相机已经打开，用的就是X摄像头」;已开但镜头不同 → 手机页翻转，等手机报告该镜头绑好(最多 15 秒)→「手机相机已切换到X摄像头」,翻转中相机关闭 →「切换摄像头失败，手机相机已关闭」;打开成功 →「手机相机已打开(X摄像头)」;15 秒内没上报打开 →「等待相机打开超时(可能在等用户授权)」;已开但 15 秒内没确认镜头 →「已通知手机用X摄像头，但手机没有确认镜头已切换好」。工具只在手机确认后才说成功，所以紧接着的 snapshot 拿到的一定是新镜头的画面 |
| `phone_camera_snapshot` | 无 | 是 | 当前一帧(MCP image block,`image/jpeg`);相机没开 / 手机没连 →「用户的手机相机没有打开(phone_camera_open 可以打开)」;5 秒没回 →「手机没有在 5 秒内返回画面」 |
| `phone_camera_close` | 无 | 是 | 「已通知手机关闭相机」/「手机相机本来就没有打开」/ 手机待命或不在时同上 |
| `switch_device` | `target`: `"desktop"` / `"phone"` | 是(见下) | 「将在本轮回复说完后切到电脑 / 手机」/「已经在电脑上」/「已经在手机上」/「手机界面没有在前台打开」;手机没连 `/ui` 时:「没有在线的已配对手机」/「手机正在被「X」上的 PA 使用，请先在那台电脑上让 PA 回到电脑」 |

`switch_device` 只登记切换：等本轮回合结束(包括被取消或失败)、播放队列放完(回合结束后最多再等 30 秒)并安静 1 秒后才执行，这样确认的话在原设备上说完。执行前再检查一次条件(手机这期间退到后台 → 不切，只记日志);新的 `switch_device`(目标有效即可，包括回「已经在电脑上 / 手机上」或「手机界面没有在前台打开」的那次)、`take_over` 或 `park` 会取消还没执行的那次：以 Hermes 最后听到的答复为准。预约登记、取消和最终提交在同一把锁里，已通过检查的旧预约不会在 `take_over` 之后再把手机送回待命。切到电脑 = 手机待命(见上文「手机待命」)。

切到手机时手机没连 `/ui`(多台 PA,FEAT-MP3):从 `/phone` 在线的已配对手机里挑最近用过这台 PA 的一台。PA 先按手机最近报告的状态回答：被别的 PA 占着 → 「手机正在被「X」上的 PA 使用…」;app 不在前台 →「手机界面没有在前台打开」;没有在线手机 →「没有在线的已配对手机」;否则登记切换，等回合结束后发 `enter_request`,由手机最终决定(此时被占或不在前台就拒绝，PA 只记日志)。手机同意后自己成为这台 PA 的 occupant 并连上 `/ui` + `/audio`。

- **鉴权**:`Authorization: Bearer {API_SERVER_KEY}`;没配 key 时一律 401(无开放模式)。
- 只处理 `initialize`、`ping`、`tools/list`、`tools/call`;通知(无 `id`)回 202;其他方法 / 未知工具回 JSON-RPC 错误;不支持批量。请求体上限 64 KB。`GET` / `DELETE` → 405(没有服务端推送流和会话)。
- Hermes 注册(`~/.hermes/profiles/<profile>/config.yaml`,改完重启网关):

```yaml
mcp_servers:
  pocket_agent:
    url: http://127.0.0.1:8650/mcp
    headers:
      Authorization: Bearer ${API_SERVER_KEY}
    trust: untrusted   # 没标 readOnlyHint 的工具要审批;现有四个工具都标了
```

Hermes 里的工具名是 `mcp__pocket_agent__<工具名>`。**审批**:Hermes 只在服务器配了 `trust: untrusted` 时，对没标 `readOnlyHint` 的工具弹审批(默认 `trust: full` 一律不弹),没有按工具放行的配置。而且 Hermes 的 MCP 审批不会记住「本会话允许 / 始终允许」,每次调用都会再问。所以现在四个工具都标了 `readOnlyHint`,都不审批(`phone_camera_open` 原来要审批，用户撤销了这一条，见 decision-20261104-153000;`switch_device` 实际会改状态，但用户要求不审批，见 decision-20261104-130000);Hermes profile 里的 `pocket_agent` 仍配 `trust: untrusted`,给以后加的、没标 `readOnlyHint` 的工具兜底。审批经 `/v1/runs` 的审批事件转发到 PA 的审批界面(PA 兼容两种事件格式：旧的 `request_id`,以及 session authority 模式的 `prompt_id` + `execution_generation`,回应时按原格式带回);`/bridge/send` 回合走 chat/completions,没人能批，Hermes 等审批超时(60 秒)后按未批准处理。

**PA 更新了工具列表后要重启 Hermes 网关**(`launchctl kickstart -k gui/$(id -u)/ai.hermes.gateway-<profile>`),Hermes 只在启动时读 `tools/list`。
