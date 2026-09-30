//! Hermes run transport: explicit approvals and cancellation for desktop turns.
use super::client::StreamEvent;
use crate::commands::computer::{tool_allowed_for_route, DesktopRoute};
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
    pending: HashMap<String, Vec<String>>,
}
fn active() -> &'static Mutex<HashMap<String, Run>> {
    static RUNS: OnceLock<Mutex<HashMap<String, Run>>> = OnceLock::new();
    RUNS.get_or_init(|| Mutex::new(HashMap::new()))
}
fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("HTTP client")
}
fn authorized(request: reqwest::RequestBuilder, key: &Option<String>) -> reqwest::RequestBuilder {
    if let Some(key) = key {
        request.bearer_auth(key)
    } else {
        request
    }
}
async fn control(run: &Run, id: &str, action: &str, body: Value) -> Result<(), String> {
    let response = authorized(
        http().post(format!("{}/v1/runs/{}/{}", run.url, id, action)),
        &run.key,
    )
    .timeout(std::time::Duration::from_secs(10))
    .json(&body)
    .send()
    .await
    .map_err(|_| "Hermes 操作响应失败，请检查连接；不要重复执行可能已完成的操作。")?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!(
            "Hermes 操作响应失败（HTTP {}）。",
            response.status()
        ))
    }
}

#[tauri::command]
pub async fn respond_computer_approval(
    run_id: String,
    request_id: String,
    choice: String,
) -> Result<(), String> {
    // Only exact pending requests owned by this PA process can be approved, once or denied.
    let run = {
        let runs = active().lock().unwrap();
        let run = runs.get(&run_id).ok_or("该电脑任务已经结束。")?;
        let choices = run
            .pending
            .get(&request_id)
            .ok_or("该操作已经处理或已过期。")?;
        if !matches!(choice.as_str(), "once" | "deny") || !choices.contains(&choice) {
            return Err("该操作不允许此批准方式。".into());
        }
        run.clone()
    };
    control(
        &run,
        &run_id,
        "approval",
        json!({"request_id": request_id, "choice": choice}),
    )
    .await?;
    if let Some(run) = active().lock().unwrap().get_mut(&run_id) {
        run.pending.remove(&request_id);
    }
    Ok(())
}

