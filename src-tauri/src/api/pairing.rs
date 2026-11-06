// pairing.rs — Pairing phones with this PA by QR code (FEAT-MP1 / SEC-QR1 / FEAT-QR1)
//
//   settings → 设备 → 添加手机: start_pairing shows
//     pocketagent://pair?v=1&id=<pa_id>&s=<secret>&h=<ip:port>,…
//   GET  /pair/hello                                  → {pa_id, pa_name, nonce}
//   POST /pair/confirm {phone_id, phone_name, nonce, proof}
//        proof        = HMAC-SHA256(secret, "phone|" + nonce + "|" + phone_id)
//        → {pa_id, pa_name, token, server_proof = HMAC-SHA256(secret, "pa|" + nonce + "|" + phone_id)}
//
// Both endpoints are open (the phone has no credentials yet); what guards them:
//   - the secret (32 random bytes) only leaves this computer on its screen;
//     the network sees nonces and HMACs, never the secret itself;
//   - it lasts 2 minutes, pairs one phone once, and a new QR code voids the old;
//   - without a QR code on screen /pair/hello answers 410 and says nothing;
//   - a nonce is single-use (at most 8 outstanding); 5 wrong proofs void the
//     code; proofs are compared in constant time, all under one lock;
//   - server_proof lets the phone check it reached the PA that showed the code;
//   - the token is 32 random bytes; settings.db keeps only its SHA-256.
// Removing a phone drops its token and cuts every socket it opened with it.
// Neither secrets, proofs nor tokens are ever logged.

use std::collections::{HashMap, VecDeque};
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use axum::{extract::State, http::StatusCode, Json};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;

use super::server::ServerState;
use crate::commands::settings_repository::{self as repo, PairedPhoneRow};

pub const QR_TTL: Duration = Duration::from_secs(120);
pub const MAX_WRONG: u32 = 5;
/// Outstanding /pair/hello nonces per QR code; older ones drop off.
pub const MAX_NONCES: usize = 8;
const ID_MAX: usize = 64;
const NAME_MAX: usize = 64;

/// Settings window: the paired list or who is online changed (no payload).
pub const PAIRING_CHANGED: &str = "pairing-changed";
/// Settings window: the QR code is no longer valid — paired, voided by wrong
/// proofs, or cancelled (no payload). Only this takes the QR code down.
pub const PAIRING_ENDED: &str = "pairing-ended";

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

/// RFC 2104 over SHA-256 (the sha2 crate is already here; no hmac crate).
fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    if key.len() > block.len() {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let inner = Sha256::new().chain_update(block.map(|b| b ^ 0x36)).chain_update(msg).finalize();
    Sha256::new().chain_update(block.map(|b| b ^ 0x5c)).chain_update(inner).finalize().into()
}

