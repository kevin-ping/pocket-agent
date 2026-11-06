// remote.rs — The phone terminal's endpoints on 8650 (FEAT-DEV1 / FEAT-DEV2 / SEC-DEV1)
//
//   GET /ui/, /assets/…  the main-window frontend, served from the app's own assets
//   WS  /ui/ws           main-UI events out, whitelisted commands in
//   WS  /audio           phone mic PCM in, sentence audio + control out
//
// Approving a computer action from /ui controls this Mac, so both sockets
// require API_SERVER_KEY or a paired phone's device token (api/pairing.rs):
// an unset key rejects (unlike /push, which is open then). Browsers and
// WebViews cannot set headers on a WebSocket, so the token rides in the
// `token` query parameter; the page reads it from its own URL. Removing a
// paired phone closes the sockets it opened.
//
// One driver at a time: a new /ui (or /audio) connection replaces the old one
// so a phone reconnecting after Wi-Fi loss never waits on a dead socket. While
// a /ui client drives, the desktop main window is hidden and its frontend
// stays an empty shell (src/main.ts), so a turn is never driven twice.
//
// Parked (FEAT-DEV1): after "back to the desktop" the phone stays connected
// but drives nothing — the desktop is the driver and the audio source, main-UI
// events stop flowing to the phone, and its page shows only a "take over"
// button. Reconnecting does not unpark; switch_device or take_over does.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use axum::{
    extract::{
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::{header, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use futures_util::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Listener, Manager};
use tokio::sync::{mpsc, oneshot};

use super::pairing::Revoked;
use super::server::ServerState;
use crate::voice::remote::{is_parked, phone_attached, set_parked, Outbound};

/// Events the main window listens to, forwarded to the /ui client. Desktop-only
/// ones (tray menu, macOS accessibility prompt) stay on the Mac.
pub const UI_EVENTS: &[&str] = &[
    "api-push", "app-ready", "audio-source-changed", "background-subagents",
    "bridge-push-received", "bridge-thinking", "bridge-thinking-start", "bridge-tool-call",
    "bridge-turn-error", "bridge-turn-finished", "chat-audio-done", "chat-audio-idle",
    "chat-audio-playing", "chat-audio-sentence-end", "chat-compacting", "chat-speaking-start",
    "chat-status-audio-playing", "chat-stream", "chat-stream-end", "chat-stream-error",
    "chat-thinking", "chat-thinking-start", "chat-tool-call", "computer-approval",
    "computer-approval-clear", "computer-control-error", "conv-speech-detected",
    "conversation-barge-in", "conversation-ended", "conversation-ready-prompt",
    "conversation-state", "fn-key-down", "fn-key-up", "settings-changed", "stt-error",
    "stt-result", "subagent-delivered", "venv-setup-done", "venv-setup-error",
    "venv-setup-progress", "venv-setup-ready", "venv-setup-started", "voice-cancel",
    "voice-service-status", "wake-check-done", "wake-checking", "wake-listener-error",
    "wake-listener-started", "wake-listener-stopped", "wake-word-detected",
];

/// Commands the main window uses. Settings window, quit, window position and
/// chat-history window stay desktop-only.
pub const UI_COMMANDS: &[&str] = &[
    "acknowledge_conversation_ready", "cancel_voice_recording", "discard_pending_turn",
    "get_audio_level", "get_computer_status", "get_config", "get_setting_asset", "is_app_ready",
    "notify_conversation_tts_done", "notify_conversation_tts_started", "pending_computer_approvals",
    "reset_hotkey_active_state", "respond_computer_approval", "save_chat_message", "send_message", "set_double_click_mode",
    "settle_background_subagents", "speak_status", "speak_text", "start_continuous_conversation",
    "start_wake_word_listening", "stop_continuous_conversation", "stop_wake_word_listening",
    "voice_setup_status", "wake_model_status", "is_ui_parked", "set_ui_visible", "take_over", "park",
];

/// All a parked phone may do: ask whether it is parked, report whether it is
/// on screen, and take over.
const PARKED_COMMANDS: &[&str] = &["is_ui_parked", "set_ui_visible", "take_over"];

/// Close code telling a replaced /ui page not to reconnect (it would kick the
/// newer client right back).
const CLOSE_REPLACED: u16 = 4001;
/// No frame (PCM, report, pong) for this long → the peer is gone.
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const PING_EVERY: Duration = Duration::from_secs(10);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

// ── Auth ─────────────────────────────────────────────────────────────────────

/// Unset key → reject. Equal-length comparison runs over every byte.
pub(crate) fn token_ok(expected: &str, provided: Option<&str>) -> bool {
    let Some(provided) = provided else { return false };
    if expected.is_empty() || expected.len() != provided.len() {
        return false;
    }
    expected
        .bytes()
        .zip(provided.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

#[derive(Deserialize)]
pub struct TokenQuery {
    token: Option<String>,
}

impl TokenQuery {
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }
}

// ── Static page ──────────────────────────────────────────────────────────────

/// Fallback for every unrouted path: the bundled frontend. No secrets live in
/// it (the key comes from the page URL), so it is served without a token.
pub async fn static_asset(State(state): State<Arc<ServerState>>, method: Method, uri: Uri) -> Response {
    if method != Method::GET {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(path) = asset_path(uri.path()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match state.app.asset_resolver().get(path) {
        Some(asset) => (
            [
                (header::CONTENT_TYPE, asset.mime_type.clone()),
                (header::CACHE_CONTROL, "no-cache".to_string()),
            ],
            asset.bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `/ui/x` and `/x` both name bundle file `/x`. Dev builds read the bundle
/// from disk, so parent-directory segments are refused outright.
fn asset_path(path: &str) -> Option<String> {
    if path.split('/').any(|segment| segment == "..") {
        return None;
    }
    let rest = if path == "/ui" { "" } else if path.starts_with("/ui/") { &path[3..] } else { path };
    Some(if rest.is_empty() || rest == "/" { "/index.html".into() } else { rest.into() })
}

// ── /ui ──────────────────────────────────────────────────────────────────────

struct UiClient {
    id: u64,
    tx: mpsc::UnboundedSender<Message>,
    /// The page is on screen (it reports visibilitychange). An Android WebView
    /// in the background keeps its socket and answers pings, so being attached
    /// says nothing about the foreground. Unknown until reported → false.
    visible: bool,
    /// Dropped when a newer client takes over; the old session then closes.
    _kick: oneshot::Sender<()>,
}

static UI_CLIENT: Mutex<Option<UiClient>> = Mutex::new(None);
static NEXT_UI_ID: AtomicU64 = AtomicU64::new(0);
static UI_REMOTE_ACTIVE: AtomicBool = AtomicBool::new(false);
/// A /ui page has connected at some point in this run (the device hint, commands/chat.rs).
static PHONE_SEEN: AtomicBool = AtomicBool::new(false);
/// Set once at startup (forward_ui_events); a deferred switch runs without a request in hand.
static APP: OnceLock<AppHandle> = OnceLock::new();

/// The desktop frontend asks this before mounting (see src/main.ts).
#[tauri::command]
pub fn is_ui_remote_active() -> bool {
    UI_REMOTE_ACTIVE.load(Ordering::Acquire)
}

/// voice/remote tests: the phone driving without a /ui session.
#[cfg(test)]
pub(crate) fn set_ui_remote_active(active: bool) {
    UI_REMOTE_ACTIVE.store(active, Ordering::Release);
}

/// Forward the main window's events to the /ui client. Registered once at startup.
pub fn forward_ui_events(app: &AppHandle) {
    let _ = APP.set(app.clone());
    for &name in UI_EVENTS {
        app.listen_any(name, move |event| {
            if is_parked() {
                return; // the desktop drives; the parked phone sees nothing of the conversation
            }
            if let Some(client) = lock(&UI_CLIENT).as_ref() {
                let text = format!(
                    r#"{{"type":"event","event":"{}","payload":{}}}"#,
                    name,
                    if event.payload().is_empty() { "null" } else { event.payload() }
                );
                let _ = client.tx.send(Message::Text(text.into()));
            }
        });
    }
}

/// An event for the /ui page alone (phone-camera, ui-parked): the desktop
/// window never sees it. Returns whether a client was there to receive it.
pub fn send_to_ui(event: &str, payload: Value) -> bool {
    let text = json!({ "type": "event", "event": event, "payload": payload }).to_string();
    lock(&UI_CLIENT)
        .as_ref()
        .map(|client| client.tx.send(Message::Text(text.into())).is_ok())
        .unwrap_or(false)
}

pub(crate) fn ui_attached() -> bool {
    lock(&UI_CLIENT).is_some()
}

pub(crate) fn phone_seen() -> bool {
    PHONE_SEEN.load(Ordering::Acquire)
}

fn ui_in_foreground() -> bool {
    lock(&UI_CLIENT).as_ref().is_some_and(|c| c.visible)
}

/// set_ui_visible: only the sending client's own report counts.
fn set_ui_visible(client: u64, visible: bool) -> bool {
    if let Some(c) = lock(&UI_CLIENT).as_mut().filter(|c| c.id == client) {
        c.visible = visible;
    }
    visible
}

pub async fn ui_ws(
    State(state): State<Arc<ServerState>>,
    Query(query): Query<TokenQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let Some((_, revoked)) = super::pairing::authenticate(&state.api_key, query.token()) else {
        eprintln!("[remote] /ui rejected: bad or missing token");
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let app = state.app.clone();
    ws.on_upgrade(move |socket| ui_session(app, socket, revoked))
}

async fn ui_session(app: AppHandle, socket: WebSocket, mut revoked: Revoked) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    let (kick, mut kicked) = oneshot::channel::<()>();
    let id = NEXT_UI_ID.fetch_add(1, Ordering::AcqRel) + 1;
    *lock(&UI_CLIENT) = Some(UiClient { id, tx: tx.clone(), visible: false, _kick: kick });
    PHONE_SEEN.store(true, Ordering::Release);
    eprintln!("[remote] /ui client {} connected", id);
    sync_driver(&app);

    let mut writer = tokio::spawn(async move {
        let mut ping = tokio::time::interval(PING_EVERY);
        loop {
            tokio::select! {
                msg = rx.recv() => match msg {
                    Some(msg) => {
                        let closing = matches!(msg, Message::Close(_));
                        if sink.send(msg).await.is_err() || closing { break }
                    }
                    None => break,
                },
                _ = ping.tick() => if sink.send(Message::Ping(Default::default())).await.is_err() { break },
            }
        }
    });

    loop {
        tokio::select! {
            _ = &mut kicked => {
                let frame = CloseFrame { code: CLOSE_REPLACED, reason: "replaced".into() };
                let _ = tx.send(Message::Close(Some(frame)));
                let _ = tokio::time::timeout(Duration::from_secs(1), &mut writer).await;
                break;
            }
            _ = revoked.wait() => {
                eprintln!("[remote] /ui client {} closed: its phone was removed", id);
                break;
            }
            _ = &mut writer => break,
            frame = tokio::time::timeout(IDLE_TIMEOUT, stream.next()) => match frame {
                Ok(Some(Ok(Message::Text(text)))) => {
                    if let Some(reply) = report_visibility(id, text.as_str()) {
                        let _ = tx.send(Message::Text(reply.into()));
                    } else {
                        let (app, tx) = (app.clone(), tx.clone());
                        tokio::spawn(async move {
                            let reply = handle_command(&app, id, text.as_str()).await;
                            let _ = tx.send(Message::Text(reply.into()));
                        });
                    }
                }
                Ok(Some(Ok(Message::Close(_)))) | Ok(Some(Err(_))) | Ok(None) | Err(_) => break,
                Ok(Some(Ok(_))) => {}
            },
        }
    }
    writer.abort();

    {
        let mut slot = lock(&UI_CLIENT);
        if slot.as_ref().map(|c| c.id == id).unwrap_or(false) {
            *slot = None;
        }
    }
    eprintln!("[remote] /ui client {} disconnected", id);
    sync_driver(&app);
}

/// The /ui client currently driving (None = the desktop window). Only
/// converge_driver writes it.
static DRIVER: Mutex<Option<u64>> = Mutex::new(None);

/// Converge the driver on the attached /ui client: hand the main UI to it (or
/// back to the desktop). Reading the target here, under the DRIVER lock, keeps
/// an old session's disconnect racing a new session's connect from landing
/// last. A phone replacing another phone is a handover too: the new page
/// starts with no conversation state, so the old one's must end.
/// Calls `apply(was_remote, remote)` and returns the new driver when it changed.
fn converge_driver(apply: impl FnOnce(bool, bool)) -> Option<Option<u64>> {
    let mut driver = lock(&DRIVER);
    // A parked phone may stay attached; the desktop drives all the same.
    let target = if is_parked() { None } else { lock(&UI_CLIENT).as_ref().map(|c| c.id) };
    if *driver == target {
        return None;
    }
    let was_remote = driver.is_some();
    *driver = target;
    UI_REMOTE_ACTIVE.store(target.is_some(), Ordering::Release);
    apply(was_remote, target.is_some());
    Some(target)
}

fn sync_driver(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        converge_driver(|was_remote, remote| {
            // The switch interrupts whatever is playing or listening so the new driver starts clean.
            crate::commands::chat::stop_audio_queue("driver switch");
            crate::voice::conversation::stop_conversation();
            if was_remote == remote {
                return; // phone → phone: the window and the desktop shell stay as they are
            }
            if let Some(window) = app.get_webview_window("main") {
                if remote {
                    let _ = window.hide();
                } else if crate::voice::stt::is_app_ready() {
                    let _ = window.show();
                }
            }
            // Desktop-only event: the main window reloads into (or out of) its idle shell.
            let _ = app.emit("ui-remote-active", remote);
        });
        // The audio source follows the driver; converging after the driver
        // (whether it changed or not) settles both whatever order /ui and /audio arrive in.
        crate::voice::remote::sync_source(&app);
    });
}

#[derive(Deserialize)]
struct CommandRequest {
    id: u64,
    cmd: String,
    #[serde(default)]
    args: Value,
}

/// set_ui_visible, handled in the session loop: reports must land in the
/// order the page sent them (hidden at boot, visible a moment later), and
/// spawned commands may finish in any order. None for every other command.
fn report_visibility(client: u64, text: &str) -> Option<String> {
    let request: CommandRequest = serde_json::from_str(text).ok()?;
    let reply = match request.cmd.as_str() {
        "set_ui_visible" => arg::<bool>(&request.args, "visible").map(|v| set_ui_visible(client, v)),
        _ => return None,
    };
    Some(match reply {
        Ok(visible) => json!({ "type": "result", "id": request.id, "ok": visible }),
        Err(e) => json!({ "type": "result", "id": request.id, "err": e }),
    }.to_string())
}

/// `client`: the /ui connection the command came in on. Commands run spawned,
/// so one may still run after a newer page has replaced its sender.
async fn handle_command(app: &AppHandle, client: u64, text: &str) -> String {
    let request: CommandRequest = match serde_json::from_str(text) {
        Ok(r) => r,
        Err(e) => return json!({ "type": "result", "id": null, "err": format!("bad request: {e}") }).to_string(),
    };
    let reply = match dispatch(app, client, &request.cmd, &request.args).await {
        Ok(value) => json!({ "type": "result", "id": request.id, "ok": value }),
        Err(e) => json!({ "type": "result", "id": request.id, "err": e }),
    };
    reply.to_string()
}

/// One named argument, in the camelCase the frontend's invoke uses. Absent
/// and null both read as None for optional arguments.
fn arg<T: DeserializeOwned>(args: &Value, name: &str) -> Result<T, String> {
    serde_json::from_value(args.get(name).cloned().unwrap_or(Value::Null))
        .map_err(|e| format!("argument {name}: {e}"))
}

fn ok<T: serde::Serialize>(value: T) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|e| e.to_string())
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())
}

fn command_allowed(cmd: &str, parked: bool) -> bool {
    UI_COMMANDS.contains(&cmd) && (!parked || PARKED_COMMANDS.contains(&cmd))
}

/// Calls the same functions the Tauri commands are. Anything not in
/// UI_COMMANDS (or, while parked, PARKED_COMMANDS) is refused before an
/// argument is read.
async fn dispatch(app: &AppHandle, client: u64, cmd: &str, a: &Value) -> Result<Value, String> {
    use crate::commands::{chat, computer, config, history, voice};
    if !command_allowed(cmd, is_parked()) {
        eprintln!("[remote] /ui command refused: {}", cmd);
        return Err(format!("command not allowed: {cmd}"));
    }
    match cmd {
        "is_ui_parked" => ok(is_parked()),
        "take_over" => ok(take_over(app, client)?),
        "park" => ok(park(app, client)?),
        "send_message" => ok(chat::send_message(
            app.clone(), app.state(), arg(a, "text")?, arg(a, "ttsFormat")?, arg(a, "ttsPrimaryVoice")?,
            arg(a, "ttsAux1Voice")?, arg(a, "ttsAux2Voice")?, arg(a, "userLanguage")?, arg(a, "fixedLang")?,
            arg(a, "ttsEnabled")?,
        ).await?),
        "speak_status" => ok(chat::speak_status(
            app.clone(), arg(a, "text")?, arg(a, "overrideVoice")?, arg(a, "ttsFormat")?,
            arg(a, "ttsPrimaryVoice")?, arg(a, "ttsAux1Voice")?, arg(a, "ttsAux2Voice")?, arg(a, "ttsEnabled")?,
        ).await?),
        "speak_text" => ok(chat::speak_text(
            app.clone(), arg(a, "text")?, arg(a, "emotion")?, arg(a, "overrideVoice")?, arg(a, "ttsFormat")?,
            arg(a, "ttsPrimaryVoice")?, arg(a, "ttsAux1Voice")?, arg(a, "ttsAux2Voice")?, arg(a, "ttsEnabled")?,
        ).await?),
        "discard_pending_turn" => ok(chat::discard_pending_turn(app.clone()).await?),
        "reset_hotkey_active_state" => ok(chat::reset_hotkey_active_state()),
        "save_chat_message" => {
            let (role, content): (String, String) = (arg(a, "role")?, arg(a, "content")?);
            ok(blocking(move || history::save_chat_message(role, content)).await??)
        }
        "get_config" => ok(config::get_config(app.clone()).await?),
        "get_setting_asset" => ok(config::get_setting_asset(arg(a, "key")?).await?),
        "is_app_ready" => ok(voice::is_app_ready()),
        "voice_setup_status" => ok(crate::voice::setup_status::voice_setup_status()),
        "get_audio_level" => ok(voice::get_audio_level()),
        "notify_conversation_tts_started" => ok(voice::notify_conversation_tts_started()?),
        "notify_conversation_tts_done" => ok(voice::notify_conversation_tts_done(arg(a, "generation")?)?),
        "acknowledge_conversation_ready" => {
            ok(voice::acknowledge_conversation_ready(arg(a, "id")?, arg(a, "played")?, arg(a, "readyDelayMs")?))
        }
        "start_continuous_conversation" => {
            let app = app.clone();
            let (t, p, s, single, b, w, barge) = (
                arg(a, "silenceTimeoutSecs")?, arg(a, "pauseToleranceMs")?, arg(a, "speechRmsThreshold")?,
                arg(a, "singleShot")?, arg(a, "bargeInRmsThreshold")?, arg(a, "wakeWordThreshold")?,
                arg(a, "bargeInEnabled")?,
            );
            ok(blocking(move || voice::start_continuous_conversation(app, t, p, s, single, b, w, barge)).await??)
        }
        "stop_continuous_conversation" => ok(blocking(voice::stop_continuous_conversation).await??),
        "start_wake_word_listening" => ok(voice::start_wake_word_listening(
            app.clone(), arg(a, "threshold")?, arg(a, "speakerName")?,
        ).await?),
        "stop_wake_word_listening" => ok(blocking(voice::stop_wake_word_listening).await??),
        "wake_model_status" => ok(voice::wake_model_status().await?),
        "cancel_voice_recording" => ok(voice::cancel_voice_recording(app.state()).await?),
        "pending_computer_approvals" => ok(crate::api::runs::pending_computer_approvals()),
        "respond_computer_approval" => ok(crate::api::runs::respond_computer_approval(
            arg(a, "runId")?, arg(a, "requestId")?, arg(a, "choice")?,
        ).await?),
        "get_computer_status" => ok(computer::get_computer_status(arg(a, "includePermissions")?).await?),
        "settle_background_subagents" => ok(crate::api::subagents::settle_background_subagents(app.clone())),
        "set_double_click_mode" => ok(crate::voice::hotkey::set_double_click_mode(arg(a, "enabled")?)),
        _ => Err(format!("command not allowed: {cmd}")),
    }
}

// ── Switching devices (FEAT-DEV1) ────────────────────────────────────────────
//
// switch_device (MCP, Hermes decides) only books the switch: it runs once the
// current turn has ended and its reply has played, so the confirmation is
// heard on the device the user spoke to and the desktop frontend never
// reloads mid-stream. A turn that is cancelled or fails ends the wait just the
// same — the user asked for the switch, and it then happens. take_over (the
// parked page's button) is the user's own hand and acts at once.

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Device {
    Desktop,
    Phone,
}

pub const ALREADY_ON_DESKTOP: &str = "已经在电脑上";
pub const ALREADY_ON_PHONE: &str = "已经在手机上";
pub const PHONE_UI_ABSENT: &str = "手机界面没有在前台打开";
pub const PHONE_IS_PARKED: &str = "手机处于待命,PA 现在在电脑上";
pub const SWITCHING_TO_DESKTOP: &str = "将在本轮回复说完后切到电脑";
pub const SWITCHING_TO_PHONE: &str = "将在本轮回复说完后切到手机";

const SWITCH_POLL: Duration = Duration::from_millis(200);
/// Quiet this long (no turn, nothing queued) before switching: a bridge reply
/// reaches the audio queue a moment after its turn returns.
const SWITCH_SETTLE: Duration = Duration::from_secs(1);
/// After the turn ends, its audio gets at most this long to finish.
const SWITCH_DRAIN_MAX: Duration = Duration::from_secs(30);

/// The switch boundary: its value is bumped by every switch_device request
/// with a valid target and by take_over, and only the latest booking may run.
/// Booking, cancelling and committing PARKED all happen under this lock, so a
/// booking that passed its checks cannot land after a take_over. Notifying
/// the page and converging run after it is released.
/// Lock order: SWITCH, then UI_CLIENT / the phone's locks.
static SWITCH: Mutex<u64> = Mutex::new(0);

/// Whether the phone can take a camera command now (phone_camera_open / _close).
pub fn phone_ui_ready() -> Result<(), &'static str> {
    if is_parked() {
        return Err(PHONE_IS_PARKED);
    }
    if !ui_in_foreground() || !phone_attached() {
        return Err(PHONE_UI_ABSENT);
    }
    Ok(())
}

/// The phone's state as switch_device sees it.
#[derive(Clone, Copy)]
struct PhoneState {
    parked: bool,
    /// A /ui page is attached (foreground or not).
    ui: bool,
    /// …and on screen.
    foreground: bool,
    audio: bool,
    /// A paired phone names this PA its occupant (FEAT-MP3), even with its
    /// /ui and /phone down for now: it would come back to this PA.
    occupied: bool,
}

/// Ok(booking reply) when the switch is possible, Err(the fact) when not.
/// Taking over needs the phone page on screen and its audio online.
fn switch_answer(target: Device, p: PhoneState) -> Result<&'static str, &'static str> {
    match target {
        Device::Desktop if p.parked || (!p.ui && !p.occupied) => Err(ALREADY_ON_DESKTOP),
        Device::Desktop => Ok(SWITCHING_TO_DESKTOP),
        Device::Phone if !p.parked && p.ui => Err(ALREADY_ON_PHONE),
        Device::Phone if !p.foreground || !p.audio => Err(PHONE_UI_ABSENT),
        Device::Phone => Ok(SWITCHING_TO_PHONE),
    }
}

fn current_switch_answer(target: Device) -> Result<&'static str, &'static str> {
    let state = PhoneState {
        parked: is_parked(),
        ui: ui_attached(),
        foreground: ui_in_foreground(),
        audio: phone_attached(),
        occupied: super::phone::occupied_by_me(),
    };
    switch_answer(target, state)
}

/// How a booked switch happens once the turn is over.
#[derive(Clone, Debug, PartialEq)]
enum Route {
    /// Flip PARKED for the attached /ui page.
    Park,
    /// No /ui page: ask this paired phone over /phone to come (FEAT-MP3).
    /// Its answer, and whether another PA occupies it, is the phone's call.
    Enter(String),
}

fn switch_plan(target: Device) -> Result<(&'static str, Route), String> {
    if target == Device::Phone && !ui_attached() {
        return super::phone::pick_for_enter()
            .map(|phone| (SWITCHING_TO_PHONE, Route::Enter(phone)))
            .map_err(|refusal| refusal.answer());
    }
    current_switch_answer(target).map(|reply| (reply, Route::Park)).map_err(String::from)
}

/// switch_device: book the switch for after this turn, or say why not.
pub fn request_switch(target: Device) -> String {
    let (reply, gen, route) = match book_switch(target) {
        Ok(booking) => booking,
        Err(fact) => return fact,
    };
    tauri::async_runtime::spawn(async move {
        wait_until_quiet(
            crate::commands::chat::turn_in_flight,
            || crate::commands::chat::audio_queue_depth() > 0,
            SWITCH_POLL, SWITCH_SETTLE, SWITCH_DRAIN_MAX,
        )
        .await;
        match route {
            Route::Park => {
                if commit_switch(gen, target, || current_switch_answer(target)) {
                    eprintln!("[remote] switching to {:?}", target);
                    announce_park(target == Device::Desktop);
                }
            }
            // The phone answers on /phone; on ok it takes this PA as its occupant (unpark_for_entered).
            Route::Enter(phone) => {
                commit_enter(gen, &phone);
            }
        }
    });
    reply.into()
}

/// Every request cancels the booking before it, even one answered "already
/// there" or refused: the latest answer Hermes heard is what happens.
/// Ok((reply, generation, route)) when a switch is booked.
fn book_switch(target: Device) -> Result<(&'static str, u64, Route), String> {
    let mut gen = lock(&SWITCH);
    *gen += 1;
    switch_plan(target).map(|(reply, route)| (reply, *gen, route))
}

/// A booked enter, once the turn is over: ask the phone if no newer request
/// came in and it is still online, free and on screen as far as we know.
fn commit_enter(gen: u64, phone: &str) -> bool {
    let current = lock(&SWITCH);
    if *current != gen {
        return false;
    }
    if ui_attached() {
        eprintln!("[remote] enter dropped: a /ui page attached meanwhile");
        return false;
    }
    match super::phone::pick_for_enter() {
        Ok(id) if id == phone => super::phone::send_enter_request(phone),
        Ok(_) => {
            eprintln!("[remote] enter dropped: another phone is now the one to ask");
            false
        }
        Err(refusal) => {
            eprintln!("[remote] enter dropped: {}", refusal.answer());
            false
        }
    }
}

/// Runs `f` under the switch boundary: api/phone.rs commits a phone's
/// hello / state together with what it does to PARKED.
pub(super) fn with_switch<R>(f: impl FnOnce(&mut u64) -> R) -> R {
    f(&mut lock(&SWITCH))
}

/// A paired phone took this PA as its occupant (api/phone.rs, under SWITCH)
/// and is opening /ui + /audio: it drives once they are up. Like take_over,
/// the user's own hand (or their yes to enter_request): unpark now and cancel
/// any booking. Returns whether it was parked; the caller announces it.
pub(super) fn unpark_for_entered(gen: &mut u64) -> bool {
    *gen += 1;
    let was_parked = is_parked();
    set_parked(false);
    was_parked
}

/// A booked switch, once the turn is over: commits PARKED if no newer request
/// or take_over came in and the switch is still possible (the phone may have
/// gone to the background meanwhile).
fn commit_switch(gen: u64, target: Device, possible: impl FnOnce() -> Result<&'static str, &'static str>) -> bool {
    let current = lock(&SWITCH);
    if *current != gen {
        return false; // superseded by a newer request or by take_over
    }
    if let Err(fact) = possible() {
        eprintln!("[remote] switch to {:?} dropped: {}", target, fact);
        return false;
    }
    set_parked(target == Device::Desktop);
    if target == Device::Desktop {
        super::phone::release_occupant();
    }
    true
}

/// Returns once no turn has run and nothing has been queued for `settle`;
/// once the turn is over its audio gets at most `drain_max`. A turn starting
/// meanwhile (the user spoke again) restarts the wait.
async fn wait_until_quiet(
    turn_running: impl Fn() -> bool,
    audio_queued: impl Fn() -> bool,
    poll: Duration,
    settle: Duration,
    drain_max: Duration,
) {
    let mut drain_deadline = None;
    let mut quiet_since = None;
    loop {
        let now = tokio::time::Instant::now();
        if turn_running() {
            drain_deadline = None;
            quiet_since = None;
        } else {
            let deadline = *drain_deadline.get_or_insert(now + drain_max);
            if now >= deadline {
                return;
            }
            if audio_queued() {
                quiet_since = None;
            } else if now - *quiet_since.get_or_insert(now) >= settle {
                return;
            }
        }
        tokio::time::sleep(poll).await;
    }
}

/// After PARKED changed: tell the page and converge driver and audio source.
/// (Parking released a paired phone in the same commit — it closes /ui +
/// /audio and returns to its device list; one set up by hand keeps its parked page.)
pub(super) fn announce_park(parked: bool) {
    // The page reloads into (or out of) its parked screen.
    send_to_ui("ui-parked", json!({ "parked": parked }));
    if let Some(app) = APP.get() {
        sync_driver(app); // converges the audio source too
    }
}

pub const TAKE_OVER_STALE: &str = "take_over from a /ui page that has since been replaced";

/// The parked page's "take over" button: unpark now and cancel any booking.
/// Only the current /ui client may: a replaced page's queued take_over is stale.
fn take_over(app: &AppHandle, client: u64) -> Result<bool, String> {
    let _ = APP.set(app.clone());
    if !commit_take_over(client) {
        eprintln!("[remote] take_over from replaced /ui client {} ignored", client);
        return Err(TAKE_OVER_STALE.into());
    }
    eprintln!("[remote] phone takes over");
    announce_park(false);
    Ok(true)
}

fn commit_take_over(client: u64) -> bool {
    let mut gen = lock(&SWITCH);
    if lock(&UI_CLIENT).as_ref().is_none_or(|c| c.id != client) {
        return false;
    }
    *gen += 1;
    set_parked(false);
    true
}

pub const PARK_STALE: &str = "park from a /ui page that has since been replaced";

/// The phone page's "move to the computer" button: park now and cancel any
/// booking. Like take_over, only the current /ui client may.
fn park(app: &AppHandle, client: u64) -> Result<bool, String> {
    let _ = APP.set(app.clone());
    if !commit_park(client) {
        eprintln!("[remote] park from replaced /ui client {} ignored", client);
        return Err(PARK_STALE.into());
    }
    eprintln!("[remote] phone parks: moving to the desktop");
    announce_park(true);
    Ok(true)
}

fn commit_park(client: u64) -> bool {
    let mut gen = lock(&SWITCH);
    if lock(&UI_CLIENT).as_ref().is_none_or(|c| c.id != client) {
        return false;
    }
    *gen += 1;
    set_parked(true);
    super::phone::release_occupant();
    true
}

// ── /audio ───────────────────────────────────────────────────────────────────

pub async fn audio_ws(
    State(state): State<Arc<ServerState>>,
    Query(query): Query<TokenQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let Some((_, revoked)) = super::pairing::authenticate(&state.api_key, query.token()) else {
        eprintln!("[remote] /audio rejected: bad or missing token");
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let app = state.app.clone();
    ws.on_upgrade(move |socket| audio_session(app, socket, revoked))
}

fn sync_source(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || crate::voice::remote::sync_source(&app));
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum PhoneMessage {
    Playing { seq: u64 },
    Done { seq: u64 },
    Camera {
        on: bool,
        #[serde(default)]
        facing: Option<crate::voice::remote::Facing>,
        #[serde(default)]
        switching: bool,
    },
    Snapshot { id: u64, #[serde(default)] jpeg: Option<String> },
}

fn on_phone_message(conn: u64, message: PhoneMessage) -> bool {
    use crate::voice::remote as phone;
    match message {
        PhoneMessage::Playing { seq } => phone::on_report(conn, "playing", seq),
        PhoneMessage::Done { seq } => phone::on_report(conn, "done", seq),
        PhoneMessage::Camera { on, facing, switching } => {
            phone::on_camera(conn, on.then_some(phone::Camera { facing, switching }))
        }
        PhoneMessage::Snapshot { id, jpeg } => phone::on_snapshot(conn, id, jpeg),
    }
}

/// Upstream: binary frames = PCM16LE 16 kHz mono; text = `{"type":"playing"|"done","seq":N}`,
/// `{"type":"camera","on":bool,"facing":"front"|"back","switching":bool}`, `{"type":"snapshot","id":N,"jpeg":"<base64>"}` (no
/// `jpeg` = camera unavailable).
/// Downstream: `{"type":"play","seq":N}` then one binary frame (the sentence's
/// mp3), `{"type":"clear"}` (drop everything queued and stop now), or
/// `{"type":"snapshot","id":N}` (send the current camera frame), or
/// `{"type":"wake"}` (the wake word was heard: light the screen, show the page).
async fn audio_session(app: AppHandle, socket: WebSocket, mut revoked: Revoked) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Outbound>();
    let conn = crate::voice::remote::attach_phone(tx);
    eprintln!("[remote] /audio phone {} connected", conn);
    sync_source(&app);

    // Ends when a newer phone replaces this one (its sender is dropped).
    let mut writer = tokio::spawn(async move {
        let mut ping = tokio::time::interval(PING_EVERY);
        loop {
            let sent = tokio::select! {
                out = rx.recv() => match out {
                    Some(Outbound::Play { seq, audio }) => {
                        let header = json!({ "type": "play", "seq": seq }).to_string();
                        match sink.send(Message::Text(header.into())).await {
                            Ok(()) => sink.send(Message::Binary(audio.into())).await,
                            Err(e) => Err(e),
                        }
                    }
                    Some(Outbound::Clear) => sink.send(Message::Text(r#"{"type":"clear"}"#.into())).await,
                    Some(Outbound::Snapshot { id }) => {
                        sink.send(Message::Text(json!({ "type": "snapshot", "id": id }).to_string().into())).await
                    }
                    Some(Outbound::Wake) => sink.send(Message::Text(r#"{"type":"wake"}"#.into())).await,
                    None => break,
                },
                _ = ping.tick() => sink.send(Message::Ping(Default::default())).await,
            };
            if sent.is_err() {
                break;
            }
        }
    });

    loop {
        tokio::select! {
            _ = &mut writer => break,
            _ = revoked.wait() => {
                eprintln!("[remote] /audio phone {} closed: its phone was removed", conn);
                break;
            }
            frame = tokio::time::timeout(IDLE_TIMEOUT, stream.next()) => match frame {
                // A replaced phone is cut off at its next frame, even if its
                // writer is still stuck sending to a slow socket.
                Ok(Some(Ok(Message::Binary(pcm)))) => if !crate::voice::remote::push_pcm(conn, &pcm) { break },
                Ok(Some(Ok(Message::Text(text)))) => match serde_json::from_str::<PhoneMessage>(text.as_str()) {
                    Ok(message) => if !on_phone_message(conn, message) { break },
                    Err(e) => eprintln!("[remote] /audio bad message: {}", e),
                },
                Ok(Some(Err(_))) | Ok(Some(Ok(Message::Close(_)))) | Ok(None) | Err(_) => break,
                Ok(Some(Ok(_))) => {}
            },
        }
    }
    writer.abort();
    eprintln!("[remote] /audio phone {} disconnected", conn);
    if crate::voice::remote::detach_phone(conn) {
        sync_source(&app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ui_client(id: u64) -> UiClient {
        let (tx, _rx) = mpsc::unbounded_channel();
        let (kick, _kicked) = oneshot::channel();
        UiClient { id, tx, visible: true, _kick: kick }
    }

    fn test_lock() -> MutexGuard<'static, ()> {
        lock(&crate::voice::remote::TEST_LOCK)
    }

    #[test]
    fn driver_follows_the_attached_client_whatever_order_sessions_finish_in() {
        let _t = test_lock();
        // Old client 1 is replaced by client 2; 1's disconnect sync may run
        // before or after 2's connect sync. Either way 2 ends up driving, and
        // the replacement itself is one handover (the new page has no
        // conversation state, so the old conversation must be stopped).
        let mut handovers = Vec::new();
        *lock(&UI_CLIENT) = Some(ui_client(1));
        assert_eq!(converge_driver(|w, r| handovers.push((w, r))), Some(Some(1)));
        *lock(&UI_CLIENT) = Some(ui_client(2)); // 2 replaces 1
        assert_eq!(converge_driver(|w, r| handovers.push((w, r))), Some(Some(2)), "phone → phone hands over");
        assert_eq!(converge_driver(|_, _| panic!("1's late disconnect must not hand over again")), None);
        assert!(is_ui_remote_active());
        *lock(&UI_CLIENT) = None; // 2 leaves
        assert_eq!(converge_driver(|w, r| handovers.push((w, r))), Some(None));
        assert!(!is_ui_remote_active());
        assert_eq!(handovers, vec![(false, true), (true, true), (true, false)]);
    }

    #[test]
    fn a_parked_phone_never_drives_even_after_reconnecting() {
        let _t = test_lock();
        let mut handovers = Vec::new();
        *lock(&UI_CLIENT) = Some(ui_client(11));
        assert_eq!(converge_driver(|w, r| handovers.push((w, r))), Some(Some(11)));
        set_parked(true); // switch_device(desktop) ran
        assert_eq!(converge_driver(|w, r| handovers.push((w, r))), Some(None), "desktop drives");
        assert!(!is_ui_remote_active());
        *lock(&UI_CLIENT) = Some(ui_client(12)); // the page reloads / the app returns to the foreground
        assert_eq!(converge_driver(|_, _| panic!("reconnecting must not unpark")), None);
        set_parked(false); // take_over
        assert_eq!(converge_driver(|w, r| handovers.push((w, r))), Some(Some(12)));
        assert_eq!(handovers, vec![(false, true), (true, false), (false, true)]);
        *lock(&UI_CLIENT) = None;
        assert_eq!(converge_driver(|_, _| {}), Some(None));
    }

    #[test]
    fn the_phone_is_the_audio_source_only_while_its_page_drives() {
        use crate::voice::remote::{attach_phone, detach_phone, phone_is_source};
        let _t = test_lock();
        let audio = || {
            let (tx, rx) = mpsc::unbounded_channel();
            std::mem::forget(rx);
            attach_phone(tx)
        };
        // /audio first (the app's service reconnects after a PA restart), /ui later.
        let conn = audio();
        converge_driver(|_, _| {});
        assert!(!phone_is_source(), "/audio alone: the desktop drives, local mic and speaker");
        *lock(&UI_CLIENT) = Some(ui_client(61));
        converge_driver(|_, _| {});
        assert!(phone_is_source(), "the page drives → the phone's mic and speaker");
        // Parked → local; taken over → the phone again.
        set_parked(true);
        converge_driver(|_, _| {});
        assert!(!phone_is_source());
        set_parked(false);
        converge_driver(|_, _| {});
        assert!(phone_is_source());
        // The page goes away while /audio stays.
        *lock(&UI_CLIENT) = None;
        converge_driver(|_, _| {});
        assert!(!phone_is_source(), "/ui gone → back to local");
        detach_phone(conn);

        // The other order: /ui first, /audio later.
        *lock(&UI_CLIENT) = Some(ui_client(62));
        converge_driver(|_, _| {});
        assert!(!phone_is_source(), "no /audio yet");
        let conn = audio();
        assert!(phone_is_source());
        detach_phone(conn);
        assert!(!phone_is_source());
        *lock(&UI_CLIENT) = None;
        converge_driver(|_, _| {});
    }

    #[test]
    fn a_parked_page_can_only_ask_and_take_over() {
        assert!(command_allowed("send_message", false));
        assert!(!command_allowed("send_message", true));
        assert!(!command_allowed("respond_computer_approval", true));
        assert!(command_allowed("take_over", true) && command_allowed("is_ui_parked", true));
        assert!(!command_allowed("quit_app", false) && !command_allowed("quit_app", true));
        for cmd in PARKED_COMMANDS {
            assert!(UI_COMMANDS.contains(cmd));
        }
    }

    #[test]
    fn switch_answers_cover_every_device_state() {
        use Device::*;
        let s = |parked, ui, foreground, audio| PhoneState { parked, ui, foreground, audio, occupied: false };
        let held = |ui| PhoneState { parked: false, ui, foreground: false, audio: false, occupied: true };
        assert_eq!(switch_answer(Desktop, held(false)), Ok(SWITCHING_TO_DESKTOP), "a paired phone gone quiet still names this PA");
        assert_eq!(switch_answer(Desktop, held(true)), Ok(SWITCHING_TO_DESKTOP));
        assert_eq!(switch_answer(Desktop, PhoneState { parked: true, ..held(false) }), Err(ALREADY_ON_DESKTOP));
        assert_eq!(switch_answer(Desktop, s(false, true, true, true)), Ok(SWITCHING_TO_DESKTOP));
        assert_eq!(switch_answer(Desktop, s(false, false, false, true)), Err(ALREADY_ON_DESKTOP), "/audio alone: voice follows the desktop driver");
        assert_eq!(switch_answer(Desktop, s(true, true, true, true)), Err(ALREADY_ON_DESKTOP));
        assert_eq!(switch_answer(Desktop, s(false, false, false, false)), Err(ALREADY_ON_DESKTOP));
        assert_eq!(switch_answer(Phone, s(false, true, false, true)), Err(ALREADY_ON_PHONE));
        assert_eq!(switch_answer(Phone, s(true, true, true, true)), Ok(SWITCHING_TO_PHONE));
        assert_eq!(switch_answer(Phone, s(true, true, false, true)), Err(PHONE_UI_ABSENT), "app in the background, socket alive");
        assert_eq!(switch_answer(Phone, s(true, false, false, true)), Err(PHONE_UI_ABSENT), "page gone");
        assert_eq!(switch_answer(Phone, s(true, true, true, false)), Err(PHONE_UI_ABSENT));
        assert_eq!(switch_answer(Phone, s(false, false, false, false)), Err(PHONE_UI_ABSENT));
    }

    #[test]
    fn only_the_current_client_reports_its_own_visibility() {
        let _t = test_lock();
        let mut client = ui_client(21);
        client.visible = false;
        *lock(&UI_CLIENT) = Some(client);
        assert!(!ui_in_foreground(), "unknown until the page reports");
        set_ui_visible(20, true); // a replaced page's late report
        assert!(!ui_in_foreground());
        set_ui_visible(21, true);
        assert!(ui_in_foreground());
        set_ui_visible(21, false);
        assert!(!ui_in_foreground());
        // Through the session loop's inline path, in order.
        let report = |v: bool| report_visibility(21, &json!({ "id": 1, "cmd": "set_ui_visible", "args": { "visible": v } }).to_string());
        assert!(report(false).unwrap().contains(r#""ok":false"#));
        report(true);
        assert!(ui_in_foreground());
        assert!(report_visibility(21, r#"{"id":2,"cmd":"send_message","args":{}}"#).is_none(), "others are dispatched");
        *lock(&UI_CLIENT) = None;
    }

    /// A phone attached on /ui (`client`, foreground) and /audio; returns the /audio connection.
    fn phone_online(client: u64) -> u64 {
        *lock(&UI_CLIENT) = Some(ui_client(client));
        let (tx, rx) = mpsc::unbounded_channel();
        std::mem::forget(rx); // keep the connection's channel open
        crate::voice::remote::attach_phone(tx)
    }

    fn phone_offline(conn: u64) {
        *lock(&UI_CLIENT) = None;
        crate::voice::remote::detach_phone(conn);
        set_parked(false);
    }

    #[test]
    fn asking_for_the_current_device_cancels_the_booking_the_other_way() {
        let _t = test_lock();
        let conn = phone_online(31);
        // The phone drives: book the desktop, then ask for the phone again.
        let (reply, gen, _) = book_switch(Device::Desktop).unwrap();
        assert_eq!(reply, SWITCHING_TO_DESKTOP);
        assert_eq!(book_switch(Device::Phone).map(|b| b.0), Err(ALREADY_ON_PHONE.into()));
        assert!(!commit_switch(gen, Device::Desktop, || Ok("")), "the desktop booking is cancelled");
        assert!(!is_parked());

        // The desktop drives (phone parked, page on screen): book the phone, then ask for the desktop again.
        set_parked(true);
        let (reply, gen, _) = book_switch(Device::Phone).unwrap();
        assert_eq!(reply, SWITCHING_TO_PHONE);
        assert_eq!(book_switch(Device::Desktop).map(|b| b.0), Err(ALREADY_ON_DESKTOP.into()));
        assert!(!commit_switch(gen, Device::Phone, || Ok("")), "the phone booking is cancelled");
        assert!(is_parked());

        // An untouched booking still runs.
        let (_, gen, _) = book_switch(Device::Phone).unwrap();
        assert!(commit_switch(gen, Device::Phone, || current_switch_answer(Device::Phone)));
        assert!(!is_parked());
        phone_offline(conn);
    }

    #[test]
    fn a_take_over_during_a_late_booking_commit_still_wins() {
        let _t = test_lock();
        let conn = phone_online(41);
        let (_, gen, _) = book_switch(Device::Desktop).unwrap();
        // The old booking has passed its checks when the user presses "take over".
        let checked = Arc::new(std::sync::Barrier::new(2));
        let booking = {
            let checked = checked.clone();
            std::thread::spawn(move || {
                commit_switch(gen, Device::Desktop, || {
                    checked.wait();
                    std::thread::sleep(Duration::from_millis(100)); // take_over arrives meanwhile
                    Ok("")
                })
            })
        };
        checked.wait();
        assert!(commit_take_over(41), "take_over waits for the commit, then runs");
        assert!(booking.join().unwrap());
        assert!(!is_parked(), "the user's take_over is the last word");

        // The other order: take_over first, then the late booking finds itself cancelled.
        let (_, gen, _) = book_switch(Device::Desktop).unwrap();
        assert!(commit_take_over(41));
        assert!(!commit_switch(gen, Device::Desktop, || Ok("")));
        assert!(!is_parked());
        phone_offline(conn);
    }

    #[test]
    fn a_replaced_pages_queued_take_over_is_stale() {
        let _t = test_lock();
        let conn = phone_online(51);
        set_parked(true);
        // Page 51 sends take_over; before it runs, page 52 replaces it (still parked).
        *lock(&UI_CLIENT) = Some(ui_client(52));
        let (_, gen, _) = book_switch(Device::Phone).unwrap(); // 52's own booking, through Hermes
        assert!(!commit_take_over(51), "51 is no longer the /ui client");
        assert!(is_parked(), "a stale take_over leaves PARKED alone");
        assert!(commit_switch(gen, Device::Phone, || Ok("")), "…and does not cancel the new page's booking");
        assert!(!is_parked());
        set_parked(true);
        assert!(commit_take_over(52));
        assert!(!is_parked());
        phone_offline(conn);
    }

    #[test]
    fn the_current_page_parks_at_once_and_cancels_the_booking() {
        let _t = test_lock();
        let conn = phone_online(71);
        // A switch_device(desktop) booked for after the turn; the user presses the button first.
        let (_, gen, _) = book_switch(Device::Desktop).unwrap();
        assert!(commit_park(71));
        assert!(is_parked(), "parked right away, not after the turn");
        assert!(!commit_switch(gen, Device::Desktop, || Ok("")), "the booking is cancelled");
        // Parked, Hermes books a switch back to the phone; pressing the button again cancels it.
        let (_, gen, _) = book_switch(Device::Phone).unwrap();
        assert!(commit_park(71));
        assert!(!commit_switch(gen, Device::Phone, || Ok("")));
        assert!(is_parked());
        // The parked page's take_over still works.
        assert!(commit_take_over(71));
        assert!(!is_parked());
        phone_offline(conn);
    }

    #[test]
    fn a_replaced_pages_queued_park_is_stale() {
        let _t = test_lock();
        let conn = phone_online(81);
        // Page 81 sends park; before it runs, page 82 replaces it and books a switch through Hermes.
        *lock(&UI_CLIENT) = Some(ui_client(82));
        let (_, gen, _) = book_switch(Device::Desktop).unwrap();
        assert!(!commit_park(81), "81 is no longer the /ui client");
        assert!(!is_parked(), "a stale park leaves PARKED alone");
        assert!(commit_switch(gen, Device::Desktop, || Ok("")), "…and does not cancel the new page's booking");
        set_parked(false);
        *lock(&UI_CLIENT) = None;
        assert!(!commit_park(82), "no /ui client: refused");
        assert!(!is_parked());
        phone_offline(conn);
    }

    #[test]
    fn a_desktop_request_releases_a_phone_that_dropped_off_while_using_this_pa() {
        use super::super::phone::{self, commit_hello, commit_state, disconnect, occupied_by_me, pick_for_enter};
        use super::super::phone::tests::{conn, me, released};
        let _t = test_lock();
        *lock(&UI_CLIENT) = None;
        set_parked(false);
        // The phone uses this PA; then Wi-Fi drops /ui and /phone. It keeps
        // naming this PA for its 90 s lease.
        let (c1, _rx1) = conn(901);
        commit_hello("rv-phone", "Pixel", c1, me(), true);
        disconnect("rv-phone", 901);
        assert!(occupied_by_me() && !ui_attached());
        // switch_device(desktop) is booked, not answered "already on the desktop".
        let (reply, gen, route) = book_switch(Device::Desktop).unwrap();
        assert_eq!((reply, route), (SWITCHING_TO_DESKTOP, Route::Park));
        assert!(commit_switch(gen, Device::Desktop, || current_switch_answer(Device::Desktop)));
        assert!(is_parked());
        // Back within the lease, still naming this PA: it is told to let go instead of taking it back.
        let (c2, mut rx2) = conn(902);
        assert!(!commit_hello("rv-phone", "Pixel", c2, me(), true), "no unpark");
        assert!(released(&mut rx2));
        assert!(is_parked());
        // It lets go: the phone is free for any PA (B's enter_request is no longer busy).
        commit_state("rv-phone", 902, None, true);
        assert!(!occupied_by_me());
        assert_eq!(pick_for_enter(), Ok("rv-phone".into()));
        assert_eq!(book_switch(Device::Desktop).map(|b| b.0), Err(ALREADY_ON_DESKTOP.into()));
        phone::forget("rv-phone");
        set_parked(false);
    }

    #[test]
    fn a_parked_page_cannot_park_again() {
        assert!(command_allowed("park", false));
        assert!(!command_allowed("park", true));
    }

    fn quiet_after(turn: Arc<AtomicBool>, audio: Arc<AtomicBool>, drain_max: Duration) -> std::time::Duration {
        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let start = std::time::Instant::now();
        rt.block_on(wait_until_quiet(
            move || turn.load(Ordering::SeqCst),
            move || audio.load(Ordering::SeqCst),
            Duration::from_millis(5), Duration::from_millis(40), drain_max,
        ));
        start.elapsed()
    }

    fn flip_after(flag: &Arc<AtomicBool>, ms: u64, value: bool) {
        let flag = flag.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(ms));
            flag.store(value, Ordering::SeqCst);
        });
    }

    #[test]
    fn a_booked_switch_waits_for_the_turn_and_its_audio_however_the_turn_ends() {
        // Finished, cancelled or failed all look the same here: the turn stops running.
        let (turn, audio) = (Arc::new(AtomicBool::new(true)), Arc::new(AtomicBool::new(true)));
        flip_after(&turn, 100, false);
        flip_after(&audio, 200, false);
        let waited = quiet_after(turn, audio, Duration::from_secs(5));
        assert!(waited >= Duration::from_millis(240), "audio end + settle, waited {waited:?}");
        assert!(waited < Duration::from_secs(2));
    }

    #[test]
    fn a_booked_switch_gives_the_audio_at_most_the_drain_limit() {
        let (turn, audio) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(true)));
        let waited = quiet_after(turn, audio, Duration::from_millis(150));
        assert!(waited >= Duration::from_millis(150) && waited < Duration::from_secs(1), "waited {waited:?}");
    }

    #[test]
    fn a_new_turn_during_the_wait_restarts_it() {
        let (turn, audio) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        flip_after(&turn, 20, true); // the user speaks again before the settle is over
        flip_after(&turn, 200, false);
        let waited = quiet_after(turn, audio, Duration::from_secs(5));
        assert!(waited >= Duration::from_millis(240), "waited {waited:?}");
    }

    #[test]
    fn token_must_be_configured_present_and_equal() {
        assert!(!token_ok("", None));
        assert!(!token_ok("", Some("")), "unset key rejects even an empty token");
        assert!(!token_ok("", Some("anything")));
        assert!(!token_ok("secret", None));
        assert!(!token_ok("secret", Some("")));
        assert!(!token_ok("secret", Some("secreT")));
        assert!(!token_ok("secret", Some("secret ")));
        assert!(!token_ok("secret", Some("secre")));
        assert!(token_ok("secret", Some("secret")));
    }

    #[test]
    fn whitelist_keeps_desktop_only_commands_out() {
        for cmd in ["quit_app", "open_settings_window", "save_window_position", "save_config",
                    "open_chat_history", "save_settings_page_config", "install_wake_model",
                    "remove_speaker", "start_enroll_recording", "update_hotkey"] {
            assert!(!UI_COMMANDS.contains(&cmd), "{cmd} must not be reachable from /ui");
        }
    }

    #[test]
    fn every_whitelisted_command_has_a_dispatch_arm() {
        // A whitelisted name without an arm would fall to the refusal branch;
        // keep the two lists in step by checking the source.
        let src = include_str!("remote.rs");
        for cmd in UI_COMMANDS {
            assert!(src.contains(&format!("\"{cmd}\" =>")), "{cmd} has no dispatch arm");
        }
    }

    #[test]
    fn ui_events_never_include_the_desktop_driver_signal() {
        assert!(!UI_EVENTS.contains(&"ui-remote-active"));
        assert!(!UI_EVENTS.contains(&"tray-open-history"));
    }

    #[test]
    fn asset_paths_map_the_ui_prefix_onto_the_bundle() {
        let p = |s: &str| asset_path(s);
        assert_eq!(p("/ui").as_deref(), Some("/index.html"));
        assert_eq!(p("/ui/").as_deref(), Some("/index.html"));
        assert_eq!(p("/").as_deref(), Some("/index.html"));
        assert_eq!(p("/assets/index-1.js").as_deref(), Some("/assets/index-1.js"));
        assert_eq!(p("/ui/assets/index-1.js").as_deref(), Some("/assets/index-1.js"));
        assert_eq!(p("/uikit.js").as_deref(), Some("/uikit.js"));
        assert_eq!(p("/app_start.mp3").as_deref(), Some("/app_start.mp3"));
        assert_eq!(p("/ui/../../.env"), None);
        assert_eq!(p("/assets/../x"), None);
    }

    #[test]
    fn arguments_read_camel_case_and_null_as_none() {
        let a = json!({ "ttsFormat": "mp3", "fixedLang": null });
        assert_eq!(arg::<Option<String>>(&a, "ttsFormat").unwrap().as_deref(), Some("mp3"));
        assert_eq!(arg::<Option<String>>(&a, "fixedLang").unwrap(), None);
        assert_eq!(arg::<Option<String>>(&a, "missing").unwrap(), None);
        assert!(arg::<String>(&a, "text").is_err(), "a required argument must be present");
    }
}
