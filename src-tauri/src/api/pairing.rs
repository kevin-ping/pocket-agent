// pairing.rs — Pairing phones with this PA (FEAT-MP1 / FEAT-MP2 / SEC-MP1)
//
//   POST /pair/start   {phone_id, phone_name}  → a 6-digit code shows on this computer only
//   POST /pair/confirm {phone_id, code}        → {pa_id, pa_name, token}
//
// Both are open (the phone has no credentials yet); what guards them:
//   - the code never leaves this computer except on its own screen (settings
//     window, through a desktop-only Tauri command — not /ui, not HTTP);
//   - one pairing at a time, 2 minutes, a cooldown between starts so a
//     stranger cannot keep popping the settings window;
//   - 5 wrong codes void it, compared in constant time, and lock pairing for
//     5 minutes (a stranger restarting it every 10 s would otherwise get
//     thousands of guesses overnight);
//   - the token is 32 random bytes; settings.db keeps only its SHA-256.
// Removing a phone drops its token and cuts every socket it opened with it.
// Neither tokens nor codes are ever logged.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;

use super::server::ServerState;
use crate::commands::settings_repository::{self as repo, PairedPhoneRow};

pub const CODE_TTL: Duration = Duration::from_secs(120);
pub const MAX_ATTEMPTS: u32 = 5;
/// Between two /pair/start calls (each one brings up the settings window).
pub const START_COOLDOWN: Duration = Duration::from_secs(10);
/// After a pairing is voided by wrong codes, no new one starts for this long.
pub const LOCKOUT: Duration = Duration::from_secs(300);
const ID_MAX: usize = 64;
const NAME_MAX: usize = 64;