/// `side` is "phone" or "pa".
fn proof(secret: &[u8], side: &str, nonce: &str, phone_id: &str) -> [u8; 32] {
    hmac_sha256(secret, format!("{side}|{nonce}|{phone_id}").as_bytes())
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

// ── The QR code on screen ────────────────────────────────────────────────────

struct Pending {
    secret: [u8; 32],
    expires: Instant,
    nonces: VecDeque<String>,
    wrong: u32,
}

#[derive(Default)]
pub(crate) struct Pairing {
    pending: Option<Pending>,
    /// Bumped by every new QR code, so the one on screen (if any) is always
    /// this generation. pairing-ended carries it: a late event from an older
    /// code must not take down a newer one.
    generation: u64,
}

#[derive(Debug, PartialEq)]
pub(crate) enum ConfirmError {
    /// No QR code on screen: never shown, expired, used, cancelled, or voided.
    NoPairing,
    /// Not a nonce this QR code handed out, or one already spent.
    UnknownNonce,
    WrongProof { tries_left: u32 },
}

impl Pairing {
    fn live(&mut self, now: Instant) -> Option<&mut Pending> {
        if self.pending.as_ref().is_some_and(|p| now >= p.expires) {
            self.pending = None;
        }
        self.pending.as_mut()
    }

    /// A new QR code; whatever was on screen before is void. Returns its generation.
    pub(crate) fn begin(&mut self, secret: [u8; 32], now: Instant) -> u64 {
        self.generation += 1;
        self.pending = Some(Pending { secret, expires: now + QR_TTL, nonces: VecDeque::new(), wrong: 0 });
        self.generation
    }

    /// Hands out `nonce` if a QR code is on screen.
    pub(crate) fn hello(&mut self, nonce: String, now: Instant) -> bool {
        let Some(p) = self.live(now) else { return false };
        if p.nonces.len() == MAX_NONCES {
            p.nonces.pop_front();
        }
        p.nonces.push_back(nonce);
        true
    }

    /// The nonce is spent whatever the outcome. Ok(server_proof); the QR code
    /// is used up.
    pub(crate) fn confirm(&mut self, phone_id: &str, nonce: &str, claimed: &[u8], now: Instant) -> Result<[u8; 32], ConfirmError> {
        let Some(p) = self.live(now) else { return Err(ConfirmError::NoPairing) };
        let Some(at) = p.nonces.iter().position(|n| n == nonce) else { return Err(ConfirmError::UnknownNonce) };
        p.nonces.remove(at);
        if !same_bytes(&proof(&p.secret, "phone", nonce, phone_id), claimed) {
            p.wrong += 1;
            let left = MAX_WRONG.saturating_sub(p.wrong);
            if left == 0 {
                self.pending = None;
            }
            return Err(ConfirmError::WrongProof { tries_left: left });
        }
        let server = proof(&p.secret, "pa", nonce, phone_id);
        self.pending = None;
        Ok(server)
    }

    /// Takes QR code `generation` off screen; false if it is gone already.
    fn cancel(&mut self, generation: u64) -> bool {
        if generation != self.generation || self.pending.is_none() {
            return false;
        }
        self.pending = None;
        true
    }
}

static PAIRING: Mutex<Pairing> = Mutex::new(Pairing { pending: None, generation: 0 });

/// The port the API server listens on (0 until it is up).
static PORT: AtomicU16 = AtomicU16::new(0);

pub fn set_port(port: u16) {
    PORT.store(port, Ordering::Release);
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

/// 10/8, 172.16/12 and 192.168/16, each once, in the order given.
fn lan_addresses(ips: impl IntoIterator<Item = Ipv4Addr>) -> Vec<Ipv4Addr> {
    let mut out: Vec<Ipv4Addr> = Vec::new();
    for ip in ips {
        if ip.is_private() && !out.contains(&ip) {
            out.push(ip);
        }
    }
    out
}

fn local_lan_addresses() -> Vec<Ipv4Addr> {
    let ips = if_addrs::get_if_addrs().unwrap_or_else(|e| {
        eprintln!("[pair] listing network interfaces failed: {e}");
        Vec::new()
    });
    lan_addresses(ips.into_iter().filter(|i| !i.is_loopback()).filter_map(|i| match i.ip() {
        std::net::IpAddr::V4(ip) => Some(ip),
        _ => None,
    }))
}

fn pair_uri(pa_id: &str, secret: &[u8], hosts: &[Ipv4Addr], port: u16) -> String {
    let hosts: Vec<String> = hosts.iter().map(|ip| format!("{ip}:{port}")).collect();
    format!("pocketagent://pair?v=1&id={pa_id}&s={}&h={}", URL_SAFE_NO_PAD.encode(secret), hosts.join(","))
}

/// Dark modules as one path, 4-module quiet zone, scales to its box.
fn qr_svg(text: &str) -> Result<String, String> {
    use qrcodegen::{QrCode, QrCodeEcc};
    let qr = QrCode::encode_text(text, QrCodeEcc::Medium).map_err(|e| e.to_string())?;
    let (n, border) = (qr.size(), 4);
    let mut path = String::new();
    for y in 0..n {
        for x in 0..n {
            if qr.get_module(x, y) {
                path.push_str(&format!("M{},{}h1v1h-1z", x + border, y + border));
            }
        }
    }
    let side = n + border * 2;
    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {side} {side}\" shape-rendering=\"crispEdges\">\
         <rect width=\"100%\" height=\"100%\" fill=\"#fff\"/><path d=\"{path}\" fill=\"#000\"/></svg>"
    ))
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
pub struct ConfirmRequest {
    phone_id: String,
    phone_name: String,
    nonce: String,
    proof: String,
}

fn fail(status: StatusCode, error: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "ok": false, "error": error })))
}

