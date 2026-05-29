# Pocket Agent API 手册

PA 在本地 `8650` 端口运行 HTTP 服务，所有接口需 Bearer token 认证（`API_SERVER_KEY`）。

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
| show_thinking | bool | ❌ | true | 是否在 PA 显示 thinking 动画 |

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
   - bridge-thinking（reasoning 更新）
   - bridge-tool-call（工具调用通知）
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
