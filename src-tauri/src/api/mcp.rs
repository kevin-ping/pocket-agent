// mcp.rs — Minimal MCP server for Hermes (FEAT-CAM1 / FEAT-CAM2 / FEAT-DEV1)
//
// Phone camera (open / snapshot / close) and switching between phone and
// desktop. Hermes decides when to use them; PA carries them out and answers
// with the plain fact. Every result is a content block, never a JSON-RPC
// error. Every tool carries readOnlyHint, so none asks for approval; the
// server stays `trust: untrusted` in Hermes so a future tool without the hint
// asks (Hermes approvals never remember "session" / "always").
// Streamable HTTP with plain JSON responses: POST /mcp carries one JSON-RPC
// message; there is no server-initiated stream (GET → 405) and no session.
// Auth: `Authorization: Bearer <API_SERVER_KEY>`, and unlike the other bearer
// routes an unset key rejects every request.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{json, Value};

use super::server::ServerState;
use crate::voice::remote::{Camera, Facing};

pub const SNAPSHOT: &str = "phone_camera_snapshot";
pub const CAMERA_OPEN: &str = "phone_camera_open";
pub const CAMERA_CLOSE: &str = "phone_camera_close";
pub const SWITCH_DEVICE: &str = "switch_device";

pub const FACING_MISSING: &str = "用户没有说明用前置还是后置摄像头";
pub const FACING_INVALID: &str = "facing 只能是 \"front\"(前置)或 \"back\"(后置)";
pub const TARGET_INVALID: &str = "target 只能是 \"desktop\"(电脑)或 \"phone\"(手机)";
pub const CAMERA_CLOSING: &str = "已通知手机关闭相机";
pub const CAMERA_ALREADY_OFF: &str = "手机相机本来就没有打开";
pub const CAMERA_SWITCH_FAILED: &str = "切换摄像头失败，手机相机已关闭";
/// The first open may wait on the Android permission dialog; a flip waits on the lens rebinding.
const CAMERA_OPEN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// JSON-RPC requests here are tiny; anything bigger is not for us.
pub const MAX_BODY: usize = 64 * 1024;
const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

pub async fn mcp_post(State(state): State<Arc<ServerState>>, headers: HeaderMap, body: Bytes) -> Response {
    if !authorized(&state.api_key, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match respond(&body).await {
        Some(reply) => ([(header::CONTENT_TYPE, "application/json")], reply.to_string()).into_response(),
        // Notifications (no id) and client responses get no body.
        None => StatusCode::ACCEPTED.into_response(),
    }
}

async fn respond(body: &[u8]) -> Option<Value> {
    let Ok(message) = serde_json::from_slice::<Value>(body) else {
        return Some(error(Value::Null, -32700, "parse error"));
    };
    if !message.is_object() {
        return Some(error(Value::Null, -32600, "expected a single JSON-RPC message"));
    }
    let id = message.get("id").cloned()?;
    let method = message.get("method").and_then(Value::as_str)?;
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    Some(match method {
        "initialize" => result(id, initialize(&params)),
        "ping" => result(id, json!({})),
        "tools/list" => result(id, json!({ "tools": tools() })),
        "tools/call" => {
            let args = params.get("arguments").cloned().unwrap_or(Value::Null);
            match params.get("name").and_then(Value::as_str) {
                Some(SNAPSHOT) => result(id, snapshot_result(crate::voice::remote::snapshot().await)),
                Some(CAMERA_OPEN) => result(id, text(&camera_open(&args).await)),
                Some(CAMERA_CLOSE) => result(id, text(camera_close())),
                Some(SWITCH_DEVICE) => result(id, text(&switch_device(&args))),
                _ => error(id, -32602, "unknown tool"),
            }
        }
        _ => error(id, -32601, "method not found"),
    })
}

pub async fn mcp_other(State(state): State<Arc<ServerState>>, headers: HeaderMap) -> Response {
    if !authorized(&state.api_key, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    StatusCode::METHOD_NOT_ALLOWED.into_response()
}

fn authorized(expected: &str, headers: &HeaderMap) -> bool {
    let provided = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    super::remote::token_ok(expected, provided)
}

fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = asked.filter(|v| PROTOCOL_VERSIONS.contains(v)).unwrap_or(PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "pocket-agent", "version": env!("CARGO_PKG_VERSION") },
    })
}