fn changed(app: &AppHandle) {
    let _ = app.emit(PAIRING_CHANGED, ());
}

fn ended(app: &AppHandle, generation: u64) {
    let _ = app.emit(PAIRING_ENDED, json!({ "generation": generation }));
}

const NO_QR: &str = "no pairing QR code on screen";

pub async fn pair_hello() -> (StatusCode, Json<Value>) {
    let nonce = URL_SAFE_NO_PAD.encode(random_bytes::<16>());
    if !lock(&PAIRING).hello(nonce.clone(), Instant::now()) {
        return fail(StatusCode::GONE, NO_QR);
    }
    let (pa_id, pa_name) = identity();
    (StatusCode::OK, Json(json!({ "ok": true, "v": 1, "pa_id": pa_id, "pa_name": pa_name, "nonce": nonce })))
}

pub async fn pair_confirm(State(state): State<Arc<ServerState>>, Json(body): Json<ConfirmRequest>) -> (StatusCode, Json<Value>) {
    let (Some(name), true) = (clean_name(&body.phone_name), valid_id(&body.phone_id)) else {
        return fail(StatusCode::BAD_REQUEST, "invalid phone_id or phone_name");
    };
    // Undecodable counts as a wrong proof, like any other.
    let claimed = URL_SAFE_NO_PAD.decode(body.proof.as_bytes()).unwrap_or_default();
    let (generation, confirmed) = {
        let mut pairing = lock(&PAIRING);
        (pairing.generation, pairing.confirm(&body.phone_id, &body.nonce, &claimed, Instant::now()))
    };
    let server_proof = match confirmed {
        Ok(server_proof) => server_proof,
        Err(ConfirmError::NoPairing) => return fail(StatusCode::GONE, NO_QR),
        Err(ConfirmError::UnknownNonce) => return fail(StatusCode::FORBIDDEN, "unknown or used nonce"),
        Err(ConfirmError::WrongProof { tries_left }) => {
            eprintln!("[pair] wrong proof ({tries_left} tries left)");
            if tries_left == 0 {
                ended(&state.app, generation);
            }
            return (StatusCode::FORBIDDEN, Json(json!({ "ok": false, "error": "wrong proof", "tries_left": tries_left })));
        }
    };
    let issued = {
        let phone_id = body.phone_id.clone();
        let name = name.clone();
        tauri::async_runtime::spawn_blocking(move || issue_token(&phone_id, &name)).await
    };
    // By now the settings window may show a newer QR code: the generation spares it.
    ended(&state.app, generation);
    changed(&state.app);
    match issued.map_err(|e| e.to_string()).and_then(|r| r) {
        Ok(token) => {
            eprintln!("[pair] {} paired", name);
            let (pa_id, pa_name) = identity();
            let server_proof = URL_SAFE_NO_PAD.encode(server_proof);
            (StatusCode::OK, Json(json!({ "ok": true, "pa_id": pa_id, "pa_name": pa_name, "token": token, "server_proof": server_proof })))
        }
        Err(e) => {
            eprintln!("[pair] saving the pairing failed: {e}");
            fail(StatusCode::INTERNAL_SERVER_ERROR, "could not save the pairing")
        }
    }
}

// ── Settings window (desktop only: none of these are in remote::UI_COMMANDS) ─

#[derive(Serialize)]
pub struct PairingQr {
    svg: String,
    expires_in: u64,
    generation: u64,
}

/// A fresh QR code (the previous one is void).
#[tauri::command]
pub fn start_pairing() -> Result<PairingQr, String> {
    let port = PORT.load(Ordering::Acquire);
    if port == 0 {
        return Err("api server not running".into());
    }
    let hosts = local_lan_addresses();
    if hosts.is_empty() {
        return Err("no LAN address".into());
    }
    let secret = random_bytes::<32>();
    let svg = qr_svg(&pair_uri(&identity().0, &secret, &hosts, port))?;
    let generation = lock(&PAIRING).begin(secret, Instant::now());
    Ok(PairingQr { svg, expires_in: QR_TTL.as_secs(), generation })
}