/// Settings window: the pairing panel or the paired list changed (no payload).
pub const PAIRING_CHANGED: &str = "pairing-changed";

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).expect("system random source");
    bytes
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Equal-length inputs are compared over every byte.
fn same_bytes(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn token_hash(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

// ── Identity (FEAT-MP1) ──────────────────────────────────────────────────────

static IDENTITY: OnceLock<(String, String)> = OnceLock::new();

fn new_uuid() -> String {
    let mut b = random_bytes::<16>();
    b[6] = (b[6] & 0x0f) | 0x40; // version 4
    b[8] = (b[8] & 0x3f) | 0x80; // RFC 4122 variant
    let h = hex(&b);
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// The name people know this computer by.
fn computer_name() -> String {
    let from_command = |cmd: &str, args: &[&str]| {
        std::process::Command::new(cmd)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    #[cfg(target_os = "macos")]
    let name = from_command("scutil", &["--get", "ComputerName"]);
    #[cfg(windows)]
    let name = std::env::var("COMPUTERNAME").ok();
    #[cfg(not(any(target_os = "macos", windows)))]
    let name: Option<String> = None;
    name.or_else(|| from_command("hostname", &[]))
        .unwrap_or_else(|| crate::platform::MACHINE.to_string())
}

/// (pa_id, pa_name). pa_id lives in settings.db; a database that cannot be
/// opened gets a per-run id (pairings then fail to save anyway).
pub fn identity() -> &'static (String, String) {
    IDENTITY.get_or_init(|| {
        if cfg!(test) {
            return ("test-pa".into(), "Test PA".into()); // tests never touch ~/.pocket-agent
        }
        let id = repo::pa_id(new_uuid).unwrap_or_else(|e| {
            eprintln!("[pair] pa_id unavailable: {e}");
            new_uuid()
        });
        (id, computer_name())
    })
}

// ── Pending pairing ──────────────────────────────────────────────────────────

struct Pending {
    phone_id: String,
    phone_name: String,
    code: String,
    expires: Instant,
    attempts: u32,
}

#[derive(Default)]
pub(crate) struct Pairing {
    pending: Option<Pending>,
    last_start: Option<Instant>,
    locked_until: Option<Instant>,
}

#[derive(Debug, PartialEq)]
pub(crate) enum StartError {
    /// Another phone is pairing right now.
    Busy,
    TooSoon,
    /// A pairing was voided by wrong codes a moment ago.
    Locked,
}

#[derive(Debug, PartialEq)]
pub(crate) enum ConfirmError {
    /// None in progress: never started, expired, finished, or voided by wrong codes.
    NoPairing,
    WrongCode { attempts_left: u32 },
}

impl Pairing {
    fn live(&mut self, now: Instant) -> Option<&mut Pending> {
        if self.pending.as_ref().is_some_and(|p| now >= p.expires) {
            self.pending = None;
        }
        self.pending.as_mut()
    }

    /// The same phone may start again (the user backed out and retried); its old code is void.
    pub(crate) fn start(&mut self, phone_id: &str, phone_name: &str, code: String, now: Instant) -> Result<(), StartError> {
        if self.locked_until.is_some_and(|t| now < t) {
            return Err(StartError::Locked);
        }
        if self.live(now).is_some_and(|p| p.phone_id != phone_id) {
            return Err(StartError::Busy);
        }
        if self.last_start.is_some_and(|t| now.duration_since(t) < START_COOLDOWN) {
            return Err(StartError::TooSoon);
        }
        self.last_start = Some(now);
        self.pending = Some(Pending {
            phone_id: phone_id.into(),
            phone_name: phone_name.into(),
            code,
            expires: now + CODE_TTL,
            attempts: 0,
        });
        Ok(())
    }

    /// Every wrong try counts, whichever phone_id it claims. Ok(phone name).
    pub(crate) fn confirm(&mut self, phone_id: &str, code: &str, now: Instant) -> Result<String, ConfirmError> {
        let Some(p) = self.live(now) else { return Err(ConfirmError::NoPairing) };
        let right = same_bytes(p.code.as_bytes(), code.as_bytes()) & same_bytes(p.phone_id.as_bytes(), phone_id.as_bytes());
        if !right {
            p.attempts += 1;
            let left = MAX_ATTEMPTS.saturating_sub(p.attempts);
            if left == 0 {
                self.pending = None;
                self.locked_until = Some(now + LOCKOUT);
            }
            return Err(ConfirmError::WrongCode { attempts_left: left });
        }
        Ok(self.pending.take().map(|p| p.phone_name).unwrap_or_default())
    }

    fn cancel(&mut self) {
        self.pending = None;
    }

    /// What the settings window shows: (phone name, code, seconds left).
    fn status(&mut self, now: Instant) -> Option<(String, String, u64)> {
        self.live(now)
            .map(|p| (p.phone_name.clone(), p.code.clone(), p.expires.saturating_duration_since(now).as_secs()))
    }
}

static PAIRING: Mutex<Pairing> = Mutex::new(Pairing { pending: None, last_start: None, locked_until: None });

fn new_code() -> String {
    // 32 bits mod 10^6: bias below 0.03 %.
    format!("{:06}", u32::from_le_bytes(random_bytes::<4>()) % 1_000_000)
}

fn valid_id(id: &str) -> bool {
    (1..=ID_MAX).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Control characters stripped, trimmed, at most NAME_MAX characters; None when empty.
fn clean_name(name: &str) -> Option<String> {
    let name: String = name.chars().filter(|c| !c.is_control()).take(NAME_MAX).collect();
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

// ── Paired phones and their sockets ──────────────────────────────────────────

struct Paired {
    phone_id: String,
    name: String,
    hash: [u8; 32],
}

/// Cache of settings.db's paired_phones, loaded on first use.
/// Lock order: PAIRED, then SESSIONS.
static PAIRED: Mutex<Option<Vec<Paired>>> = Mutex::new(None);
/// Sockets opened with a phone's token: (session, phone_id, dropped to cut it).
static SESSIONS: Mutex<Vec<(u64, String, oneshot::Sender<()>)>> = Mutex::new(Vec::new());
static NEXT_SESSION: AtomicU64 = AtomicU64::new(0);

fn parse_hash(hex_hash: &str) -> Option<[u8; 32]> {
    if hex_hash.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex_hash.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

fn paired() -> MutexGuard<'static, Option<Vec<Paired>>> {
    let mut guard = lock(&PAIRED);
    if guard.is_none() {
        let rows = if cfg!(test) { Ok(Vec::new()) } else { repo::paired_phones() };
        let rows = rows.unwrap_or_else(|e| {
            eprintln!("[pair] paired phones unavailable: {e}");
            Vec::new()
        });
        *guard = Some(
            rows.into_iter()
                .filter_map(|r| Some(Paired { hash: parse_hash(&r.token_hash)?, phone_id: r.phone_id, name: r.name }))
                .collect(),
        );
    }
    guard
}

/// How a socket authenticated.
#[derive(Clone, Debug, PartialEq)]
pub enum Auth {
    /// API_SERVER_KEY (the hand-entered key; older apps).
    Key,
    /// A paired phone's device token.
    Phone(String),
}

/// Resolves once the phone the socket belongs to has been removed; never for the key.
pub struct Revoked {
    session: Option<u64>,
    rx: Option<oneshot::Receiver<()>>,
}

impl Revoked {
    pub async fn wait(&mut self) {
        match self.rx.as_mut() {
            Some(rx) => {
                let _ = rx.await;
            }
            None => std::future::pending::<()>().await,
        }
    }
}

impl Drop for Revoked {
    fn drop(&mut self) {
        if let Some(id) = self.session {
            lock(&SESSIONS).retain(|(s, _, _)| *s != id);
        }
    }
}

fn find_phone<'a>(phones: &'a [Paired], token: &str) -> Option<&'a Paired> {
    let hash = Sha256::digest(token.as_bytes());
    // Every entry is compared, so timing says nothing about which one matched.
    phones.iter().fold(None, |found, p| if same_bytes(&p.hash, &hash) { Some(p) } else { found })
}

/// API_SERVER_KEY or a paired phone's token. The watch is registered under
/// the same lock removal takes, so a socket either is refused or gets cut.
pub fn authenticate(api_key: &str, token: Option<&str>) -> Option<(Auth, Revoked)> {
    if super::remote::token_ok(api_key, token) {
        return Some((Auth::Key, Revoked { session: None, rx: None }));
    }
    let token = token.filter(|t| !t.is_empty())?;
    let phones = paired();
    let phone = find_phone(phones.as_deref().unwrap_or_default(), token)?;
    let session = NEXT_SESSION.fetch_add(1, Ordering::AcqRel) + 1;
    let (tx, rx) = oneshot::channel();
    lock(&SESSIONS).push((session, phone.phone_id.clone(), tx));
    Some((Auth::Phone(phone.phone_id.clone()), Revoked { session: Some(session), rx: Some(rx) }))
}

pub fn phone_name(phone_id: &str) -> Option<String> {
    paired().as_deref().unwrap_or_default().iter().find(|p| p.phone_id == phone_id).map(|p| p.name.clone())
}

/// Drops the token and cuts the phone's /ui, /audio and /phone sockets.
pub fn remove_phone(phone_id: &str) -> Result<bool, String> {
    let mut phones = paired();
    let removed = repo::remove_paired_phone(phone_id)?;
    if let Some(list) = phones.as_mut() {
        list.retain(|p| p.phone_id != phone_id);
    }
    // Dropping the senders resolves every Revoked of this phone.
    lock(&SESSIONS).retain(|(_, id, _)| id != phone_id);
    drop(phones);
    eprintln!("[pair] phone {} removed", phone_id);
    Ok(removed)
}

/// Stores a fresh token for the phone (replacing any earlier one); returns the token.
fn issue_token(phone_id: &str, name: &str) -> Result<String, String> {
    let token = hex(&random_bytes::<32>());
    let hash = token_hash(&token);
    let mut phones = paired();
    repo::save_paired_phone(&PairedPhoneRow {
        phone_id: phone_id.into(),
        name: name.into(),
        token_hash: hash.clone(),
        created_at: chrono::Local::now().to_rfc3339(),
        last_seen_at: None,
    })?;
    let list = phones.get_or_insert_with(Vec::new);
    list.retain(|p| p.phone_id != phone_id);
    list.push(Paired { phone_id: phone_id.into(), name: name.into(), hash: parse_hash(&hash).expect("own hash") });
    // Sockets on the replaced token go too.
    lock(&SESSIONS).retain(|(_, id, _)| id != phone_id);
    Ok(token)
}

// ── HTTP ─────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct StartRequest {
    phone_id: String,
    phone_name: String,
}

#[derive(Deserialize)]
pub struct ConfirmRequest {
    phone_id: String,
    code: String,
}

fn fail(status: StatusCode, error: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "ok": false, "error": error })))
}

