// phone.rs — Paired phones' presence: WS /phone (FEAT-MP3)
//
// Every paired phone keeps one light socket to every PA it is paired with,
// from its foreground service, whether or not it is using that PA. That is how
// a PA knows which phones are around and who is using each, without scanning
// the LAN.
//
//   phone → PA  {"type":"hello","phone_id","phone_name","occupant":{pa_id,pa_name}|null,"foreground":bool}
//               {"type":"state","occupant":…,"foreground":bool}      whenever either changes
//               {"type":"enter_reply","req_id":N,"ok":true}
//               {"type":"enter_reply","req_id":N,"ok":false,"reason":"busy"|"not_foreground","busy_with":"Mac"}
//   PA → phone  {"type":"welcome","pa_id","pa_name"}                 answer to hello
//               {"type":"enter_request","req_id":N}                  this PA wants the phone
//               {"type":"release"}                                   this PA has moved back to its computer
//
// The phone alone decides who uses it (the occupant): a PA only learns it
// from hello / state and asks with enter_request; the phone refuses while
// another PA occupies it. A PA that sees itself become the occupant unparks
// (remote::unpark_for_entered) and the phone then opens /ui + /audio as before.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, oneshot};

use super::pairing::{self, Auth, Revoked};
use super::remote::TokenQuery;
use super::server::ServerState;

pub const NO_ONLINE_PHONE: &str = "没有在线的已配对手机";

pub fn busy_answer(pa_name: &str) -> String {
    format!("手机正在被「{pa_name}」上的 PA 使用，请先在那台电脑上让 PA 回到电脑")
}

const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const PING_EVERY: Duration = Duration::from_secs(10);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Occupant {
    pub pa_id: String,
    #[serde(default)]
    pub pa_name: String,
}

type Conn = (u64, mpsc::UnboundedSender<Message>, oneshot::Sender<()>);

#[derive(Default)]
struct PhoneEntry {
    name: String,
    /// Online: the current /phone connection and its outbox. A newer
    /// connection of the same phone replaces it (dropping `_kick`).
    conn: Option<Conn>,
    occupant: Option<Occupant>,
    foreground: bool,
    /// The last time the phone took this PA as its occupant.
    last_used: Option<Instant>,
    /// This PA moved back to its computer while the phone still names it the
    /// occupant: release it (again) until the phone says otherwise.
    owe_release: bool,
}

/// Lock order: remote::SWITCH, then PHONES. Never emit while holding it.
static PHONES: Mutex<Option<HashMap<String, PhoneEntry>>> = Mutex::new(None);
static NEXT_CONN: AtomicU64 = AtomicU64::new(0);
static NEXT_REQ: AtomicU64 = AtomicU64::new(0);

fn phones() -> MutexGuard<'static, Option<HashMap<String, PhoneEntry>>> {
    let mut guard = lock(&PHONES);
    guard.get_or_insert_with(HashMap::new);
    guard
}

fn my_id() -> &'static str {
    &pairing::identity().0
}

fn send(entry: &PhoneEntry, value: serde_json::Value) -> bool {
    entry.conn.as_ref().is_some_and(|(_, tx, _)| tx.send(Message::Text(value.to_string().into())).is_ok())
}

pub fn online_phones() -> Vec<String> {
    phones().as_ref().unwrap().iter().filter(|(_, e)| e.conn.is_some()).map(|(id, _)| id.clone()).collect()
}

// ── Occupancy (pure) ─────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
enum Effect {
    None,
    /// The phone just took this PA as its occupant.
    Entered,
    /// It names this PA, which has moved back to its computer meanwhile.
    Release,
}