fn tools() -> Value {
    let none = json!({ "type": "object", "properties": {} });
    json!([
        {
            "name": CAMERA_OPEN,
            "description": "Opens the camera on the user's phone (the phone running Pocket Agent) with the front or back lens, or switches lens if it is already open. The camera stays open until phone_camera_close. Without `facing` it does nothing and says the user has not said which lens. Returns a text with the outcome.",
            "inputSchema": {
                "type": "object",
                "properties": { "facing": { "type": "string", "enum": ["front", "back"], "description": "The lens the user asked for: front = selfie lens, back = rear lens" } },
            },
            "annotations": { "title": "Open phone camera", "readOnlyHint": true, "openWorldHint": false },
        },
        {
            "name": SNAPSHOT,
            "description": "Returns the current frame from the camera on the user's phone (the phone running Pocket Agent), as a JPEG. Works only while the camera is open (phone_camera_open opens it); otherwise returns a text saying so.",
            "inputSchema": none,
            "annotations": { "title": "Phone camera snapshot", "readOnlyHint": true, "openWorldHint": false },
        },
        {
            "name": CAMERA_CLOSE,
            "description": "Closes the camera on the user's phone. Returns a text with the outcome.",
            "inputSchema": none,
            "annotations": { "title": "Close phone camera", "readOnlyHint": true, "openWorldHint": false },
        },
        {
            "name": SWITCH_DEVICE,
            "description": "Moves Pocket Agent's screen and voice between the user's phone and this computer. The switch happens after the current reply has been spoken. Moving to the phone needs the Pocket Agent phone app open in the foreground. Returns a text with the outcome.",
            "inputSchema": {
                "type": "object",
                "properties": { "target": { "type": "string", "enum": ["desktop", "phone"] } },
                "required": ["target"],
            },
            // Not read-only, but the user asked for no approval and Hermes has no
            // per-tool allowance (decision-20261104-130000).
            "annotations": { "title": "Switch device", "readOnlyHint": true, "openWorldHint": false },
        },
    ])
}

fn snapshot_result(frame: Result<String, &'static str>) -> Value {
    match frame {
        Ok(jpeg) => json!({ "content": [{ "type": "image", "data": jpeg, "mimeType": "image/jpeg" }] }),
        Err(fact) => text(fact),
    }
}

fn text(fact: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": fact }] })
}

fn lens(facing: Facing) -> &'static str {
    match facing {
        Facing::Front => "前置",
        Facing::Back => "后置",
    }
}

/// Absent and null both mean "not given".
fn string_arg<'a>(args: &'a Value, name: &str) -> Option<Option<&'a str>> {
    match args.get(name) {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_str()),
    }
}

/// Succeeds only once the phone reports the asked-for lens bound, so a
/// snapshot right after never gets a frame from the old lens.
async fn camera_open(args: &Value) -> String {
    let (facing, name) = match string_arg(args, "facing") {
        None => return FACING_MISSING.into(),
        Some(Some(name @ "front")) => (Facing::Front, name),
        Some(Some(name @ "back")) => (Facing::Back, name),
        Some(_) => return FACING_INVALID.into(),
    };
    if let Err(fact) = super::remote::phone_ui_ready() {
        return fact.into();
    }
    let ready = Some(Camera::ready(facing));
    let before = crate::voice::remote::camera();
    if before == ready {
        return format!("手机相机已经打开，用的就是{}摄像头", lens(facing));
    }
    // The page owns the viewfinder box, so it opens (or flips) the camera.
    if !super::remote::send_to_ui("phone-camera", json!({ "action": "open", "facing": name })) {
        return super::remote::PHONE_UI_ABSENT.into();
    }
    let was_on = before.is_some();
    // An open camera closing meanwhile is the flip failing; a closed one may still be behind the permission dialog.
    let after = crate::voice::remote::wait_camera(|c| c == ready || (was_on && c.is_none()), CAMERA_OPEN_TIMEOUT).await;
    camera_open_answer(facing, was_on, after)
}

