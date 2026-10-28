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
    // Only exact pending requests owned by this PA process can be answered, and only with a
    // choice Hermes offered for that request. Claim it before sending so it cannot be reused.
    let (run, choices) = {
        let mut runs = active().lock().unwrap();
        let run = runs.get_mut(&run_id).ok_or("该电脑任务已经结束。")?;
        let choices = run
            .pending
            .get(&request_id)
            .ok_or("该操作已经处理或已过期。")?;
        if !choices.contains(&choice) {
            return Err("该操作不允许此批准方式。".into());
        }
        let choices = run.pending.remove(&request_id).unwrap_or_default();
        (run.clone(), choices)
    };
    if let Err(error) = control(
        &run,
        &run_id,
        "approval",
        json!({"request_id": request_id, "choice": choice}),
    )
    .await
    {
        if let Some(run) = active().lock().unwrap().get_mut(&run_id) {
            run.pending.insert(request_id, choices);
        }
        return Err(error);
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
                    "tool.started" => {
                        let name = value["tool"].as_str().unwrap_or("computer_use").to_string();
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
                        if let Some(output) = completion_reply(&value, received_text)? {
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
/// Hermes' own reply is relayed even for a partial run; only a run with no reply at all fails.
/// Returns the output still to be spoken, if it was not already streamed.
fn completion_reply(value: &Value, received_text: bool) -> Result<Option<String>, String> {
    if received_text {
        return Ok(None);
    }
    if let Some(output) = value["output"].as_str().filter(|s| !s.is_empty()) {
        return Ok(Some(output.into()));
    }
    if value["completed"] == false || value["partial"] == true {
        Err("Hermes 任务只完成了一部分，请检查实际结果。".into())
    } else {
        Err("Hermes 任务结束，但未提供可核实的回复。".into())
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
    #[test]
    fn rejects_run_ids_that_can_escape_control_path() {
        assert!(valid_run_id("run_123abc"));
        assert!(!valid_run_id("run_../../stop"));
        assert!(!valid_run_id("other"));
    }
    #[test]
    fn partial_runs_still_relay_hermes_reply() {
        let partial = json!({"event": "run.completed", "partial": true, "completed": false});
        assert_eq!(completion_reply(&partial, true), Ok(None));
        let mut with_output = partial.clone();
        with_output["output"] = json!("只打开了浏览器");
        assert_eq!(completion_reply(&with_output, false), Ok(Some("只打开了浏览器".into())));
        assert!(completion_reply(&partial, false).unwrap_err().contains("只完成了一部分"));
        assert!(completion_reply(&json!({}), false).unwrap_err().contains("未提供"));
    }
    #[tokio::test]
    async fn cannot_approve_unowned_run() {
        assert!(
            respond_computer_approval("run_unknown".into(), "request".into(), "once".into())
                .await
                .is_err()
        );
    }
    fn register(id: &str, url: String, request: &str, choices: &[&str]) {
        let mut pending = HashMap::new();
        pending.insert(request.into(), choices.iter().map(|c| c.to_string()).collect());
        active().lock().unwrap().insert(id.into(), Run { url, key: None, pending });
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
    async fn cannot_widen_or_reuse_an_approval() {
        let id = "run_approval_test";
        register(id, "http://127.0.0.1:1".into(), "specific", &["once", "deny"]);
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
        assert!(active().lock().unwrap()[id].pending.contains_key("specific"));
        active().lock().unwrap().remove(id);
    }

    #[tokio::test]
    async fn relays_offered_session_choice_once() {
        let id = "run_session_test";
        register(id, ok_gateway().await, "request", &["once", "session", "always", "deny"]);
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
}