fn apply_state(entry: &mut PhoneEntry, occupant: Option<Occupant>, foreground: bool, me: &str, now: Instant) -> Effect {
    let was_me = entry.occupant.as_ref().is_some_and(|o| o.pa_id == me);
    let is_me = occupant.as_ref().is_some_and(|o| o.pa_id == me);
    entry.occupant = occupant;
    entry.foreground = foreground;
    if !is_me {
        entry.owe_release = false;
        return Effect::None;
    }
    entry.last_used = Some(now);
    if entry.owe_release {
        Effect::Release
    } else if was_me {
        Effect::None
    } else {
        Effect::Entered
    }
}

#[derive(Debug, PartialEq)]
pub enum Refusal {
    NoPhone,
    Busy(String),
    NotForeground,
}

impl Refusal {
    pub fn answer(&self) -> String {
        match self {
            Refusal::NoPhone => NO_ONLINE_PHONE.into(),
            Refusal::Busy(name) => busy_answer(name),
            Refusal::NotForeground => super::remote::PHONE_UI_ABSENT.into(),
        }
    }
}

/// The online phone this PA would ask in: the one that used it last.
fn choose(map: &HashMap<String, PhoneEntry>, me: &str) -> Result<String, Refusal> {
    let (id, entry) = map
        .iter()
        .filter(|(_, e)| e.conn.is_some())
        .max_by_key(|(_, e)| e.last_used)
        .ok_or(Refusal::NoPhone)?;
    match &entry.occupant {
        Some(o) if o.pa_id != me => Err(Refusal::Busy(if o.pa_name.is_empty() { "另一台电脑".into() } else { o.pa_name.clone() })),
        _ if !entry.foreground => Err(Refusal::NotForeground),
        _ => Ok(id.clone()),
    }
}

/// switch_device(phone) with no /ui page attached: which phone to ask, or why none.
/// The phone checks again when asked; this answers Hermes at once.
pub fn pick_for_enter() -> Result<String, Refusal> {
    choose(phones().as_ref().unwrap(), my_id())
}

/// Asks the phone to come over (remote::commit_enter, under SWITCH).
/// Returns false when it has gone offline meanwhile.
pub fn send_enter_request(phone_id: &str) -> bool {
    let req_id = NEXT_REQ.fetch_add(1, Ordering::AcqRel) + 1;
    let sent = phones()
        .as_ref()
        .unwrap()
        .get(phone_id)
        .is_some_and(|e| send(e, json!({ "type": "enter_request", "req_id": req_id })));
    if sent {
        eprintln!("[phone] enter_request {} → {}", req_id, phone_id);
    }
    sent
}

/// Whether a paired phone still names this PA its occupant, online or not
/// (an offline phone keeps it for its 90 s lease).
pub fn occupied_by_me() -> bool {
    let me = my_id();
    phones().as_ref().unwrap().values().any(|e| e.occupant.as_ref().is_some_and(|o| o.pa_id == me))
}

/// This PA moved back to its computer (park / switch_device(desktop), under
/// remote::SWITCH with PARKED): every phone naming it the occupant is
/// released. A phone offline now is told when it comes back.
pub fn release_occupant() {
    let me = my_id();
    for (id, entry) in phones().as_mut().unwrap().iter_mut() {
        if entry.occupant.as_ref().is_some_and(|o| o.pa_id == me) {
            entry.owe_release = true;
            if send(entry, json!({ "type": "release" })) {
                eprintln!("[phone] release → {}", id);
            }
        }
    }
}

/// Tests sharing PHONES (under voice::remote::TEST_LOCK) clean up after themselves.
#[cfg(test)]
pub(super) fn forget(phone_id: &str) {
    phones().as_mut().unwrap().remove(phone_id);
}

// ── /phone ───────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PhoneIn {
    Hello {
        #[serde(default)]
        phone_id: Option<String>,
        #[serde(default)]
        phone_name: Option<String>,
        #[serde(default)]
        occupant: Option<Occupant>,
        #[serde(default)]
        foreground: bool,
    },
    State {
        #[serde(default)]
        occupant: Option<Occupant>,
        #[serde(default)]
        foreground: bool,
    },
    EnterReply {
        req_id: u64,
        ok: bool,
        #[serde(default)]
        reason: Option<String>,
        #[serde(default)]
        busy_with: Option<String>,
    },
}