fn changed(app: &AppHandle) {
    let _ = app.emit(PAIRING_CHANGED, ());
}

pub async fn pair_start(State(state): State<Arc<ServerState>>, Json(body): Json<StartRequest>) -> (StatusCode, Json<Value>) {
    let (Some(name), true) = (clean_name(&body.phone_name), valid_id(&body.phone_id)) else {
        return fail(StatusCode::BAD_REQUEST, "invalid phone_id or phone_name");
    };
    let started = lock(&PAIRING).start(&body.phone_id, &name, new_code(), Instant::now());
    match started {
        Err(StartError::Busy) => return fail(StatusCode::CONFLICT, "another phone is pairing"),
        Err(StartError::TooSoon) => return fail(StatusCode::TOO_MANY_REQUESTS, "try again in a few seconds"),
        Err(StartError::Locked) => {
            return fail(StatusCode::TOO_MANY_REQUESTS, "too many wrong codes; try again in 5 minutes")
        }
        Ok(()) => {}
    }
    eprintln!("[pair] {} asks to pair", name);
    let app = state.app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(e) = crate::commands::config::open_settings_window(app.clone()) {
            eprintln!("[pair] settings window: {e}");
        }
        changed(&app);
    });
    let (pa_id, pa_name) = identity();
    (StatusCode::OK, Json(json!({ "ok": true, "pa_id": pa_id, "pa_name": pa_name, "expires_in": CODE_TTL.as_secs() })))
}

