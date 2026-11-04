//! Hermes run transport: explicit approvals and cancellation for desktop turns.
use super::client::StreamEvent;
use crate::commands::config::{get_api_key, get_api_url};
use eventsource_stream::Eventsource;
use futures_util::{stream::BoxStream, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Emitter};

#[derive(Clone)]
struct Run {
    url: String,
    key: Option<String>,
    pending: HashMap<String, Approval>,
    cancelled: bool,
}
/// One unanswered Hermes request: the choices it offered, and the
/// `computer-approval` payload, kept so a reloaded frontend can show it again.
#[derive(Clone)]
struct Approval {
    choices: Vec<String>,
    shown: Value,
    /// Session-authority runs fence each prompt to an execution generation,
    /// echoed back in the answer.
    generation: Option<i64>,
}

/// The id to answer an approval.request with. Legacy runs send `request_id`;
/// session-authority runs send the prompt's `prompt_id` plus its
/// `execution_generation` (both required then).
fn approval_ref(event: &Value) -> Result<(String, Option<i64>), String> {
    let missing = || "Hermes 批准请求缺少编号，已停止任务。".to_string();
    let valid = |id: &Value| id.as_str().filter(|s| !s.is_empty() && s.len() <= 256).map(String::from);
    if let Some(id) = valid(&event["request_id"]) {
        return Ok((id, None));
    }
    let id = valid(&event["prompt_id"]).ok_or_else(missing)?;
    let generation = event["execution_generation"].as_i64().ok_or_else(missing)?;
    Ok((id, Some(generation)))
}

/// Session-authority runs accept exactly these keys.
fn approval_answer(request_id: &str, generation: Option<i64>, choice: &str) -> Value {
    match generation {
        Some(generation) => json!({"request_id": request_id, "execution_generation": generation, "choice": choice}),
        None => json!({"request_id": request_id, "choice": choice}),
    }
}
fn active() -> &'static Mutex<HashMap<String, Run>> {
    static RUNS: OnceLock<Mutex<HashMap<String, Run>>> = OnceLock::new();
    RUNS.get_or_init(|| Mutex::new(HashMap::new()))
}
pub(super) fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("HTTP client")
}
/// 401 / 403 from the gateway: the key PA sends is not the gateway's.
pub(super) fn auth_failed(status: reqwest::StatusCode) -> String {
    format!(
        "Hermes 拒绝了密钥（HTTP {}）：PA 的 API_SERVER_KEY 必须与网关 ~/.hermes/.env 中的 API_SERVER_KEY 一致（{}）。",
        status.as_u16(),
        crate::commands::config::gateway_hint()
    )
}

pub(super) fn authorized(request: reqwest::RequestBuilder, key: &Option<String>) -> reqwest::RequestBuilder {
    if let Some(key) = key {
        request.bearer_auth(key)
    } else {
        request
    }
}
struct ControlError {
    message: String,
    // Only connection establishment failures prove that no approval was delivered.
    retryable: bool,
}
async fn control(run: &Run, id: &str, action: &str, body: Value) -> Result<(), ControlError> {
    let response = authorized(
        http().post(format!("{}/v1/runs/{}/{}", run.url, id, action)),
        &run.key,
    )
    .timeout(std::time::Duration::from_secs(10))
    .json(&body)
    .send()
    .await
    .map_err(|error| ControlError {
        message: "Hermes 操作响应失败，请检查连接；不要重复执行可能已完成的操作。".into(),
        retryable: error.is_connect(),
    })?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(ControlError {
            message: format!("Hermes 操作响应失败（HTTP {}）。", response.status()),
            retryable: false,
        })
    }
}