fn camera_open_answer(facing: Facing, was_on: bool, after: Option<Camera>) -> String {
    match after {
        Some(c) if c == Camera::ready(facing) && was_on => format!("手机相机已切换到{}摄像头", lens(facing)),
        Some(c) if c == Camera::ready(facing) => format!("手机相机已打开({}摄像头)", lens(facing)),
        None if was_on => CAMERA_SWITCH_FAILED.into(),
        None => crate::voice::remote::CAMERA_OPEN_TIMED_OUT.into(),
        Some(_) => format!("已通知手机用{}摄像头，但手机没有确认镜头已切换好", lens(facing)),
    }
}

fn camera_close() -> &'static str {
    if !crate::voice::remote::is_camera_on() {
        return CAMERA_ALREADY_OFF;
    }
    if let Err(fact) = super::remote::phone_ui_ready() {
        return fact;
    }
    if !super::remote::send_to_ui("phone-camera", json!({ "action": "close" })) {
        return super::remote::PHONE_UI_ABSENT;
    }
    CAMERA_CLOSING
}

fn switch_device(args: &Value) -> String {
    use super::remote::{request_switch, Device};
    match string_arg(args, "target") {
        Some(Some("desktop")) => request_switch(Device::Desktop),
        Some(Some("phone")) => request_switch(Device::Phone),
        _ => TARGET_INVALID.into(),
    }
}