pub async fn phone_ws(
    State(state): State<Arc<ServerState>>,
    Query(query): Query<TokenQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let Some((auth, revoked)) = pairing::authenticate(&state.api_key, query.token()) else {
        eprintln!("[phone] /phone rejected: bad or missing token");
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let app = state.app.clone();
    ws.on_upgrade(move |socket| phone_session(app, socket, auth, revoked))
}

/// The phone's id and name: a device token fixes the id; the key (a phone
/// set up by hand) takes them from hello.
fn identify(auth: &Auth, hello_id: Option<String>, hello_name: Option<String>) -> Option<(String, String)> {
    match auth {
        Auth::Phone(id) => Some((id.clone(), pairing::phone_name(id).unwrap_or_else(|| id.clone()))),
        Auth::Key => {
            let id = hello_id.filter(|id| (1..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))?;
            let name: String = hello_name.unwrap_or_default().chars().filter(|c| !c.is_control()).take(64).collect();
            Some((id.clone(), if name.trim().is_empty() { id } else { name.trim().to_string() }))
        }
    }
}

/// A phone's hello / state is applied and its effect committed in one step,
/// under remote's SWITCH, then PHONES (lesson-20261104-143500): a replaced
/// connection's message, or a park landing in between, cannot be undone by
/// an effect worked out earlier. Returns true when PARKED was cleared: the
/// caller announces it once both locks are released.
fn commit_effect(gen: &mut u64, entry: &PhoneEntry, effect: Effect, phone_id: &str) -> bool {
    match effect {
        Effect::None => false,
        Effect::Entered => {
            eprintln!("[phone] {} now uses this PA", phone_id);
            super::remote::unpark_for_entered(gen)
        }
        Effect::Release => {
            send(entry, json!({ "type": "release" }));
            eprintln!("[phone] release → {} (owed)", phone_id);
            false
        }
    }
}

/// hello on a new connection: it replaces the phone's older one (dropping its kick).
pub(super) fn commit_hello(phone_id: &str, name: &str, conn: Conn, occupant: Option<Occupant>, foreground: bool) -> bool {
    super::remote::with_switch(|gen| {
        let mut map = phones();
        let entry = map.as_mut().unwrap().entry(phone_id.to_string()).or_default();
        entry.name = name.to_string();
        entry.conn = Some(conn);
        let effect = apply_state(entry, occupant, foreground, my_id(), Instant::now());
        commit_effect(gen, entry, effect, phone_id)
    })
}

/// state from connection `conn`; ignored once a newer connection replaced it.
pub(super) fn commit_state(phone_id: &str, conn: u64, occupant: Option<Occupant>, foreground: bool) -> bool {
    super::remote::with_switch(|gen| {
        let mut map = phones();
        let Some(entry) = map.as_mut().unwrap().get_mut(phone_id).filter(|e| e.conn.as_ref().is_some_and(|(c, _, _)| *c == conn)) else {
            return false;
        };
        let effect = apply_state(entry, occupant, foreground, my_id(), Instant::now());
        commit_effect(gen, entry, effect, phone_id)
    })
}

/// Connection `conn` closed: the phone is offline unless a newer one replaced it.
pub(super) fn disconnect(phone_id: &str, conn: u64) {
    if let Some(entry) = phones().as_mut().unwrap().get_mut(phone_id) {
        if entry.conn.as_ref().is_some_and(|(c, _, _)| *c == conn) {
            entry.conn = None;
        }
    }
}

async fn phone_session(app: AppHandle, socket: WebSocket, auth: Auth, mut revoked: Revoked) {
    let (mut sink, mut stream) = socket.split();

    // hello first.
    let hello = match tokio::time::timeout(HELLO_TIMEOUT, stream.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => serde_json::from_str::<PhoneIn>(text.as_str()).ok(),
        _ => None,
    };
    let Some(PhoneIn::Hello { phone_id, phone_name, occupant, foreground }) = hello else {
        eprintln!("[phone] /phone closed: no hello");
        return;
    };
    let Some((phone_id, name)) = identify(&auth, phone_id, phone_name) else {
        eprintln!("[phone] /phone closed: invalid phone_id");
        return;
    };

    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    let (kick, mut kicked) = oneshot::channel::<()>();
    let conn = NEXT_CONN.fetch_add(1, Ordering::AcqRel) + 1;
    let (pa_id, pa_name) = pairing::identity();
    let _ = tx.send(Message::Text(json!({ "type": "welcome", "pa_id": pa_id, "pa_name": pa_name }).to_string().into()));
    let unparked = commit_hello(&phone_id, &name, (conn, tx.clone(), kick), occupant, foreground);
    eprintln!("[phone] {} online ({})", name, phone_id);
    if unparked {
        super::remote::announce_park(false);
    }
    let _ = app.emit(pairing::PAIRING_CHANGED, ());
    if let Auth::Phone(id) = &auth {
        let id = id.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let _ = crate::commands::settings_repository::touch_paired_phone(&id);
        });
    }

    let mut writer = tokio::spawn(async move {
        let mut ping = tokio::time::interval(PING_EVERY);
        loop {
            let sent = tokio::select! {
                msg = rx.recv() => match msg {
                    Some(msg) => sink.send(msg).await,
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
            _ = &mut kicked => break,
            _ = revoked.wait() => {
                eprintln!("[phone] {} removed: closing /phone", phone_id);
                break;
            }
            _ = &mut writer => break,
            frame = tokio::time::timeout(IDLE_TIMEOUT, stream.next()) => match frame {
                Ok(Some(Ok(Message::Text(text)))) => match serde_json::from_str::<PhoneIn>(text.as_str()) {
                    Ok(PhoneIn::State { occupant, foreground }) => {
                        if commit_state(&phone_id, conn, occupant, foreground) {
                            super::remote::announce_park(false);
                        }
                    }
                    Ok(PhoneIn::EnterReply { req_id, ok, reason, busy_with }) => {
                        // ok: the phone now names this PA its occupant (its state follows) and opens /ui + /audio.
                        eprintln!(
                            "[phone] enter_reply {} from {}: {}",
                            req_id, phone_id,
                            if ok { "ok".to_string() } else { format!("{} {}", reason.unwrap_or_default(), busy_with.unwrap_or_default()) }
                        );
                    }
                    Ok(PhoneIn::Hello { .. }) => {}
                    Err(e) => eprintln!("[phone] bad message: {}", e),
                },
                Ok(Some(Ok(Message::Close(_)))) | Ok(Some(Err(_))) | Ok(None) | Err(_) => break,
                Ok(Some(Ok(_))) => {}
            },
        }
    }
    writer.abort();
    disconnect(&phone_id, conn);
    eprintln!("[phone] {} offline", phone_id);
    let _ = app.emit(pairing::PAIRING_CHANGED, ());
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const A: &str = "pa-A";
    const B: &str = "pa-B";

    fn occ(id: &str) -> Option<Occupant> {
        Some(Occupant { pa_id: id.into(), pa_name: format!("{id}-name") })
    }

    fn online(occupant: Option<Occupant>, foreground: bool) -> PhoneEntry {
        let (tx, rx) = mpsc::unbounded_channel();
        std::mem::forget(rx);
        let (kick, _) = oneshot::channel();
        PhoneEntry { name: "Pixel".into(), conn: Some((1, tx, kick)), occupant, foreground, ..Default::default() }
    }

    fn one(entry: PhoneEntry) -> HashMap<String, PhoneEntry> {
        HashMap::from([("phone".to_string(), entry)])
    }

    #[test]
    fn while_a_uses_the_phone_b_is_refused() {
        let map = one(online(occ(A), true));
        assert_eq!(choose(&map, B), Err(Refusal::Busy("pa-A-name".into())));
        assert!(Refusal::Busy("Mac".into()).answer().contains("「Mac」"));
        assert_eq!(choose(&map, A), Ok("phone".into()), "A itself may ask (the phone reconnects its /ui)");
    }

    #[test]
    fn after_a_releases_b_may_enter() {
        let mut map = one(online(occ(A), true));
        let entry = map.get_mut("phone").unwrap();
        assert_eq!(apply_state(entry, None, true, B, Instant::now()), Effect::None); // A parked: the phone reports no occupant
        assert_eq!(choose(&map, B), Ok("phone".into()));
    }

    #[test]
    fn a_offline_within_the_lease_still_blocks_b_and_after_it_does_not() {
        // The phone keeps naming A for 90 s after losing A, then clears it (android Occupancy / phone_sim).
        let mut map = one(online(occ(A), true));
        assert_eq!(choose(&map, B), Err(Refusal::Busy("pa-A-name".into())), "within the lease");
        apply_state(map.get_mut("phone").unwrap(), None, true, B, Instant::now()); // lease over
        assert_eq!(choose(&map, B), Ok("phone".into()));
    }

    #[test]
    fn no_online_phone_or_one_in_the_background() {
        let mut map = one(online(None, false));
        assert_eq!(choose(&map, A), Err(Refusal::NotForeground));
        map.get_mut("phone").unwrap().conn = None;
        assert_eq!(choose(&map, A), Err(Refusal::NoPhone));
        assert_eq!(choose(&HashMap::new(), A), Err(Refusal::NoPhone));
    }

    #[test]
    fn the_most_recently_used_online_phone_is_asked() {
        let mut map = one(online(None, true));
        let mut other = online(None, true);
        other.last_used = Some(Instant::now());
        map.insert("other".into(), other);
        assert_eq!(choose(&map, A), Ok("other".into()));
    }

    #[test]
    fn becoming_the_occupant_enters_once() {
        let mut e = online(None, true);
        let now = Instant::now();
        assert_eq!(apply_state(&mut e, occ(A), true, A, now), Effect::Entered);
        assert_eq!(apply_state(&mut e, occ(A), false, A, now), Effect::None, "a foreground change is not a new entry");
        assert_eq!(apply_state(&mut e, occ(B), true, A, now), Effect::None);
        assert_eq!(apply_state(&mut e, occ(A), true, A, now), Effect::Entered);
    }

    #[test]
    fn a_release_missed_offline_is_sent_again_instead_of_entering() {
        let mut e = online(occ(A), true);
        e.owe_release = true; // A parked while the phone was away
        assert_eq!(apply_state(&mut e, occ(A), true, A, Instant::now()), Effect::Release);
        assert_eq!(apply_state(&mut e, None, true, A, Instant::now()), Effect::None);
        assert!(!e.owe_release, "settled once the phone lets go");
        assert_eq!(apply_state(&mut e, occ(A), true, A, Instant::now()), Effect::Entered, "a later entry is real");
    }

    /// A /phone connection's slot and the outbox behind it.
    pub(in crate::api) fn conn(id: u64) -> (Conn, mpsc::UnboundedReceiver<Message>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let (kick, kicked) = oneshot::channel();
        std::mem::forget(kicked);
        ((id, tx, kick), rx)
    }

    pub(in crate::api) fn me() -> Option<Occupant> {
        occ(my_id())
    }

    pub(in crate::api) fn released(rx: &mut mpsc::UnboundedReceiver<Message>) -> bool {
        std::iter::from_fn(|| rx.try_recv().ok()).any(|m| matches!(m, Message::Text(t) if t.as_str().contains(r#""release""#)))
    }

    fn test_lock() -> MutexGuard<'static, ()> {
        lock(&crate::voice::remote::TEST_LOCK)
    }

    /// Holds the switch boundary while `meanwhile` runs, with `message` from
    /// another thread already waiting for it (lesson-20261104-143500's barrier).
    fn while_waiting<R: Send + 'static>(message: impl FnOnce() -> R + Send + 'static, meanwhile: impl FnOnce()) -> R {
        let started = Arc::new(std::sync::Barrier::new(2));
        let waiting = {
            let started = started.clone();
            std::thread::spawn(move || {
                started.wait();
                message()
            })
        };
        super::super::remote::with_switch(|_| {
            started.wait();
            std::thread::sleep(Duration::from_millis(100)); // the message is now blocked on SWITCH
            meanwhile();
        });
        waiting.join().unwrap()
    }

    #[test]
    fn a_replaced_connections_state_cannot_unpark_after_the_new_hello() {
        use crate::voice::remote::{is_parked, set_parked};
        let _t = test_lock();
        set_parked(true);
        let (c1, _rx1) = conn(1001);
        assert!(!commit_hello("barrier-1", "Pixel", c1, None, true));
        // Connection 1001's state (naming this PA) is on its way when 1002's hello commits.
        let (c2, _rx2) = conn(1002);
        let unparked = while_waiting(
            || commit_state("barrier-1", 1001, me(), true),
            || {
                let mut map = phones();
                let entry = map.as_mut().unwrap().get_mut("barrier-1").unwrap();
                entry.conn = Some(c2); // what commit_hello does for 1002 (occupant: null)
            },
        );
        assert!(!unparked, "the stale connection's message is dropped whole");
        assert!(is_parked());
        assert!(!occupied_by_me());
        forget("barrier-1");
        set_parked(false);
    }

    #[test]
    fn a_park_landing_before_a_state_is_not_undone_by_it() {
        use crate::voice::remote::{is_parked, set_parked};
        let _t = test_lock();
        set_parked(false);
        let (c, mut rx) = conn(1011);
        commit_hello("barrier-2", "Pixel", c, me(), true); // the phone uses this PA
        // Its next state (still naming this PA) waits while the page's park commits.
        let unparked = while_waiting(
            || commit_state("barrier-2", 1011, me(), true),
            || {
                set_parked(true); // what commit_park does under SWITCH
                release_occupant();
            },
        );
        assert!(!unparked);
        assert!(is_parked(), "the park stands");
        assert!(released(&mut rx), "the phone is told to let go (again)");
        forget("barrier-2");
        set_parked(false);
    }

    #[test]
    fn hand_set_phones_identify_from_hello_and_paired_ones_from_the_token() {
        assert_eq!(identify(&Auth::Key, Some("p-1".into()), Some(" Pixel\n2 ".into())), Some(("p-1".into(), "Pixel2".into())));
        assert_eq!(identify(&Auth::Key, Some("bad id".into()), None), None);
        assert_eq!(identify(&Auth::Key, None, None), None);
        let (id, _) = identify(&Auth::Phone("tok-phone".into()), Some("spoofed".into()), None).unwrap();
        assert_eq!(id, "tok-phone", "a token's phone cannot claim another id");
    }

    #[test]
    fn messages_parse() {
        let m: PhoneIn = serde_json::from_str(r#"{"type":"hello","phone_id":"p","phone_name":"n","occupant":{"pa_id":"x","pa_name":"Mac"},"foreground":true}"#).unwrap();
        assert!(matches!(m, PhoneIn::Hello { occupant: Some(_), foreground: true, .. }));
        let m: PhoneIn = serde_json::from_str(r#"{"type":"state","occupant":null}"#).unwrap();
        assert!(matches!(m, PhoneIn::State { occupant: None, foreground: false }));
        let m: PhoneIn = serde_json::from_str(r#"{"type":"enter_reply","req_id":3,"ok":false,"reason":"busy","busy_with":"Mac"}"#).unwrap();
        assert!(matches!(m, PhoneIn::EnterReply { ok: false, .. }));
    }
}