#[tauri::command]
pub async fn respond_computer_approval(
    run_id: String,
    request_id: String,
    choice: String,
) -> Result<(), String> {
    // Only exact pending requests owned by this PA process can be answered, and only with a
    // choice Hermes offered for that request. Claim it before sending so it cannot be reused.
    let (run, approval) = {
        let mut runs = active().lock().unwrap();
        let run = runs.get_mut(&run_id).ok_or("该电脑任务已经结束。")?;
        if run.cancelled {
            return Err("该电脑任务已取消。".into());
        }
        let approval = run
            .pending
            .get(&request_id)
            .ok_or("该操作已经处理或已过期。")?;
        if !approval.choices.contains(&choice) {
            return Err("该操作不允许此批准方式。".into());
        }
        let approval = run.pending.remove(&request_id).ok_or("该操作已经处理或已过期。")?;
        (run.clone(), approval)
    };
    if let Err(error) = control(
        &run,
        &run_id,
        "approval",
        approval_answer(&request_id, approval.generation, &choice),
    )
    .await
    {
        if error.retryable {
            restore_approval(&run_id, request_id, approval);
        }
        return Err(error.message);
    }
    Ok(())
}

fn restore_approval(run_id: &str, request_id: String, approval: Approval) {
    if let Some(run) = active().lock().unwrap().get_mut(run_id) {
        if !run.cancelled {
            run.pending.entry(request_id).or_insert(approval);
        }
    }
}

/// The `computer-approval` payloads still waiting for an answer. A frontend
/// that (re)mounts after they were emitted — the desktop reloading out of the
/// phone shell, a phone reconnecting — shows them from here. Answering still
/// goes through respond_computer_approval's exact run / request / choice check.
#[tauri::command]
pub fn pending_computer_approvals() -> Vec<Value> {
    active()
        .lock()
        .unwrap()
        .values()
        .filter(|run| !run.cancelled)
        .flat_map(|run| run.pending.values().map(|a| a.shown.clone()))
        .collect()
}

pub async fn cancel_all() -> Result<(), String> {
    let runs = {
        let mut runs = active().lock().unwrap();
        for run in runs.values_mut() {
            run.cancelled = true;
            run.pending.clear();
        }
        runs.clone()
    };
    let mut errors = Vec::new();
    for (id, run) in runs {
        if let Err(error) = control(&run, &id, "stop", json!({})).await {
            errors.push(error.message);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join(" "))
    }
}

