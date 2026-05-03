use eventsource_stream::Eventsource;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use reqwest::Client;
use serde_json::{json, Value};

pub struct HermesClient {
    client: Client,
    base_url: String,
    api_key: Option<String>,
}

impl HermesClient {
    pub fn new(base_url: &str, api_key: Option<String>) -> Self {
        let client = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
        }
    }

    /// 发起流式对话请求，返回 delta 文字流
    pub async fn chat_stream(
        &self,
        text: &str,
        voice_hint: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<BoxStream<'static, Result<String, String>>, String> {
        // Prepend voice output instruction directly to user message text
        // This is the most reliable way — it can't be overridden by other system prompts
        let full_text = match voice_hint {
            Some(hint) => {
                format!(
                    "{}\n\n---\nUser's message: {}",
                    hint, text
                )
            }
            None => text.to_string(),
        };

        let messages = vec![
            json!({ "role": "user", "content": full_text }),
        ];

        let body = json!({
            "model": "default",
            "messages": messages,
            "stream": true
        });

        let mut request_builder = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .json(&body);

        // Bearer auth (required by Hermes api_server when API_SERVER_KEY is configured)
        if let Some(ref key) = self.api_key {
            request_builder = request_builder.header("Authorization", format!("Bearer {}", key));
        }

        // Session continuity
        if let Some(sid) = session_id {
            request_builder = request_builder.header("X-Hermes-Session-Id", sid);
        }

        let response = request_builder
            .send()
            .await
            .map_err(|e| format!("连接 Hermes 失败: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let resp_body = response.text().await.unwrap_or_default();
            return Err(format!("Hermes 返回错误 {}: {}", status, resp_body));
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
                        Ok(e) => {
                            error_count.store(0, std::sync::atomic::Ordering::SeqCst);
                            let v: Value = match serde_json::from_str(&e.data) {
                                Ok(v) => v,
                                Err(_) => return None,
                            };
                            let delta = v["choices"][0]["delta"]["content"]
                                .as_str()
                                .unwrap_or("")
                                .to_string();
                            if delta.is_empty() {
                                None
                            } else {
                                Some(Ok(delta))
                            }
                        }
                        Err(e) => {
                            let count = error_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                            eprintln!("[SSE] transient error ({}/{}): {}", count, max_errors, e);
                            if count >= max_errors {
                                eprintln!("[SSE] too many errors, terminating stream");
                                Some(Err(format!("SSE 连接不稳定，已自动断开 ({} errors)", max_errors)))
                            } else {
                                None
                            }
                        }
                    }
                }
            })
            .boxed();

        Ok(stream)
    }
}