pub async fn pair_confirm(State(state): State<Arc<ServerState>>, Json(body): Json<ConfirmRequest>) -> (StatusCode, Json<Value>) {
    let confirmed = lock(&PAIRING).confirm(&body.phone_id, &body.code, Instant::now());
    let name = match confirmed {
        Ok(name) => name,
        Err(ConfirmError::NoPairing) => return fail(StatusCode::GONE, "no pairing in progress"),
        Err(ConfirmError::WrongCode { attempts_left }) => {
            eprintln!("[pair] wrong code ({attempts_left} tries left)");
            if attempts_left == 0 {
                changed(&state.app);
            }
            return (StatusCode::FORBIDDEN, Json(json!({ "ok": false, "error": "wrong code", "attempts_left": attempts_left })));
        }
    };
    let issued = {
        let phone_id = body.phone_id.clone();
        let name = name.clone();
        tauri::async_runtime::spawn_blocking(move || issue_token(&phone_id, &name)).await
    };
    changed(&state.app);
    match issued.map_err(|e| e.to_string()).and_then(|r| r) {
        Ok(token) => {
            eprintln!("[pair] {} paired", name);
            let (pa_id, pa_name) = identity();
            (StatusCode::OK, Json(json!({ "ok": true, "pa_id": pa_id, "pa_name": pa_name, "token": token })))
        }
        Err(e) => {
            eprintln!("[pair] saving the pairing failed: {e}");
            fail(StatusCode::INTERNAL_SERVER_ERROR, "could not save the pairing")
        }
    }
}

// ── Settings window (desktop only: none of these are in remote::UI_COMMANDS) ─

#[derive(Serialize)]
pub struct PairingStatus {
    phone_name: String,
    code: String,
    expires_in: u64,
}

#[tauri::command]
pub fn pairing_status() -> Option<PairingStatus> {
    lock(&PAIRING)
        .status(Instant::now())
        .map(|(phone_name, code, expires_in)| PairingStatus { phone_name, code, expires_in })
}

#[tauri::command]
pub fn cancel_pairing(app: AppHandle) {
    lock(&PAIRING).cancel();
    changed(&app);
}

#[derive(Serialize)]
pub struct PhoneListing {
    #[serde(flatten)]
    row: PairedPhoneRow,
    online: bool,
}

#[tauri::command]
pub async fn list_paired_phones() -> Result<Vec<PhoneListing>, String> {
    let rows = tauri::async_runtime::spawn_blocking(repo::paired_phones).await.map_err(|e| e.to_string())??;
    let online: HashMap<String, bool> = super::phone::online_phones().into_iter().map(|id| (id, true)).collect();
    Ok(rows
        .into_iter()
        .map(|row| PhoneListing { online: online.contains_key(&row.phone_id), row })
        .collect())
}