pub async fn cancel_all() -> Result<(), String> {
    let runs = {
        let mut runs = active().lock().unwrap();
        for run in runs.values_mut() {
            run.pending.clear();
        }
        runs.clone()
    };
    let mut errors = Vec::new();
    for (id, run) in runs {
        if let Err(error) = control(&run, &id, "stop", json!({})).await {
            errors.push(error);
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
    route: DesktopRoute,
) -> Result<BoxStream<'static, Result<StreamEvent, String>>, String> {
    let run = Run {
        url: get_api_url().trim_end_matches('/').into(),
        key: get_api_key(),
        pending: HashMap::new(),
    };
    let instructions = [context.unwrap_or(""), hint.unwrap_or("")].join("\n\n");
    // No automatic replay of run creation. A lost response may mean it already ran.
    let response = authorized(http().post(format!("{}/v1/runs", run.url)), &run.key)
        .timeout(std::time::Duration::from_secs(15))
        .json(&json!({"input": text, "instructions": instructions, "session_id": session}))
        .send()
        .await
        .map_err(|_| "Hermes 任务创建响应丢失，执行状态未知；请检查后再重试。")?;
    if !response.status().is_success() {
        return Err(format!(
            "Hermes 任务接口失败（HTTP {}）。电脑交互需要支持 /v1/runs 的 Hermes 网关。",
            response.status()
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
            let mut reply = RunReply::new(route);
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
                        if let Some(delta) = reply.delta(value["delta"].as_str().unwrap_or("")) {
                            if tx
                                .send(Ok(StreamEvent::Content(delta)))
                                .await
                                .is_err()
                            {
                                return Ok(());
                            }
                        }
                    }
                    "tool.started" => {
                        let name = value["tool"].as_str().unwrap_or("computer_use").to_string();
                        if !tool_allowed_for_route(route, &name) {
                            let stopped = control(&run, &id, "stop", json!({})).await.is_ok();
                            terminal = stopped;
                            let _ = tx
                                .send(Err(format!(
                                    "Hermes 为当前 {:?} 路由选择了不允许的工具 {}，Pocket Agent 已停止该任务且不会把它当作成功。",
                                    route, name
                                )))
                                .await;
                            return Ok(());
                        }
                        if tx
                            .send(Ok(StreamEvent::ToolCallStart {
                                id: format!("{}-{}", id, value["timestamp"]),
                                name,
                            }))
                            .await
                            .is_err()
                        {
                            return Ok(());
                        }
                    }
                    "approval.request" => {
                        let request_id = value["request_id"]
                            .as_str()
                            .filter(|s| !s.is_empty() && s.len() <= 256)
                            .ok_or("Hermes 批准请求缺少编号，已停止任务。")?;
                        let choices: Vec<String> = value["choices"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect();
                        if !choices.iter().any(|choice| choice == "deny") {
                            return Err("Hermes 批准请求缺少拒绝选项，已停止任务。".into());
                        }
                        if let Some(run) = active().lock().unwrap().get_mut(&id) {
                            run.pending.insert(request_id.into(), choices.clone());
                        }
                        app.emit(
                            "computer-approval",
                            json!({"run_id": id, "request_id": request_id,
                            "choices": choices, "command": value["command"].as_str().unwrap_or(""),
                            "description": value["description"].as_str().unwrap_or("")}),
                        )
                        .map_err(|e| e.to_string())?;
                    }
                    "run.completed" => {
                        terminal = true;
                        if let Some(output) = reply.complete(&value)? {
                            let _ = tx.send(Ok(StreamEvent::Content(output))).await;
                        }
                        return Ok(());
                    }
                    "run.failed" | "run.cancelled" | "run.interrupted" => {
                        terminal = true;
                        return Err(value["error"]
                            .as_str()
                            .unwrap_or("电脑任务已停止或未完成；请检查当前窗口确认结果。")
                            .into());
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
                    format!("{} 后台任务可能仍在运行。", error),
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
fn valid_run_id(id: &str) -> bool {
    id.starts_with("run_")
        && id.len() <= 128
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Desktop narration never enters the Content stream (UI, speech and history).
/// Progress/approval events are handled independently and remain live.
struct RunReply {
    route: DesktopRoute,
    received_text: bool,
}

impl RunReply {
    fn new(route: DesktopRoute) -> Self {
        Self { route, received_text: false }
    }

    fn delta(&mut self, text: &str) -> Option<String> {
        if self.route == DesktopRoute::Visible || text.is_empty() {
            return None;
        }
        self.received_text = true;
        Some(text.into())
    }

    fn complete(&self, event: &Value) -> Result<Option<String>, String> {
        // Never flush a success-looking partial answer on failure or cancellation.
        if event["event"] != "run.completed" {
            return Err("电脑任务未完整结束。".into());
        }
        if event["completed"] == false || event["partial"] == true {
            return Err("Hermes 任务只完成了一部分，请检查实际结果。".into());
        }
        let output = event["output"].as_str().unwrap_or("");
        if self.route == DesktopRoute::Visible {
            return desktop_final_reply(output).map(Some);
        }
        if self.received_text {
            Ok(None)
        } else if output.is_empty() {
            Err("Hermes 任务结束，但未提供可核实的回复。".into())
        } else {
            Ok(Some(output.into()))
        }
    }
}

/// Whole final messages are available before playback, so even split or orphaned
/// model reasoning delimiters can be removed without leaking an earlier chunk.
fn desktop_final_reply(text: &str) -> Result<String, String> {
    static TAGS: OnceLock<regex::Regex> = OnceLock::new();
    let tags = TAGS.get_or_init(|| regex::Regex::new(r"(?i)<(/?)(?:mm:)?think\s*>").unwrap());
    let mut result = String::new();
    let mut cursor = 0;
    let mut depth: usize = 0;
    for captures in tags.captures_iter(text) {
        let tag = captures.get(0).unwrap();
        if depth == 0 {
            result.push_str(&text[cursor..tag.start()]);
        }
        if &captures[1] == "/" {
            if depth == 0 {
                // Some providers omit the opening tag, as observed in PA history.
                result.clear();
            } else {
                depth -= 1;
            }
        } else {
            depth += 1;
        }
        cursor = tag.end();
    }
    if depth == 0 {
        result.push_str(&text[cursor..]);
    }
    let result = result.trim().to_string();
    if result.is_empty() {
        Err("电脑任务结束，但未返回可核实的结果说明。".into())
    } else {
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_turn_only_emits_the_terminal_answer() {
        let mut reply = RunReply::new(DesktopRoute::Visible);
        // These represent model deltas before and between real tool calls.
        for delta in ["我准备先打开浏览器。", "内部推理</mm:", "think>", "已经完成。"] {
            assert_eq!(reply.delta(delta), None);
        }
        let completed = json!({"event": "run.completed", "completed": true,
            "partial": false, "output": "<think>检查工具结果</think>无法把窗口带到前台。"});
        assert_eq!(reply.complete(&completed).unwrap(), Some("无法把窗口带到前台。".into()));
        // No terminal answer: never fall back to earlier success-looking deltas.
        assert!(reply.complete(&json!({"event":"run.completed", "output":""})).is_err());
    }

    #[test]
    fn failed_or_partial_desktop_turn_never_flushes_success_text() {
        let reply = RunReply::new(DesktopRoute::Visible);
        for event in ["run.failed", "run.cancelled", "run.interrupted"] {
            assert!(reply.complete(&json!({"event":event, "output":"已完成。"})).is_err());
        }
        for flags in [(false, false), (true, true)] {
            assert!(reply.complete(&json!({"event":"run.completed", "completed":flags.0,
                "partial":flags.1, "output":"已完成。"})).is_err());
        }
    }

    #[test]
    fn other_routes_preserve_streaming_and_nonstreaming_fallback() {
        for route in [DesktopRoute::Conversation, DesktopRoute::Background, DesktopRoute::Uncertain] {
            let mut reply = RunReply::new(route);
            let event = json!({"event":"run.completed", "output":"回答。"});
            assert_eq!(reply.complete(&event).unwrap(), Some("回答。".into()));
            assert_eq!(reply.delta(""), None);
            assert_eq!(reply.delta("回答。"), Some("回答。".into()));
            assert_eq!(reply.complete(&event).unwrap(), None);
        }
    }
    #[test]
    fn desktop_reply_removes_reasoning_before_any_playback() {
        assert_eq!(desktop_final_reply("<think>计划\n再检查</think>浏览器已显示。").unwrap(), "浏览器已显示。");
        assert_eq!(desktop_final_reply("内部推理</mm:think>无法把窗口带到前台。").unwrap(), "无法把窗口带到前台。");
        assert_eq!(desktop_final_reply("<MM:THINK>推理<think>嵌套</think></MM:THINK>需要本次授权。").unwrap(), "需要本次授权。");
        assert!(desktop_final_reply("<think>未完成的内部推理").is_err());
        assert!(desktop_final_reply("  ").is_err());
        assert_eq!(desktop_final_reply("无法打开窗口，请先恢复浏览器。").unwrap(), "无法打开窗口，请先恢复浏览器。");
    }
    #[test]
    fn rejects_run_ids_that_can_escape_control_path() {
        assert!(valid_run_id("run_123abc"));
        assert!(!valid_run_id("run_../../stop"));
        assert!(!valid_run_id("other"));
    }
    #[tokio::test]
    async fn cannot_approve_unowned_run() {
        assert!(
            respond_computer_approval("run_unknown".into(), "request".into(), "once".into())
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn cannot_widen_or_reuse_an_approval() {
        let id = "run_approval_test".to_string();
        let mut pending = HashMap::new();
        pending.insert("specific".into(), vec!["once".into(), "deny".into()]);
        active().lock().unwrap().insert(
            id.clone(),
            Run {
                url: "http://127.0.0.1:1".into(),
                key: None,
                pending,
            },
        );
        assert!(
            respond_computer_approval(id.clone(), "specific".into(), "always".into())
                .await
                .is_err()
        );
        assert!(
            respond_computer_approval(id.clone(), "stale".into(), "once".into())
                .await
                .is_err()
        );
        active().lock().unwrap().remove(&id);
    }
}