fn result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bearer(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, value.parse().unwrap());
        headers
    }

    async fn call(body: &str) -> Option<Value> {
        respond(body.as_bytes()).await
    }

    #[test]
    fn bearer_must_be_configured_present_and_equal() {
        assert!(!authorized("secret", &HeaderMap::new()));
        assert!(!authorized("secret", &bearer("Bearer wrong!")));
        assert!(!authorized("secret", &bearer("secret")), "scheme is required");
        assert!(!authorized("", &bearer("Bearer ")), "unset key has no open mode");
        assert!(authorized("secret", &bearer("Bearer secret")));
    }

    #[tokio::test]
    async fn initialize_echoes_a_supported_version_and_offers_tools() {
        let reply = call(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#).await.unwrap();
        assert_eq!(reply["id"], 1);
        assert_eq!(reply["result"]["protocolVersion"], "2025-03-26");
        assert!(reply["result"]["capabilities"]["tools"].is_object());
        let reply = call(r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#).await.unwrap();
        assert_eq!(reply["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
    }

    #[tokio::test]
    async fn tools_list_marks_every_tool_read_only() {
        let reply = call(r#"{"jsonrpc":"2.0","id":"a","method":"tools/list"}"#).await.unwrap();
        let tools = reply["result"]["tools"].as_array().unwrap();
        let read_only: Vec<(&str, bool)> = tools
            .iter()
            .map(|t| (t["name"].as_str().unwrap(), t["annotations"]["readOnlyHint"] == true))
            .collect();
        assert_eq!(read_only, vec![(CAMERA_OPEN, true), (SNAPSHOT, true), (CAMERA_CLOSE, true), (SWITCH_DEVICE, true)]);
        assert_eq!(tools[0]["inputSchema"]["properties"]["facing"]["enum"], json!(["front", "back"]));
        assert!(tools[0]["inputSchema"].get("required").is_none(), "facing is optional: Hermes asks the user");
        assert_eq!(tools[3]["inputSchema"]["required"], json!(["target"]));
    }

    fn call_text(reply: &Value) -> &str {
        reply["result"]["content"][0]["text"].as_str().unwrap()
    }

    #[tokio::test]
    async fn camera_open_needs_a_valid_facing_before_anything_happens() {
        // No /ui client in tests: had the facing passed, the answer would be PHONE_UI_ABSENT.
        for args in ["{}", r#"{"facing":null}"#] {
            let body = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"phone_camera_open","arguments":{args}}}}}"#);
            assert_eq!(call_text(&call(&body).await.unwrap()), FACING_MISSING);
        }
        let reply = call(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"phone_camera_open"}}"#).await.unwrap();
        assert_eq!(call_text(&reply), FACING_MISSING);
        for bad in [r#""left""#, "1", r#""FRONT""#] {
            let body = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"phone_camera_open","arguments":{{"facing":{bad}}}}}}}"#);
            assert_eq!(call_text(&call(&body).await.unwrap()), FACING_INVALID);
        }
    }

    #[tokio::test]
    async fn camera_and_switch_tools_state_the_fact_when_the_phone_is_not_there() {
        let _t = crate::voice::remote::TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let reply = call(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"phone_camera_open","arguments":{"facing":"back"}}}"#).await.unwrap();
        assert_eq!(call_text(&reply), super::super::remote::PHONE_UI_ABSENT);
        let reply = call(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"phone_camera_close"}}"#).await.unwrap();
        assert_eq!(call_text(&reply), CAMERA_ALREADY_OFF);
        // No /ui page and no paired phone online on /phone.
        let reply = call(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"switch_device","arguments":{"target":"phone"}}}"#).await.unwrap();
        assert_eq!(call_text(&reply), super::super::phone::NO_ONLINE_PHONE);
        let reply = call(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"switch_device","arguments":{"target":"desktop"}}}"#).await.unwrap();
        assert_eq!(call_text(&reply), super::super::remote::ALREADY_ON_DESKTOP);
        let reply = call(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"switch_device","arguments":{"target":"tv"}}}"#).await.unwrap();
        assert_eq!(call_text(&reply), TARGET_INVALID);
    }

    #[tokio::test]
    async fn notifications_get_no_body_and_unknown_requests_get_errors() {
        assert!(call(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).await.is_none());
        assert_eq!(call("not json").await.unwrap()["error"]["code"], -32700);
        assert_eq!(call(r#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#).await.unwrap()["error"]["code"], -32600);
        assert_eq!(call(r#"{"jsonrpc":"2.0","id":1,"method":"resources/list"}"#).await.unwrap()["error"]["code"], -32601);
        let reply = call(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"other"}}"#).await.unwrap();
        assert_eq!(reply["error"]["code"], -32602);
    }

    #[test]
    fn camera_open_reports_success_only_for_the_lens_the_phone_confirmed() {
        let front = Some(Camera::ready(Facing::Front));
        let switching = Some(Camera { facing: Some(Facing::Front), switching: true });
        assert_eq!(camera_open_answer(Facing::Front, true, front), "手机相机已切换到前置摄像头");
        assert_eq!(camera_open_answer(Facing::Front, false, front), "手机相机已打开(前置摄像头)");
        assert_eq!(camera_open_answer(Facing::Front, true, None), CAMERA_SWITCH_FAILED, "bind failed: the phone closed it");
        assert_eq!(camera_open_answer(Facing::Front, false, None), crate::voice::remote::CAMERA_OPEN_TIMED_OUT);
        for unconfirmed in [switching, Some(Camera::ready(Facing::Back)), Some(Camera { facing: None, switching: false })] {
            let answer = camera_open_answer(Facing::Front, true, unconfirmed);
            assert!(answer.contains("没有确认"), "{unconfirmed:?} → {answer}");
        }
    }

    #[test]
    fn snapshot_results_are_an_image_or_the_fact_as_text() {
        let image = snapshot_result(Ok("AAAA".into()));
        assert_eq!(image["content"][0]["type"], "image");
        assert_eq!(image["content"][0]["mimeType"], "image/jpeg");
        let off = snapshot_result(Err(crate::voice::remote::CAMERA_OFF));
        assert_eq!(off["content"][0]["type"], "text");
        assert!(off["content"][0]["text"].as_str().unwrap().contains("phone_camera_open"));
    }
}