#[tauri::command]
pub fn cancel_pairing(app: AppHandle, generation: u64) {
    if lock(&PAIRING).cancel(generation) {
        ended(&app, generation);
    }
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

    const SECRET: [u8; 32] = [7u8; 32];

    fn shown(now: Instant) -> Pairing {
        let mut p = Pairing::default();
        p.begin(SECRET, now);
        p
    }

    fn hello(p: &mut Pairing, nonce: &str, now: Instant) {
        assert!(p.hello(nonce.into(), now), "a QR code is on screen");
    }

    fn phone_proof(nonce: &str, phone_id: &str) -> [u8; 32] {
        proof(&SECRET, "phone", nonce, phone_id)
    }

    #[test]
    fn hmac_matches_rfc_4231() {
        // Test cases 1, 2 and 6 (key longer than the block).
        let hx = |b: &[u8]| hex(b);
        assert_eq!(hx(&hmac_sha256(&[0x0b; 20], b"Hi There")), "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
        assert_eq!(hx(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        assert_eq!(
            hx(&hmac_sha256(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First")),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn the_right_proof_pairs_once_and_proves_the_pa_back() {
        let now = Instant::now();
        let mut p = shown(now);
        hello(&mut p, "n1", now);
        hello(&mut p, "n2", now);
        let server = p.confirm("phone-a", "n1", &phone_proof("n1", "phone-a"), now).unwrap();
        assert_eq!(server, proof(&SECRET, "pa", "n1", "phone-a"));
        assert_ne!(server, phone_proof("n1", "phone-a"), "the two directions differ");
        assert_eq!(p.confirm("phone-a", "n2", &phone_proof("n2", "phone-a"), now), Err(ConfirmError::NoPairing), "single-use");
        assert!(!p.hello("n3".into(), now), "and /pair/hello is gone again");
    }

    #[test]
    fn no_qr_code_no_hello() {
        let now = Instant::now();
        let mut p = Pairing::default();
        assert!(!p.hello("n".into(), now));
        assert_eq!(p.confirm("phone-a", "n", &phone_proof("n", "phone-a"), now), Err(ConfirmError::NoPairing));
    }

    #[test]
    fn the_qr_code_expires_after_two_minutes() {
        let now = Instant::now();
        let mut p = shown(now);
        hello(&mut p, "n1", now + QR_TTL - Duration::from_secs(1));
        assert_eq!(p.confirm("phone-a", "n1", &phone_proof("n1", "phone-a"), now + QR_TTL), Err(ConfirmError::NoPairing));
        assert!(!p.hello("n2".into(), now + QR_TTL));
    }

    #[test]
    fn a_new_qr_code_voids_the_old_one() {
        let now = Instant::now();
        let mut p = shown(now);
        hello(&mut p, "n1", now);
        p.begin([9u8; 32], now);
        assert_eq!(p.confirm("phone-a", "n1", &phone_proof("n1", "phone-a"), now), Err(ConfirmError::UnknownNonce), "its nonces go too");
        hello(&mut p, "n2", now);
        assert_eq!(
            p.confirm("phone-a", "n2", &phone_proof("n2", "phone-a"), now),
            Err(ConfirmError::WrongProof { tries_left: MAX_WRONG - 1 }),
            "the old secret no longer proves anything"
        );
        p.cancel(p.generation);
        assert!(!p.hello("n3".into(), now), "cancel takes it off screen");
    }

    #[test]
    fn a_late_end_of_an_old_qr_code_spares_the_new_one() {
        // pair_confirm reads the generation in the lock, then waits for
        // issue_token; meanwhile the user cancels and shows a new code.
        let now = Instant::now();
        let mut p = Pairing::default();
        let old = p.begin(SECRET, now);
        hello(&mut p, "n1", now);
        let (ended_gen, confirmed) = (p.generation, p.confirm("phone-a", "n1", &phone_proof("n1", "phone-a"), now));
        assert!(confirmed.is_ok());
        assert_eq!(ended_gen, old);
        assert!(!p.cancel(old), "already used: nothing to cancel, no second end");
        let new = p.begin([9u8; 32], now);
        assert!(new > old, "generations only go up");
        // The old confirm resumes and emits pairing-ended { generation: old }:
        assert_ne!(ended_gen, new, "the settings window keeps the new code");
        assert!(!p.cancel(old), "a late cancel of the old code leaves the new one alone");
        hello(&mut p, "n2", now); // and /pair/hello still answers 200
        assert!(p.cancel(new));
        assert!(!p.hello("n3".into(), now));
    }

    #[test]
    fn nonces_are_single_use_and_at_most_eight() {
        let now = Instant::now();
        let mut p = shown(now);
        for i in 0..=MAX_NONCES {
            hello(&mut p, &format!("n{i}"), now);
        }
        assert_eq!(p.confirm("phone-a", "n0", &phone_proof("n0", "phone-a"), now), Err(ConfirmError::UnknownNonce), "the oldest dropped off");
        assert_eq!(p.confirm("phone-a", "n1", b"wrong", now), Err(ConfirmError::WrongProof { tries_left: 4 }));
        assert_eq!(p.confirm("phone-a", "n1", &phone_proof("n1", "phone-a"), now), Err(ConfirmError::UnknownNonce), "spent even when wrong");
        assert_eq!(p.confirm("phone-a", "never", &phone_proof("never", "phone-a"), now), Err(ConfirmError::UnknownNonce));
        assert!(p.confirm("phone-a", "n8", &phone_proof("n8", "phone-a"), now).is_ok());
    }

    #[test]
    fn a_proof_is_bound_to_its_nonce_and_phone_id() {
        let now = Instant::now();
        let mut p = shown(now);
        hello(&mut p, "n1", now);
        hello(&mut p, "n2", now);
        assert_eq!(p.confirm("phone-b", "n1", &phone_proof("n1", "phone-a"), now), Err(ConfirmError::WrongProof { tries_left: 4 }));
        assert_eq!(p.confirm("phone-a", "n2", &phone_proof("n1", "phone-a"), now), Err(ConfirmError::WrongProof { tries_left: 3 }));
    }

    #[test]
    fn five_wrong_proofs_void_the_qr_code() {
        let now = Instant::now();
        let mut p = shown(now);
        for left in (0..MAX_WRONG).rev() {
            let n = format!("n{left}");
            hello(&mut p, &n, now);
            assert_eq!(p.confirm("phone-a", &n, &[0u8; 32], now), Err(ConfirmError::WrongProof { tries_left: left }));
        }
        assert!(!p.hello("n".into(), now), "void: even the right proof can no longer get a nonce");
    }

    #[test]
    fn only_private_ipv4_addresses_go_into_the_qr_code() {
        let ips = ["192.168.1.20", "127.0.0.1", "10.0.0.5", "172.16.3.4", "172.32.0.1", "169.254.1.1", "8.8.8.8", "192.168.1.20"];
        let got = lan_addresses(ips.iter().map(|s| s.parse().unwrap()));
        let want: Vec<Ipv4Addr> = ["192.168.1.20", "10.0.0.5", "172.16.3.4"].iter().map(|s| s.parse().unwrap()).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn the_uri_carries_id_secret_and_every_address() {
        let hosts: Vec<Ipv4Addr> = vec!["192.168.1.20".parse().unwrap(), "10.0.0.5".parse().unwrap()];
        let uri = pair_uri("0f3a9c21-1111-4222-8333-444455556666", &[0xffu8; 32], &hosts, 8651);
        assert_eq!(
            uri,
            format!(
                "pocketagent://pair?v=1&id=0f3a9c21-1111-4222-8333-444455556666&s={}&h=192.168.1.20:8651,10.0.0.5:8651",
                "_".repeat(42) + "8"
            )
        );
        let svg = qr_svg(&uri).unwrap();
        assert!(svg.starts_with("<svg") && svg.contains("<path d=\"M"));
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
    fn no_http_reply_or_log_line_carries_a_secret() {
        // Log lines and error texts name no secret, proof or token value.
        let src = include_str!("pairing.rs");
        let code = &src[..src.find("#[cfg(test)]\nmod tests").unwrap()];
        for line in code.lines().filter(|l| l.contains("eprintln!") || l.contains("Err(format!")) {
            for word in ["secret", "token}", "proof}", "nonce}", "uri", "svg"] {
                assert!(!line.contains(word), "{line}");
            }
        }
        let hello = &code[code.find("pub async fn pair_hello").unwrap()..code.find("pub async fn pair_confirm").unwrap()];
        assert!(!hello.contains("secret"), "/pair/hello never touches the secret");
        assert!(!code.contains("\"secret\""), "no reply carries the secret");
    }
}
