use eventsource_stream::Eventsource;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use reqwest::Client;
use serde_json::{json, Value};

use crate::commands::computer::DesktopRoute;

pub struct HermesClient {
    client: Client,
    base_url: String,
    api_key: Option<String>,
    api_agent: Option<String>,
}

/// A single chunk from the SSE stream — carries content, reasoning, or tool call events.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// Regular content text delta
    Content(String),
    /// LLM reasoning/thinking text (e.g. DeepSeek R1 thinking process)
    Reasoning(String),
    /// Start of a new tool call
    ToolCallStart { id: String, name: String },
}

impl HermesClient {
    pub fn new(base_url: &str, api_key: Option<String>, api_agent: Option<String>) -> Self {
        let client = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            api_agent,
        }
    }

    /// Ask the model for a tool-free semantic routing decision before an API-server UI turn.
    /// An exact token is required so malformed or expansive output fails closed.
    pub async fn classify_desktop_route(&self, text: &str) -> Result<DesktopRoute, String> {
        const ROUTER_PROMPT: &str = r#"Classify the user's current request by meaning. The current explicit instruction wins over earlier context.
VISIBLE: they ask to open, inspect, or operate their visible local browser/app; also use this for searches or current external information unless they explicitly request background/results-only behavior.
BACKGROUND: they explicitly want only the result, background research, or no browser/app interaction.
CONVERSATION: ordinary conversation, brainstorming, or a knowledge explanation that needs no current retrieval or app action.
Do not call any tool. Output exactly one token: VISIBLE, BACKGROUND, or CONVERSATION."#;
        let model = if let Some(ref agent) = self.api_agent {
            format!("openclaw/{}", agent)
        } else {
            "default".to_string()
        };
        let body = json!({
            "model": model,
            "messages": [
                {"role": "system", "content": ROUTER_PROMPT},
                {"role": "user", "content": text}
            ],
            "stream": false,
            "tools": [],
            "tool_choice": "none"
        });
        let mut request = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .timeout(std::time::Duration::from_secs(20))
            .json(&body);
        if let Some(ref key) = self.api_key {
            request = request.header("Authorization", format!("Bearer {}", key));
        }
        let response = request
            .send()
            .await
            .map_err(|_| "Hermes 语义路由请求失败。".to_string())?;
        if !response.status().is_success() {
            return Err(format!(
                "Hermes 语义路由失败（HTTP {}）。",
                response.status()
            ));
        }
        let value: Value = response
            .json()
            .await
            .map_err(|_| "Hermes 语义路由返回格式无效。".to_string())?;
        let content = value["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default();
        Ok(DesktopRoute::parse_classifier_output(content))
    }

    /// Initiate a streaming chat request, returning a stream of events
    /// (content, reasoning, tool_calls).
    pub async fn chat_stream(
        &self,
        text: &str,
        voice_hint: Option<&str>,
        context: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<BoxStream<'static, Result<StreamEvent, String>>, String> {
        let mut messages = vec![];
        if let Some(ctx) = context {
            if !ctx.is_empty() {
                messages.push(json!({ "role": "system", "content": ctx }));
            }
        }
        if let Some(hint) = voice_hint {
            if !hint.is_empty() {
                messages.push(json!({ "role": "system", "content": hint }));
            }
        }
        messages.push(json!({ "role": "user", "content": text }));

        let model = if let Some(ref agent) = self.api_agent {
            format!("openclaw/{}", agent)
        } else {
            "default".to_string()
        };
        let body = json!({
            "model": model,
            "messages": messages,
            "stream": true
        });

        let mut request_builder = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .json(&body);

        if let Some(ref key) = self.api_key {
            request_builder = request_builder.header("Authorization", format!("Bearer {}", key));
        }

        if let Some(sid) = session_id {
            // Always send Hermes session ID for session continuity
            request_builder = request_builder.header("X-Hermes-Session-Id", sid);
            // Also send OpenClaw format if agent is configured (for compatibility)
            if let Some(ref agent) = self.api_agent {
                request_builder = request_builder.header("x-openclaw-session-key", format!("agent:{}:{}", agent, sid));
            }
        }

        let response = request_builder
            .send()
            .await
            .map_err(|e| format!("Connection failed: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let resp_body = response.text().await.unwrap_or_default();
            return Err(format!("API returned error {}: {}", status, resp_body));
        }

        let error_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let max_errors: u32 = 5;

        let stream = response
            .bytes_stream()
            .eventsource()
            .filter_map(move |event| {
                let error_count = error_count.clone();
                async move {
                    match event {
                        Ok(e) if e.data == "[DONE]" => None,
                        // OpenClaw Hermes custom SSE events for tool progress
                        Ok(e) if e.event == "hermes.tool.progress" => {
                            error_count.store(0, std::sync::atomic::Ordering::SeqCst);
                            if let Ok(v) = serde_json::from_str::<Value>(&e.data) {
                                let status = v["status"].as_str().unwrap_or("");
                                if status == "running" {
                                    if let (Some(id), Some(name)) = (
                                        v["toolCallId"].as_str().filter(|s| !s.is_empty()),
                                        v["tool"].as_str().filter(|s| !s.is_empty()),
                                    ) {
                                        return Some(Ok(StreamEvent::ToolCallStart {
                                            id: id.to_string(),
                                            name: name.to_string(),
                                        }));
                                    }
                                }
                                // Skip "completed" status events silently
                            }
                            None
                        }
                        Ok(e) => {
                            error_count.store(0, std::sync::atomic::Ordering::SeqCst);
                            let v: Value = match serde_json::from_str(&e.data) {
                                Ok(v) => v,
                                Err(_) => return None,
                            };
                            let choices = &v["choices"];
                            if !choices.is_array() || choices.as_array().unwrap().is_empty() {
                                return None;
                            }
                            let delta = &choices[0]["delta"];

                            // 1. Reasoning / thinking content (DeepSeek R1 style)
                            let reasoning = delta["reasoning"]
                                .as_str()
                                .or_else(|| delta["reasoning_content"].as_str())
                                .map(|s| s.to_string())
                                .filter(|s| !s.is_empty());
                            if let Some(text) = reasoning {
                                return Some(Ok(StreamEvent::Reasoning(text)));
                            }

                            // 2. Tool calls via OpenAI format — fallback for non-OpenClaw providers
                            if let Some(tool_calls) = delta["tool_calls"].as_array() {
                                for tc in tool_calls {
                                    if let (Some(id), Some(name)) = (
                                        tc["id"].as_str().filter(|s| !s.is_empty()),
                                        tc["function"]["name"].as_str().filter(|s| !s.is_empty()),
                                    ) {
                                        return Some(Ok(StreamEvent::ToolCallStart {
                                            id: id.to_string(),
                                            name: name.to_string(),
                                        }));
                                    }
                                }
                            }

                            // 3. Regular content
                            let content = delta["content"]
                                .as_str()
                                .unwrap_or("")
                                .to_string();
                            if !content.is_empty() {
                                return Some(Ok(StreamEvent::Content(content)));
                            }

                            None
                        }
                        Err(e) => {
                            let count = error_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                            eprintln!("[SSE] transient error ({}/{}): {}", count, max_errors, e);
                            if count >= max_errors {
                                eprintln!("[SSE] too many errors, terminating stream");
                                Some(Err(format!("SSE stream unstable, auto-disconnected ({} errors)", max_errors)))
                            } else {
                                None
                            }
                        }
                    }
                }
            })
            .boxed();

        eprintln!("[SSE] model={} url={}/v1/chat/completions", model, self.base_url);
        Ok(stream)
    }
}