pub async fn chat_stream(
    app: &AppHandle,
    text: &str,
    hint: Option<&str>,
    context: Option<&str>,
    session: &str,
) -> Result<BoxStream<'static, Result<StreamEvent, String>>, String> {
    let run = Run {
        url: get_api_url().trim_end_matches('/').into(),
        key: get_api_key(),
        pending: HashMap::new(),
        cancelled: false,
    };
    let instructions = [context.unwrap_or(""), hint.unwrap_or("")].join("\n\n");
    // No automatic replay of run creation. A lost response may mean it already ran.
    let response = authorized(http().post(format!("{}/v1/runs", run.url)), &run.key)
        .timeout(std::time::Duration::from_secs(15))
        .json(&json!({"input": text, "instructions": instructions, "session_id": session}))
        .send()
        .await
        .map_err(|e| {
            if e.is_connect() {
                // Refused before anything was sent: the run was not created.
                format!("连不上 Hermes 网关：请确认网关已启动、API_SERVER 正确（{}）。", crate::commands::config::gateway_hint())
            } else {
                "Hermes 任务创建响应丢失，执行状态未知；请检查后再重试。".to_string()
            }
        })?;
    if !response.status().is_success() {
        let status = response.status();
        if matches!(status.as_u16(), 401 | 403) {
            return Err(auth_failed(status));
        }
        return Err(format!(
            "Hermes 任务接口失败（HTTP {}）。电脑交互需要支持 /v1/runs 的 Hermes 网关。",
            status
        ));
    }
    let value: Value = response
        .json()
        .await
        .map_err(|_| "Hermes 任务返回格式无效。")?;
    let id = value["run_id"]
        .as_str()
        .filter(|s| valid_run_id(s))
        .ok_or("Hermes 未返回有效任务编号，执行状态未知。")?
        .to_string();
    active().lock().unwrap().insert(id.clone(), run.clone());
    let (tx, rx) = tokio::sync::mpsc::channel(64);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut terminal = false;
        let result: Result<(), String> = async {
            let connect = authorized(
                http().get(format!("{}/v1/runs/{}/events", run.url, id)),
                &run.key,
            )
            .send();
            let response = tokio::select! {
                _ = tx.closed() => return Ok(()),
                result = tokio::time::timeout(std::time::Duration::from_secs(15), connect) =>
                    result.map_err(|_| "电脑任务事件连接超时。".to_string())?
                        .map_err(|_| "电脑任务事件连接失败。".to_string())?,
            };
            if !response.status().is_success() {
                return Err(format!(
                    "电脑任务事件接口失败（HTTP {}）。",
                    response.status()
                ));
            }
            let mut events = response.bytes_stream().eventsource();
            let mut received_text = false;
            loop {
                let next = tokio::select! {
                    _ = tx.closed() => return Ok(()),
                    event = events.next() => event,
                };
                let Some(next) = next else {
                    return Err("电脑任务连接提前结束，结果尚未确认。".into());
                };
                let event = next.map_err(|_| "电脑任务连接中断，结果尚未确认。".to_string())?;
                let value: Value = serde_json::from_str(&event.data)
                    .map_err(|_| "电脑任务事件格式无效。".to_string())?;
                match value["event"].as_str().unwrap_or("") {
                    "message.delta" => {
                        if let Some(delta) = value["delta"].as_str().filter(|s| !s.is_empty()) {
                            received_text = true;
                            if tx
                                .send(Ok(StreamEvent::Content(delta.into())))
                                .await
                                .is_err()
                            {
                                return Ok(());
                            }
                        }
                    }
                    // Streamed reasoning (local Hermes patch, see HERMES.md); chunks join as-is.
                    "reasoning.delta" => {
                        if let Some(delta) = value["delta"].as_str().filter(|s| !s.is_empty()) {
                            if tx
                                .send(Ok(StreamEvent::Reasoning(delta.into())))
                                .await
                                .is_err()
                            {
                                return Ok(());
                            }
                        }
                    }
                    "reasoning.available" => {
                        if let Some(text) = reasoning_text(&value) {
                            // Each event is a whole block; the newline keeps blocks apart.
                            if tx
                                .send(Ok(StreamEvent::Reasoning(format!("{}\n", text))))
                                .await
                                .is_err()
                            {
                                return Ok(());
                            }
                        }
                    }
                    "tool.started" => {
                        if tx
                            .send(Ok(tool_started(&id, &value)))
                            .await
                            .is_err()
                        {
                            return Ok(());
                        }
                    }
                    // Lifecycle status (local Hermes patch, see HERMES.md); only compaction is shown.
                    "hermes.status" => {
                        if let Some(active) = compaction_status(&value) {
                            if tx.send(Ok(StreamEvent::Compaction(active))).await.is_err() {
                                return Ok(());
                            }
                        }
                    }
                    "approval.request" => {
                        let (request_id, generation) = approval_ref(&value)?;
                        let request_id = request_id.as_str();
                        let choices: Vec<String> = value["choices"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect();
                        if !choices.iter().any(|choice| choice == "deny") {
                            return Err("Hermes 批准请求缺少拒绝选项，已停止任务。".into());
                        }
                        let shown = json!({"run_id": id, "request_id": request_id,
                            "choices": choices, "command": value["command"].as_str().unwrap_or(""),
                            "description": value["description"].as_str().unwrap_or("")});
                        if let Some(run) = active().lock().unwrap().get_mut(&id) {
                            if run.cancelled {
                                continue;
                            }
                            run.pending.insert(request_id.into(), Approval { choices, shown: shown.clone(), generation });
                        } else {
                            continue;
                        }
                        app.emit("computer-approval", shown).map_err(|e| e.to_string())?;
                    }
                    "run.completed" | "run.failed" | "run.cancelled" | "run.interrupted" => {
                        terminal = true;
                        if let Some(output) = terminal_reply(&value, received_text)? {
                            let _ = tx.send(Ok(StreamEvent::Content(output))).await;
                        }
                        return Ok(());
                    }
                    _ => {}
                }
            }
        }
        .await;
        if !terminal {
            if let Err(error) = control(&run, &id, "stop", json!({})).await {
                let _ = app.emit(
                    "computer-control-error",
                    format!("{} 后台任务可能仍在运行。", error.message),
                );
            }
        }
        active().lock().unwrap().remove(&id);
        let _ = app.emit("computer-approval-clear", &id);
        if let Err(error) = result {
            let _ = tx.send(Err(error)).await;
        }
    });
    Ok(futures_util::stream::unfold(rx, |mut rx| async {
        rx.recv().await.map(|event| (event, rx))
    })
    .boxed())
}
fn terminal_reply(value: &Value, received_text: bool) -> Result<Option<String>, String> {
    match value["event"].as_str() {
        Some("run.completed") => completion_reply(value, received_text),
        // Hard failures carry an error; partial/budget exits carry completion fields
        // and may carry the agent's own explanation instead.
        Some("run.failed")
            if (value["partial"] == true || value["completed"] == false)
                && value["interrupted"] != true
                && value.get("error").is_none_or(|error| {
                    error.is_null() || error.as_str().is_some_and(|text| text.trim().is_empty())
                }) =>
        {
            completion_reply(value, received_text)
        }
        _ => Err(value["error"]
            .as_str()
            .unwrap_or("电脑任务已停止或未完成；请检查当前窗口确认结果。")
            .into()),
    }
}
/// Hermes' own reply is relayed even for a partial run; only a run with no reply at all fails.
/// Returns the output still to be spoken, if it was not already streamed.
fn completion_reply(value: &Value, received_text: bool) -> Result<Option<String>, String> {
    if received_text {
        return Ok(None);
    }
    if let Some(output) = value["output"].as_str().filter(|s| !s.trim().is_empty()) {
        return Ok(Some(output.into()));
    }
    if value["completed"] == false || value["partial"] == true {
        Err("Hermes 任务只完成了一部分，请检查实际结果。".into())
    } else {
        Err("Hermes 任务结束，但未提供可核实的回复。".into())
    }
}
fn reasoning_text(value: &Value) -> Option<String> {
    value["text"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .map(String::from)
}
/// Some(true) while Hermes compacts the context, Some(false) when it is done, None for
/// any other status. Matches Hermes' status lines in agent/conversation_compression.py;
/// warnings (e.g. a failed summary) are not progress.
fn compaction_status(value: &Value) -> Option<bool> {
    match value["kind"].as_str() {
        Some("compacted") => return Some(false),
        Some("warn" | "error") => return None,
        _ => {}
    }
    let text = value["text"].as_str()?.to_lowercase();
    (text.contains("compacting") || text.contains("compression")).then_some(true)
}
fn tool_started(run_id: &str, value: &Value) -> StreamEvent {
    StreamEvent::ToolCallStart {
        id: format!("{}-{}", run_id, value["timestamp"]),
        name: value["tool"].as_str().unwrap_or("computer_use").to_string(),
        preview: value["preview"].as_str().unwrap_or("").to_string(),
    }
}
fn valid_run_id(id: &str) -> bool {
    id.starts_with("run_")
        && id.len() <= 128
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    // Registry-wide cancellation must not race unrelated tests sharing active().
    static REGISTRY_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    #[test]
    fn compaction_status_tracks_hermes_compaction_lines() {
        let status = |kind: &str, text: &str| json!({"event": "hermes.status", "kind": kind, "text": text});
        assert_eq!(compaction_status(&status("lifecycle",
            "📦 Preflight compression: ~129,641 tokens >= 80,000 threshold. This may take a moment.")), Some(true));
        assert_eq!(compaction_status(&status("lifecycle",
            "🗜️ Compacting context — still summarizing earlier conversation so I can continue...")), Some(true));
        assert_eq!(compaction_status(&status("compacted",
            "✓ Context compaction complete — continuing turn...")), Some(false));
        assert_eq!(compaction_status(&status("lifecycle", "⏳ Waiting for provider...")), None);
        assert_eq!(compaction_status(&status("lifecycle", "")), None);
        assert_eq!(compaction_status(&status("warn", "Context compression failed")), None);
        assert_eq!(compaction_status(&json!({"event": "hermes.status"})), None);
    }
    #[test]
    fn rejects_run_ids_that_can_escape_control_path() {
        assert!(valid_run_id("run_123abc"));
        assert!(!valid_run_id("run_../../stop"));
        assert!(!valid_run_id("other"));
    }
    #[test]
    fn reasoning_text_relays_only_non_empty_text() {
        let event = json!({"event": "reasoning.available", "text": "先打开浏览器"});
        assert_eq!(reasoning_text(&event), Some("先打开浏览器".into()));
        assert_eq!(reasoning_text(&json!({"text": "  \n"})), None);
        assert_eq!(reasoning_text(&json!({"event": "reasoning.available"})), None);
    }
    #[test]
    fn tool_started_relays_hermes_preview() {
        let event = json!({"event": "tool.started", "tool": "terminal",
            "preview": "open -a WeChat", "timestamp": 1});
        let StreamEvent::ToolCallStart { name, preview, .. } = tool_started("run_1", &event) else {
            panic!("expected ToolCallStart");
        };
        assert_eq!((name.as_str(), preview.as_str()), ("terminal", "open -a WeChat"));
        let StreamEvent::ToolCallStart { preview, .. } =
            tool_started("run_1", &json!({"event": "tool.started", "tool": "terminal"}))
        else {
            panic!("expected ToolCallStart");
        };
        assert_eq!(preview, "");
    }
    #[test]
    fn partial_runs_still_relay_hermes_reply() {
        let partial = json!({"event": "run.completed", "partial": true, "completed": false});
        assert_eq!(completion_reply(&partial, true), Ok(None));
        let mut with_output = partial.clone();
        with_output["output"] = json!("只打开了浏览器");
        assert_eq!(
            completion_reply(&with_output, false),
            Ok(Some("只打开了浏览器".into()))
        );
        assert!(completion_reply(&partial, false)
            .unwrap_err()
            .contains("只完成了一部分"));
        assert!(completion_reply(&json!({}), false)
            .unwrap_err()
            .contains("未提供"));
    }
    #[test]
    fn actual_partial_failed_events_relay_output_but_real_failures_do_not() {
        for fields in [json!({"partial": true}), json!({"completed": false})] {
            let mut event = fields;
            event["event"] = json!("run.failed");
            event["output"] = json!("只打开了浏览器，搜索没有完成。");
            assert_eq!(
                terminal_reply(&event, false),
                Ok(Some("只打开了浏览器，搜索没有完成。".into()))
            );
            assert_eq!(terminal_reply(&event, true), Ok(None));
            event["output"] = json!("  \n");
            assert!(terminal_reply(&event, false).is_err());
        }
        for name in ["run.failed", "run.cancelled", "run.interrupted"] {
            let event = json!({"event": name, "partial": true, "completed": false,
                "output": "不能掩盖错误", "error": "实际故障"});
            assert_eq!(terminal_reply(&event, false), Err("实际故障".into()));
            assert_eq!(terminal_reply(&event, true), Err("实际故障".into()));
        }
        for name in ["run.cancelled", "run.interrupted"] {
            assert!(terminal_reply(
                &json!({"event": name, "partial": true,
                "output": "已停止"}),
                true
            )
            .is_err());
        }
        assert!(terminal_reply(
            &json!({"event": "run.failed", "partial": true,
            "interrupted": true, "output": "已停止"}),
            true
        )
        .is_err());
        assert!(terminal_reply(
            &json!({"event": "run.failed", "completed": false,
            "error": {"message": "故障"}, "output": "不能掩盖错误"}),
            true
        )
        .is_err());
    }
    #[tokio::test]
    async fn cannot_approve_unowned_run() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        assert!(
            respond_computer_approval("run_unknown".into(), "request".into(), "once".into())
                .await
                .is_err()
        );
    }
    fn register(id: &str, url: String, request: &str, choices: &[&str]) {
        let mut pending = HashMap::new();
        pending.insert(
            request.into(),
            Approval {
                choices: choices.iter().map(|c| c.to_string()).collect(),
                shown: json!({"run_id": id, "request_id": request, "choices": choices}),
                generation: None,
            },
        );
        active().lock().unwrap().insert(
            id.into(),
            Run {
                url,
                key: None,
                pending,
                cancelled: false,
            },
        );
    }

    /// A gateway stub that answers every request with 200 OK.
    async fn ok_gateway() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf).await;
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                    .await;
            }
        });
        url
    }

    #[tokio::test]
    async fn unanswered_approvals_can_be_shown_again_until_answered() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let id = "run_replay_test";
        register(id, ok_gateway().await, "request", &["once", "deny"]);
        let replay = pending_computer_approvals();
        assert!(replay.iter().any(|a| a["run_id"] == id && a["request_id"] == "request"));
        respond_computer_approval(id.into(), "request".into(), "once".into())
            .await
            .unwrap();
        assert!(!pending_computer_approvals().iter().any(|a| a["run_id"] == id));
        active().lock().unwrap().get_mut(id).unwrap().cancelled = true;
        register(id, ok_gateway().await, "again", &["deny"]);
        active().lock().unwrap().get_mut(id).unwrap().cancelled = true;
        assert!(!pending_computer_approvals().iter().any(|a| a["run_id"] == id), "cancelled runs show nothing");
        active().lock().unwrap().remove(id);
    }

    #[tokio::test]
    async fn cannot_widen_or_reuse_an_approval() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let id = "run_approval_test";
        register(
            id,
            "http://127.0.0.1:1".into(),
            "specific",
            &["once", "deny"],
        );
        // Choices Hermes did not offer for this request are refused, including session.
        for choice in ["session", "always"] {
            assert!(
                respond_computer_approval(id.into(), "specific".into(), choice.into())
                    .await
                    .is_err()
            );
        }
        assert!(
            respond_computer_approval(id.into(), "stale".into(), "once".into())
                .await
                .is_err()
        );
        // A failed delivery keeps the request answerable.
        assert!(
            respond_computer_approval(id.into(), "specific".into(), "once".into())
                .await
                .is_err()
        );
        assert!(active().lock().unwrap()[id]
            .pending
            .contains_key("specific"));
        active().lock().unwrap().remove(id);
    }

    #[test]
    fn approvals_are_answered_in_the_shape_their_run_uses() {
        // Legacy run: request_id, answered with request_id + choice.
        let legacy = json!({"event": "approval.request", "request_id": "r1", "choices": ["once", "deny"]});
        assert_eq!(approval_ref(&legacy), Ok(("r1".into(), None)));
        assert_eq!(approval_answer("r1", None, "once"), json!({"request_id": "r1", "choice": "once"}));
        // Session-authority run (MCP trust gate seen 2026-11-04): prompt_id + execution_generation,
        // answered with exactly request_id, execution_generation and choice.
        let authority = json!({"event": "approval.request", "kind": "approval", "prompt_id": "p9",
                               "execution_generation": 3, "choices": ["once", "deny", "session", "always"]});
        assert_eq!(approval_ref(&authority), Ok(("p9".into(), Some(3))));
        assert_eq!(approval_answer("p9", Some(3), "deny"),
                   json!({"request_id": "p9", "execution_generation": 3, "choice": "deny"}));
        // Anything else stops the run rather than guessing.
        for bad in [json!({}), json!({"prompt_id": "p9"}), json!({"prompt_id": "p9", "execution_generation": "3"}),
                    json!({"request_id": ""}), json!({"prompt_id": "x".repeat(257), "execution_generation": 1})] {
            assert!(approval_ref(&bad).is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn relays_offered_session_choice_once() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let id = "run_session_test";
        register(
            id,
            ok_gateway().await,
            "request",
            &["once", "session", "always", "deny"],
        );
        respond_computer_approval(id.into(), "request".into(), "session".into())
            .await
            .unwrap();
        // The answered request cannot be replayed.
        assert!(
            respond_computer_approval(id.into(), "request".into(), "session".into())
                .await
                .is_err()
        );
        active().lock().unwrap().remove(id);
    }

    #[tokio::test]
    async fn cancellation_during_approval_delivery_does_not_restore_request() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let id = "run_cancel_approval_test";
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        register(
            id,
            format!("http://{}", listener.local_addr().unwrap()),
            "request",
            &["session", "deny"],
        );
        let (received_tx, received_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let gateway = tokio::spawn(async move {
            let (mut approval, _) = listener.accept().await.unwrap();
            let mut buf = [0; 4096];
            let n = approval.read(&mut buf).await.unwrap();
            assert!(String::from_utf8_lossy(&buf[..n]).contains("/approval"));
            received_tx.send(()).unwrap();
            let (mut stop, _) = listener.accept().await.unwrap();
            let n = stop.read(&mut buf).await.unwrap();
            assert!(String::from_utf8_lossy(&buf[..n]).contains("/stop"));
            stop.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                .await
                .unwrap();
            release_rx.await.unwrap();
            // Simulate a gateway that received the approval but lost its response.
            drop(approval);
        });
        let response = tokio::spawn(respond_computer_approval(
            id.into(),
            "request".into(),
            "session".into(),
        ));
        received_rx.await.unwrap();
        assert!(
            respond_computer_approval(id.into(), "request".into(), "session".into())
                .await
                .is_err()
        );
        cancel_all().await.unwrap();
        release_tx.send(()).unwrap();
        assert!(response.await.unwrap().is_err());
        gateway.await.unwrap();
        {
            let runs = active().lock().unwrap();
            assert!(runs[id].cancelled);
            assert!(runs[id].pending.is_empty());
        }
        assert!(
            respond_computer_approval(id.into(), "request".into(), "session".into())
                .await
                .is_err()
        );
        active().lock().unwrap().remove(id);
    }

    #[tokio::test]
    async fn lost_approval_response_cannot_be_replayed() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let id = "run_lost_approval_test";
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        register(
            id,
            format!("http://{}", listener.local_addr().unwrap()),
            "request",
            &["once", "deny"],
        );
        let gateway = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0; 4096];
            let n = socket.read(&mut buf).await.unwrap();
            assert!(String::from_utf8_lossy(&buf[..n]).contains("/approval"));
            // No response: delivery may already have caused side effects.
        });
        assert!(
            respond_computer_approval(id.into(), "request".into(), "once".into())
                .await
                .is_err()
        );
        gateway.await.unwrap();
        assert!(!active().lock().unwrap()[id].pending.contains_key("request"));
        assert!(
            respond_computer_approval(id.into(), "request".into(), "once".into())
                .await
                .is_err()
        );
        active().lock().unwrap().remove(id);
    }

    #[tokio::test]
    async fn retryable_failure_restores_only_a_live_uncancelled_request() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let id = "run_restore_approval_test";
        register(
            id,
            "http://127.0.0.1:1".into(),
            "request",
            &["once", "deny"],
        );
        let approval = active()
            .lock()
            .unwrap()
            .get_mut(id)
            .unwrap()
            .pending
            .remove("request")
            .unwrap();
        // An ordinary definitely-unsent failure may be retried.
        restore_approval(id, "request".into(), approval.clone());
        assert!(active().lock().unwrap()[id].pending.contains_key("request"));
        // cancel_all's locked invalidation occurs before the failed request completes.
        {
            let mut runs = active().lock().unwrap();
            let run = runs.get_mut(id).unwrap();
            run.pending.clear();
            run.cancelled = true;
        }
        restore_approval(id, "request".into(), approval.clone());
        assert!(active().lock().unwrap()[id].pending.is_empty());
        active().lock().unwrap().remove(id);
        restore_approval(id, "request".into(), approval);
        assert!(!active().lock().unwrap().contains_key(id));
    }
}