#[tauri::command]
pub async fn remove_paired_phone(app: AppHandle, phone_id: String) -> Result<bool, String> {
    let removed = tauri::async_runtime::spawn_blocking(move || remove_phone(&phone_id))
        .await
        .map_err(|e| e.to_string())??;
    changed(&app);
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started(code: &str, now: Instant) -> Pairing {
        let mut p = Pairing::default();
        p.start("phone-a", "Pixel 2", code.into(), now).unwrap();
        p
    }

    #[test]
    fn the_right_code_pairs_once() {
        let now = Instant::now();
        let mut p = started("123456", now);
        assert_eq!(p.confirm("phone-a", "123456", now), Ok("Pixel 2".into()));
        assert_eq!(p.confirm("phone-a", "123456", now), Err(ConfirmError::NoPairing), "a code is single-use");
    }

    #[test]
    fn the_code_expires_after_two_minutes() {
        let now = Instant::now();
        let mut p = started("123456", now);
        assert!(p.status(now + CODE_TTL - Duration::from_secs(1)).is_some());
        assert_eq!(p.confirm("phone-a", "123456", now + CODE_TTL), Err(ConfirmError::NoPairing));
        assert!(p.status(now + CODE_TTL).is_none(), "the panel closes too");
    }

    #[test]
    fn five_wrong_codes_void_it_and_the_sixth_try_is_refused() {
        let now = Instant::now();
        let mut p = started("123456", now);
        for left in (0..MAX_ATTEMPTS).rev() {
            assert_eq!(p.confirm("phone-a", "000000", now), Err(ConfirmError::WrongCode { attempts_left: left }));
        }
        assert_eq!(p.confirm("phone-a", "123456", now), Err(ConfirmError::NoPairing), "even the right code: void");
        assert!(p.status(now).is_none());
    }

    #[test]
    fn a_voided_pairing_locks_new_ones_for_five_minutes() {
        let now = Instant::now();
        let mut p = started("123456", now);
        for _ in 0..MAX_ATTEMPTS {
            let _ = p.confirm("phone-a", "000000", now);
        }
        let later = now + START_COOLDOWN;
        assert_eq!(p.start("phone-a", "Pixel 2", "1".into(), later), Err(StartError::Locked), "the same phone too");
        assert_eq!(p.start("phone-b", "Other", "1".into(), later), Err(StartError::Locked));
        p.cancel();
        assert_eq!(p.start("phone-b", "Other", "1".into(), later), Err(StartError::Locked), "cancel does not unlock");
        assert_eq!(p.start("phone-b", "Other", "1".into(), now + LOCKOUT), Ok(()));
    }

    #[test]
    fn a_wrong_phone_id_counts_as_a_wrong_try() {
        let now = Instant::now();
        let mut p = started("123456", now);
        assert_eq!(p.confirm("phone-b", "123456", now), Err(ConfirmError::WrongCode { attempts_left: 4 }));
        assert_eq!(p.confirm("phone-a", "123456", now), Ok("Pixel 2".into()));
    }

    #[test]
    fn one_pairing_at_a_time_with_a_cooldown_between_starts() {
        let now = Instant::now();
        let mut p = started("123456", now);
        let later = now + START_COOLDOWN;
        assert_eq!(p.start("phone-b", "Other", "1".into(), later), Err(StartError::Busy));
        assert_eq!(p.start("phone-a", "Pixel 2", "654321".into(), now + Duration::from_secs(1)), Err(StartError::TooSoon));
        assert_eq!(p.start("phone-a", "Pixel 2", "654321".into(), later), Ok(()), "the same phone may retry");
        assert_eq!(p.confirm("phone-a", "123456", later), Err(ConfirmError::WrongCode { attempts_left: 4 }), "the old code is void");
        // Once it expires another phone may start.
        assert_eq!(p.start("phone-b", "Other", "1".into(), later + CODE_TTL), Ok(()));
    }

    #[test]
    fn codes_are_six_digits() {
        for _ in 0..50 {
            let c = new_code();
            assert_eq!(c.len(), 6);
            assert!(c.bytes().all(|b| b.is_ascii_digit()));
        }
    }

    #[test]
    fn ids_and_names_are_checked() {
        assert!(valid_id("a1-B_2") && !valid_id("") && !valid_id("a b") && !valid_id(&"a".repeat(65)));
        assert_eq!(clean_name("  Pixel\n2 "), Some("Pixel2".into()));
        assert_eq!(clean_name(" \u{7} "), None);
        assert_eq!(clean_name(&"长".repeat(80)).unwrap().chars().count(), NAME_MAX);
    }

    #[test]
    fn tokens_match_by_hash_only() {
        let token = hex(&random_bytes::<32>());
        assert_eq!(token.len(), 64);
        let hash = token_hash(&token);
        assert_ne!(hash, token);
        let phones = vec![
            Paired { phone_id: "a".into(), name: "A".into(), hash: parse_hash(&token_hash("other")).unwrap() },
            Paired { phone_id: "b".into(), name: "B".into(), hash: parse_hash(&hash).unwrap() },
        ];
        assert_eq!(find_phone(&phones, &token).map(|p| p.phone_id.as_str()), Some("b"));
        assert!(find_phone(&phones, &token[..63]).is_none());
        assert!(find_phone(&phones, &hash).is_none(), "the stored hash is not a token");
    }

    #[test]
    fn identity_is_a_v4_uuid() {
        let id = new_uuid();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
        assert_ne!(id, new_uuid());
    }

    /// Paired phone `id` with `token`, in the cache only (no settings.db).
    pub(crate) fn pair_for_test(id: &str, token: &str) {
        let mut phones = paired_cache_for_test();
        let list = phones.get_or_insert_with(Vec::new);
        list.retain(|p| p.phone_id != id);
        list.push(Paired { phone_id: id.into(), name: id.into(), hash: parse_hash(&token_hash(token)).unwrap() });
    }

    fn paired_cache_for_test() -> MutexGuard<'static, Option<Vec<Paired>>> {
        let mut guard = lock(&PAIRED);
        guard.get_or_insert_with(Vec::new);
        guard
    }

    #[test]
    fn key_and_device_tokens_both_authenticate_and_removal_cuts_the_socket() {
        pair_for_test("auth-phone", "device-token-1");
        assert_eq!(authenticate("the-key", Some("the-key")).map(|a| a.0), Some(Auth::Key), "the old key still works");
        assert!(authenticate("the-key", Some("nope")).is_none());
        assert!(authenticate("", None).is_none());
        let (auth, mut revoked) = authenticate("the-key", Some("device-token-1")).unwrap();
        assert_eq!(auth, Auth::Phone("auth-phone".into()));

        // Removing the phone (cache only: what remove_phone does after the DB delete).
        lock(&PAIRED).as_mut().unwrap().retain(|p| p.phone_id != "auth-phone");
        lock(&SESSIONS).retain(|(_, id, _)| id != "auth-phone");
        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(1), revoked.wait()).await.expect("the socket is cut");
        });
        assert!(authenticate("the-key", Some("device-token-1")).is_none(), "the old token is refused");
    }

    #[test]
    fn the_key_never_gets_revoked() {
        let (_, mut revoked) = authenticate("k", Some("k")).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        rt.block_on(async {
            assert!(tokio::time::timeout(Duration::from_millis(50), revoked.wait()).await.is_err());
        });
    }

    #[test]
    fn no_http_reply_or_log_line_carries_the_code_or_token() {
        // The handlers' JSON bodies and eprintln! lines name neither field.
        let src = include_str!("pairing.rs");
        let handlers = &src[src.find("// ── HTTP").unwrap()..src.find("// ── Settings window").unwrap()];
        for line in handlers.lines().filter(|l| l.contains("eprintln!")) {
            assert!(!line.contains("code") || line.contains("wrong code"), "{line}");
            assert!(!line.contains("token"), "{line}");
        }
        let start = &handlers[handlers.find("pub async fn pair_start").unwrap()..handlers.find("pub async fn pair_confirm").unwrap()];
        assert!(!start.contains("\"code\""), "/pair/start never returns the code");
        let confirm = &handlers[handlers.find("pub async fn pair_confirm").unwrap()..];
        assert!(!confirm.contains("\"code\""), "/pair/confirm never echoes the code");
    }
}
