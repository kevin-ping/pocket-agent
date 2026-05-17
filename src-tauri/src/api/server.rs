use axum::{
    Json, Router,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tower_http::cors::CorsLayer;

use crate::commands::chat::{build_bridge_session_key, dispatch_bridge_message, is_queue_full};

pub struct ServerState {
    pub app: AppHandle,
    pub api_key: String,
}

/// Valid emotions for push messages
#[derive(Deserialize, Clone)]
#[serde(rename_all = "lowercase")]
pub enum Emotion {
    Friendly,
    Cheerful,
    Calm,
    Serious,
    Sad,
    Whisper,
    Excited,
    Angry,
}

impl Emotion {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Friendly => "friendly",
            Self::Cheerful => "cheerful",
            Self::Calm => "calm",
            Self::Serious => "serious",
            Self::Sad => "sad",
            Self::Whisper => "whisper",
            Self::Excited => "excited",
            Self::Angry => "angry",
        }
    }
}

#[derive(Deserialize)]
pub struct PushRequest {
    /// Text to speak and display (required)
    pub text: String,
    /// Emotion tone for TTS display speed (optional, default: friendly)
    #[serde(default)]
    pub emotion: Option<Emotion>,
    /// Override TTS voice, e.g. "zh-CN-XiaoxiaoNeural" (optional)
    #[serde(default)]
    pub voice: Option<String>,
}

#[derive(Deserialize, Clone)]
pub struct BridgeSendRequest {
    pub source: String,
    pub session_id: String,
    pub text: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub user_language: Option<String>,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default = "default_show_thinking")]
    pub show_thinking: bool,
}

#[derive(Serialize, Clone)]
pub struct PushResponse {
    pub ok: bool,
    pub message: String,
}

#[derive(Serialize, Clone)]
pub struct BridgeSendResponse {
    pub ok: bool,
    pub accepted: bool,
    pub source: String,
    pub session_id: String,
    pub bridge_session: String,
    pub message: String,
}

#[derive(Serialize, Clone)]
pub struct ApiErrorResponse {
    pub ok: bool,
    pub error: String,
}

#[derive(Serialize, Clone)]
pub struct ApiPayload {
    pub text: String,
    pub emotion: String,
    pub voice: Option<String>,
}

fn default_show_thinking() -> bool {
    true
}

fn validate_bridge_request(body: &BridgeSendRequest) -> Result<(), String> {
    if body.source.trim().is_empty() {
        return Err("source is empty".into());
    }
    if body.session_id.trim().is_empty() {
        return Err("session_id is empty".into());
    }
    if body.text.trim().is_empty() {
        return Err("text is empty".into());
    }
    Ok(())
}

fn bridge_ack_response(source: &str, session_id: &str) -> BridgeSendResponse {
    BridgeSendResponse {
        ok: true,
        accepted: true,
        source: source.to_string(),
        session_id: session_id.to_string(),
        bridge_session: build_bridge_session_key(source, session_id),
        message: "accepted for Hermes dispatch".into(),
    }
}

async fn health() -> &'static str {
    "ok"
}

/// Bearer token auth middleware
async fn auth_middleware(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let expected = &state.api_key;
    if expected.is_empty() {
        return Ok(next.run(request).await); // no key configured = open
    }

    let provided = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match provided {
        Some(token) if token == expected => Ok(next.run(request).await),
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

async fn push_handler(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<PushRequest>,
) -> (StatusCode, Json<PushResponse>) {
    let text = body.text.trim().to_string();
    if text.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(PushResponse { ok: false, message: "text is empty".into() }),
        );
    }

    // Reject if audio queue full (max 3 concurrent)
    if is_queue_full() {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(PushResponse { ok: false, message: "audio queue full".into() }),
        );
    }

    let emotion = body.emotion.as_ref()
        .map(|e| e.as_str())
        .unwrap_or("friendly")
        .to_string();

    eprintln!("[API] push: {} chars, emotion={}", text.len(), emotion);

    let _ = state.app.emit("bridge-push-received", ());
    let _ = state.app.emit("api-push", ApiPayload {
        text: text.clone(),
        emotion: emotion.clone(),
        voice: body.voice.clone(),
    });

    (StatusCode::OK, Json(PushResponse { ok: true, message: "pushed".into() }))
}

async fn bridge_send_handler(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<BridgeSendRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if let Err(error) = validate_bridge_request(&body) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!(ApiErrorResponse { ok: false, error })),
        );
    }

    let source = body.source.trim().to_string();
    let session_id = body.session_id.trim().to_string();
    let text = body.text.trim().to_string();
    let context = body.context.as_ref().map(|value| value.trim().to_string()).filter(|value| !value.is_empty());
    let show_thinking = body.show_thinking;
    let app = state.app.clone();
    let ack = bridge_ack_response(&source, &session_id);

    tauri::async_runtime::spawn(async move {
        if let Err(error) = dispatch_bridge_message(app, source, session_id, text, context, show_thinking).await {
            eprintln!("[API] bridge dispatch failed: {}", error);
        }
    });

    (StatusCode::ACCEPTED, Json(json!(ack)))
}

pub async fn start_server(app: AppHandle, port: u16) {
    // Read API key from .env (same key as Hermes gateway)
    let api_key = std::env::var("API_SERVER_KEY").unwrap_or_default();

    let state = Arc::new(ServerState { app, api_key });

    let router = Router::new()
        .route("/health", get(health))
        .route("/push", post(push_handler))
        .route("/bridge/send", post(bridge_send_handler))
        .layer(middleware::from_fn_with_state(state.clone(), auth_middleware))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    eprintln!("[API] listening on http://{}", addr);

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[API] bind failed: {} (port {} in use?)", e, port);
            return;
        }
    };

    if let Err(e) = axum::serve(listener, router).await {
        eprintln!("[API] server error: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_request_validation_rejects_blank_fields() {
        let request = BridgeSendRequest {
            source: "   ".into(),
            session_id: "game-001".into(),
            text: "move now".into(),
            user_language: None,
            context: None,
            show_thinking: true,
        };
        assert_eq!(validate_bridge_request(&request), Err("source is empty".into()));
    }

    #[test]
    fn bridge_ack_uses_accepted_wording_and_session_namespace() {
        let ack = bridge_ack_response("chess-app", "game-001");
        assert!(ack.ok);
        assert!(ack.accepted);
        assert_eq!(ack.bridge_session, "bridge:chess-app:game-001");
        assert_eq!(ack.message, "accepted for Hermes dispatch");
    }
}
