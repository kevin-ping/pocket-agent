use axum::{
    Json, Router,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use crate::commands::chat::is_speaking;
use tower_http::cors::CorsLayer;

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

#[derive(Serialize, Clone)]
pub struct PushResponse {
    pub ok: bool,
    pub message: String,
}

#[derive(Serialize, Clone)]
pub struct ApiPayload {
    pub text: String,
    pub emotion: String,
    pub voice: Option<String>,
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

    // Reject if already speaking — only one message at a time
    if is_speaking() {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(PushResponse { ok: false, message: "busy speaking".into() }),
        );
    }

    let emotion = body.emotion.as_ref()
        .map(|e| e.as_str())
        .unwrap_or("friendly")
        .to_string();

    eprintln!("[API] push: {} chars, emotion={}", text.len(), emotion);

    let _ = state.app.emit("api-push", ApiPayload {
        text: text.clone(),
        emotion: emotion.clone(),
        voice: body.voice.clone(),
    });

    (StatusCode::OK, Json(PushResponse { ok: true, message: "pushed".into() }))
}

pub async fn start_server(app: AppHandle, port: u16) {
    // Read API key from .env (same key as Hermes gateway)
    let api_key = std::env::var("API_SERVER_KEY").unwrap_or_default();

    let state = Arc::new(ServerState { app, api_key });

    let router = Router::new()
        .route("/health", get(health))
        .route("/push", post(push_handler))
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
