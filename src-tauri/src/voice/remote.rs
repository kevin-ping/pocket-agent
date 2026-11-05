// remote.rs — The phone as microphone and speaker (FEAT-DEV1)
//
// api/remote.rs owns the /audio WebSocket; voice code only talks to this module.
// While a phone is attached:
//   - record.rs routes streaming captures (wake / conversation) to the PCM the
//     phone uploads (16 kHz mono i16) instead of opening cpal;
//   - the chat.rs audio player hands each sentence to `play_sentence` instead of
//     rodio; the phone's playing / done reports stand in for rodio's.
// Every source switch bumps SOURCE_EPOCH. A capture started on the old source
// sees itself stale and its worker exits through its normal end path.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter};

use crate::voice::record::{current_owner, lock_capture_gate, OWNER_NONE};

pub const REMOTE_SAMPLE_RATE: u32 = 16_000;
/// The phone decodes and starts a sentence well within this.
const START_TIMEOUT: Duration = Duration::from_secs(10);
/// Upper bound for one sentence; a phone that never reports done can't wedge the queue.
const PLAY_TIMEOUT: Duration = Duration::from_secs(120);

static ONLINE: AtomicBool = AtomicBool::new(false);
static SOURCE_EPOCH: AtomicU64 = AtomicU64::new(0);

pub fn is_online() -> bool {
    ONLINE.load(Ordering::Acquire)
}

pub fn source_epoch() -> u64 {
    SOURCE_EPOCH.load(Ordering::Acquire)
}

/// The phone is parked (FEAT-DEV1): PA moved to the desktop while the phone
/// stays connected. A parked phone is neither the driver nor the audio source,
/// and reconnecting does not unpark it. Memory only: a PA restart clears it.
static PARKED: AtomicBool = AtomicBool::new(false);

pub fn is_parked() -> bool {
    PARKED.load(Ordering::Acquire)
}

/// Callers converge afterwards (api/remote.rs: sync_driver + sync_source).
pub fn set_parked(parked: bool) {
    PARKED.store(parked, Ordering::Release);
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

// ── Capture ──────────────────────────────────────────────────────────────────

type Callback = Box<dyn FnMut(&[i16], u32) + Send>;

static CAPTURE: Mutex<Option<(u64, Callback)>> = Mutex::new(None);
static NEXT_CAPTURE_ID: AtomicU64 = AtomicU64::new(0);

/// CAPTURE_OWNER already admits one streaming owner at a time, so attaching
/// simply takes the slot.
pub(crate) fn attach_capture(callback: Callback) -> u64 {
    let id = NEXT_CAPTURE_ID.fetch_add(1, Ordering::AcqRel) + 1;
    *lock(&CAPTURE) = Some((id, callback));
    id
}

pub(crate) fn detach_capture(id: u64) {
    let mut slot = lock(&CAPTURE);
    if slot.as_ref().map(|(current, _)| *current == id).unwrap_or(false) {
        *slot = None;
    }
}

/// PCM16 little-endian mono at 16 kHz from phone `conn`. Dropped when no
/// capture is attached — the phone streams continuously, PA listens only when
/// asked. Returns false once `conn` has been replaced: the check and the write
/// share the OUTBOUND lock that attach_phone takes, so after a new phone
/// attaches not one more frame from the old one reaches a capture.
pub fn push_pcm(conn: u64, bytes: &[u8]) -> bool {
    let outbound = lock(&OUTBOUND);
    if !is_conn(&outbound, conn) {
        return false;
    }
    if is_parked() {
        return true; // a parked phone stays connected but is not listened to
    }
    let samples = decode_pcm16(bytes);
    if samples.is_empty() {
        return true;
    }
    if let Some((_, callback)) = lock(&CAPTURE).as_mut() {
        crate::voice::record::update_level_i16(&samples);
        callback(&samples, REMOTE_SAMPLE_RATE);
    }
    true
}

fn decode_pcm16(bytes: &[u8]) -> Vec<i16> {
    bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect()
}

// ── Playback ─────────────────────────────────────────────────────────────────

pub enum Outbound {
    Play { seq: u64, audio: Vec<u8> },
    Clear,
    Snapshot { id: u64 },
    Wake,
}

type Phone = Option<(u64, tokio::sync::mpsc::UnboundedSender<Outbound>)>;

/// Lock order: PLAYBACK, then OUTBOUND, then CAPTURE / CAMERA / SNAPSHOTS.
static OUTBOUND: Mutex<Phone> = Mutex::new(None);
static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(0);

struct Pending {
    seq: u64,
    played: bool,
    finished: bool,
}

static PLAYBACK: Mutex<Option<Pending>> = Mutex::new(None);
static PLAYBACK_CHANGED: Condvar = Condvar::new();
static NEXT_SEQ: AtomicU64 = AtomicU64::new(0);

fn abort_pending() {
    *lock(&PLAYBACK) = None;
    PLAYBACK_CHANGED.notify_all();
}

/// A new phone replaces the old one (a phone reconnecting after Wi-Fi loss
/// must not wait for the dead socket to time out). Returns the connection id.
pub fn attach_phone(tx: tokio::sync::mpsc::UnboundedSender<Outbound>) -> u64 {
    let id = NEXT_CONN_ID.fetch_add(1, Ordering::AcqRel) + 1;
    let mut slot = lock(&OUTBOUND);
    *slot = Some((id, tx));
    forget_camera();
    drop(slot);
    abort_pending();
    id
}

/// Returns true when `id` was still the attached phone.
pub fn detach_phone(id: u64) -> bool {
    let mut slot = lock(&OUTBOUND);
    if slot.as_ref().map(|(current, _)| *current == id).unwrap_or(false) {
        *slot = None;
        forget_camera();
        drop(slot);
        abort_pending();
        return true;
    }
    false
}

fn is_conn(phone: &Phone, conn: u64) -> bool {
    phone.as_ref().map(|(current, _)| *current == conn).unwrap_or(false)
}

pub fn phone_attached() -> bool {
    lock(&OUTBOUND).is_some()
}

/// Send one sentence to the phone and block until it has played. `still_current`
/// is checked under the playback lock that `clear` also takes, so a stopped
/// generation can never start after the stop. Returns whether the phone
/// reported it playing (the caller then emits the sentence end).
pub fn play_sentence(audio: Vec<u8>, still_current: impl Fn() -> bool, on_playing: impl FnOnce()) -> bool {
    let seq = NEXT_SEQ.fetch_add(1, Ordering::AcqRel) + 1;
    {
        let mut pending = lock(&PLAYBACK);
        if !still_current() {
            return false;
        }
        let sent = lock(&OUTBOUND)
            .as_ref()
            .map(|(_, tx)| tx.send(Outbound::Play { seq, audio }).is_ok())
            .unwrap_or(false);
        if !sent {
            return false;
        }
        *pending = Some(Pending { seq, played: false, finished: false });
    }
    let started = wait_for_phone(seq, on_playing);
    eprintln!("[remote] sentence {} → phone (played={})", seq, started);
    started
}

fn wait_for_phone(seq: u64, on_playing: impl FnOnce()) -> bool {
    let mut on_playing = Some(on_playing);
    let mut started = false;
    let mut deadline = Instant::now() + START_TIMEOUT;
    let mut pending = lock(&PLAYBACK);
    loop {
        let (played, finished) = match pending.as_ref() {
            Some(p) if p.seq == seq => (p.played, p.finished),
            // Cleared, phone gone, or replaced: this sentence is over.
            _ => return started,
        };
        if played && !started {
            started = true;
            deadline = Instant::now() + PLAY_TIMEOUT;
            drop(pending);
            if let Some(f) = on_playing.take() {
                f();
            }
            pending = lock(&PLAYBACK);
            continue;
        }
        if finished {
            *pending = None;
            return started;
        }
        let now = Instant::now();
        if now >= deadline {
            eprintln!("[remote] sentence {} timed out (played={})", seq, played);
            *pending = None;
            return started;
        }
        pending = PLAYBACK_CHANGED
            .wait_timeout(pending, deadline - now)
            .unwrap_or_else(|p| p.into_inner())
            .0;
    }
}

/// Phone `conn`'s report for sentence `seq`. Reports for anything but the
/// sentence being waited on (an older generation) are ignored. Returns false
/// once `conn` has been replaced.
pub fn on_report(conn: u64, kind: &str, seq: u64) -> bool {
    let mut pending = lock(&PLAYBACK);
    if !is_conn(&lock(&OUTBOUND), conn) {
        return false;
    }
    if is_parked() {
        return true;
    }
    if let Some(p) = pending.as_mut().filter(|p| p.seq == seq) {
        match kind {
            "playing" => p.played = true,
            "done" => p.finished = true,
            _ => return true,
        }
        drop(pending);
        PLAYBACK_CHANGED.notify_all();
    }
    true
}

/// Called by stop_audio_queue after the generation bump: the phone drops its
/// queue and the waiting player thread is released.
pub fn clear() {
    let mut pending = lock(&PLAYBACK);
    if let Some((_, tx)) = lock(&OUTBOUND).as_ref() {
        let _ = tx.send(Outbound::Clear);
    }
    *pending = None;
    drop(pending);
    PLAYBACK_CHANGED.notify_all();
}

/// The owner's wake word was heard (BUG-DEV3): the phone turns its screen on
/// and brings the page forward, since Chromium freezes a hidden page and the
/// conversation logic lives there. Only when the phone is the audio source,
/// i.e. it heard the word; a desktop-driven PA never lights the phone.
pub fn wake_phone() -> bool {
    let outbound = lock(&OUTBOUND);
    if !is_online() {
        return false;
    }
    outbound.as_ref().map(|(_, tx)| tx.send(Outbound::Wake).is_ok()).unwrap_or(false)
}

// ── Camera snapshot (FEAT-CAM1) ──────────────────────────────────────────────
//
// The phone reports `{"type":"camera","on":bool,"facing":"front"|"back","switching":bool}`
// when the camera opens, closes or starts / finishes switching lens (`facing`
// is the lens bound, or being bound while `switching`); Hermes asks for a
// frame through /mcp and the phone answers the
// matching `{"type":"snapshot","id":N}` with base64 JPEG (or none when it
// can't). Frames live only in memory.

pub const CAMERA_OFF: &str = "用户的手机相机没有打开(phone_camera_open 可以打开)";
pub const SNAPSHOT_TIMED_OUT: &str = "手机没有在 5 秒内返回画面";
pub const SNAPSHOT_INVALID: &str = "手机返回的画面无法识别";
pub const CAMERA_OPEN_TIMED_OUT: &str = "等待相机打开超时(可能在等用户授权)";
pub const CAMERA_SWITCHING: &str = "手机相机正在切换镜头，稍后再取画面";
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(5);
/// The phone scales to about 1280 px on the long edge, far below this.
const MAX_SNAPSHOT_B64: usize = 4 * 1024 * 1024;

type SnapshotReply = tokio::sync::oneshot::Sender<Option<String>>;

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Facing {
    Front,
    Back,
}

/// An open camera as the phone reported it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// The lens bound (or being bound while `switching`); None = not reported.
    pub facing: Option<Facing>,
    /// Rebinding to `facing`: no frame until it is done.
    pub switching: bool,
}

impl Camera {
    /// Open with `facing` bound and serving frames.
    pub fn ready(facing: Facing) -> Self {
        Camera { facing: Some(facing), switching: false }
    }
}

/// Connection whose camera is open, and its state.
static CAMERA: Mutex<Option<(u64, Camera)>> = Mutex::new(None);
/// (request id, connection, reply) for requests still waiting on the phone.
static SNAPSHOTS: Mutex<Vec<(u64, u64, SnapshotReply)>> = Mutex::new(Vec::new());
static NEXT_SNAPSHOT_ID: AtomicU64 = AtomicU64::new(0);
/// Woken whenever the camera state may have changed (wait_camera).
static CAMERA_CHANGED: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Caller holds OUTBOUND. Waiting requests see their reply dropped → camera off.
fn forget_camera() {
    *lock(&CAMERA) = None;
    lock(&SNAPSHOTS).clear();
    CAMERA_CHANGED.notify_waiters();
}

/// The current phone's open camera (None = closed or no phone).
pub fn camera() -> Option<Camera> {
    let outbound = lock(&OUTBOUND);
    let camera = *lock(&CAMERA);
    camera.filter(|(conn, _)| is_conn(&outbound, *conn)).map(|(_, state)| state)
}

pub fn is_camera_on() -> bool {
    camera().is_some()
}

/// Phone `conn` reported its camera (None = closed). Returns false once `conn` has been replaced.
pub fn on_camera(conn: u64, state: Option<Camera>) -> bool {
    let outbound = lock(&OUTBOUND);
    if !is_conn(&outbound, conn) {
        return false;
    }
    match state {
        Some(state) => {
            *lock(&CAMERA) = Some((conn, state));
            CAMERA_CHANGED.notify_waiters();
        }
        None => forget_camera(),
    }
    true
}

/// Wait until the current phone's camera state satisfies `done` (phone_camera_open:
/// the page opens or flips it, the first time maybe behind the permission
/// dialog). Returns the state it ended on, satisfied or at the timeout.
pub async fn wait_camera(done: impl Fn(Option<Camera>) -> bool, timeout: Duration) -> Option<Camera> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        // Registered before the check, so a report landing in between still wakes us.
        let changed = CAMERA_CHANGED.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        let state = camera();
        if done(state) {
            return state;
        }
        if tokio::time::timeout_at(deadline, changed).await.is_err() {
            return camera();
        }
    }
}

/// Phone `conn`'s answer to request `id`. A reply from another connection never
/// completes a request. Returns false once `conn` has been replaced.
pub fn on_snapshot(conn: u64, id: u64, jpeg: Option<String>) -> bool {
    let outbound = lock(&OUTBOUND);
    if !is_conn(&outbound, conn) {
        return false;
    }
    let mut waiting = lock(&SNAPSHOTS);
    if let Some(i) = waiting.iter().position(|(rid, rconn, _)| *rid == id && *rconn == conn) {
        let (_, _, reply) = waiting.swap_remove(i);
        let _ = reply.send(jpeg);
    }
    true
}

/// The current camera frame as base64 JPEG, or the fact to tell Hermes instead.
pub async fn snapshot() -> Result<String, &'static str> {
    let (id, reply) = {
        let outbound = lock(&OUTBOUND);
        let Some((conn, tx)) = outbound.as_ref() else { return Err(CAMERA_OFF) };
        match *lock(&CAMERA) {
            Some((camera_conn, state)) if camera_conn == *conn => {
                if state.switching {
                    return Err(CAMERA_SWITCHING); // a frame now would come from the old lens
                }
            }
            _ => return Err(CAMERA_OFF),
        }
        let id = NEXT_SNAPSHOT_ID.fetch_add(1, Ordering::AcqRel) + 1;
        let (reply_tx, reply) = tokio::sync::oneshot::channel();
        lock(&SNAPSHOTS).push((id, *conn, reply_tx));
        if tx.send(Outbound::Snapshot { id }).is_err() {
            lock(&SNAPSHOTS).retain(|(rid, _, _)| *rid != id);
            return Err(CAMERA_OFF);
        }
        (id, reply)
    };
    match tokio::time::timeout(SNAPSHOT_TIMEOUT, reply).await {
        Err(_) => {
            lock(&SNAPSHOTS).retain(|(rid, _, _)| *rid != id);
            Err(SNAPSHOT_TIMED_OUT)
        }
        // Dropped (phone gone, replaced, camera closed) or answered "off".
        Ok(Err(_)) | Ok(Ok(None)) => Err(CAMERA_OFF),
        Ok(Ok(Some(jpeg))) => if is_jpeg_b64(&jpeg) { Ok(jpeg) } else { Err(SNAPSHOT_INVALID) },
    }
}

fn is_jpeg_b64(b64: &str) -> bool {
    use base64::Engine;
    if b64.len() > MAX_SNAPSHOT_B64 {
        return false;
    }
    base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map(|bytes| bytes.starts_with(&[0xFF, 0xD8, 0xFF]))
        .unwrap_or(false)
}

// ── Source switch ────────────────────────────────────────────────────────────

static SWITCH: Mutex<()> = Mutex::new(());

/// The phone's mic and speaker follow the driver: only while the phone page
/// drives (attached on /ui and not parked) and /audio is connected. /audio
/// alone (e.g. the app's service reconnected after a PA restart, the page did
/// not) or a parked phone → local mic and speaker.
pub(crate) fn phone_is_source() -> bool {
    phone_attached() && crate::api::remote::is_ui_remote_active() && !is_parked()
}

/// Converge the active source on phone_is_source. Called after every /audio
/// attach / detach and every driver convergence (blocking: it joins the wake worker). Reading the
/// target here rather than passing it in keeps racing connect / disconnect
/// tasks from landing in the wrong order.
pub fn sync_source(app: &AppHandle) {
    let _switch = lock(&SWITCH);
    let online = phone_is_source();
    {
        // Under the capture gate: no capture can start on the old source after this.
        let _gate = lock_capture_gate();
        if ONLINE.load(Ordering::Acquire) == online {
            return;
        }
        ONLINE.store(online, Ordering::Release);
        SOURCE_EPOCH.fetch_add(1, Ordering::AcqRel);
    }
    eprintln!("[remote] audio source → {}", if online { "phone" } else { "local" });
    crate::commands::chat::stop_audio_queue(if online { "audio source → phone" } else { "audio source → local" });
    crate::voice::sherpa_wake::stop_wake_listener();
    crate::voice::conversation::stop_conversation();
    // Workers on the old source also exit on their own once they see the epoch
    // move; wait for them so the frontend's re-arm doesn't hit "capture busy".
    for _ in 0..300 {
        if current_owner() == OWNER_NONE
            && !crate::voice::sherpa_wake::is_wake_active()
            && !crate::voice::conversation::is_active()
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = app.emit("audio-source-changed", serde_json::json!({ "remote": online }));
}

/// Phone state is process-global; every test touching it (here and in api/)
/// holds this.
#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;


    fn next_play(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Outbound>) -> u64 {
        for _ in 0..200 {
            match rx.try_recv() {
                Ok(Outbound::Play { seq, .. }) => return seq,
                Ok(_) => continue,
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
        panic!("no Play sent");
    }

    #[test]
    fn remote_frames_reach_the_capture_as_16k_mono_samples() {
        let _t = lock(&TEST_LOCK);
        let (ptx, _prx) = tokio::sync::mpsc::unbounded_channel();
        let conn = attach_phone(ptx);
        let (tx, rx) = mpsc::channel();
        let id = attach_capture(Box::new(move |s: &[i16], sr: u32| tx.send((s.to_vec(), sr)).unwrap()));
        let frame: Vec<u8> = [1i16, -2, 32767, -32768].iter().flat_map(|s| s.to_le_bytes()).collect();
        assert!(push_pcm(conn, &frame));
        assert!(push_pcm(conn, &[7])); // a lone byte is not a sample
        assert_eq!(rx.try_recv().unwrap(), (vec![1, -2, 32767, -32768], REMOTE_SAMPLE_RATE));
        assert!(rx.try_recv().is_err());
        detach_capture(id);
        push_pcm(conn, &frame);
        assert!(rx.try_recv().is_err(), "detached capture gets nothing");
        detach_phone(conn);
    }

    #[test]
    fn a_replaced_phone_can_no_longer_feed_the_capture_or_the_player() {
        let _t = lock(&TEST_LOCK);
        let (tx1, _rx1) = tokio::sync::mpsc::unbounded_channel();
        let old = attach_phone(tx1);
        let (tx2, mut rx2) = tokio::sync::mpsc::unbounded_channel();
        let new = attach_phone(tx2);
        let (tx, rx) = mpsc::channel();
        let id = attach_capture(Box::new(move |s: &[i16], _| tx.send(s.to_vec()).unwrap()));
        assert!(!push_pcm(old, &[1, 0]), "old phone's PCM is refused");
        assert!(rx.try_recv().is_err());
        assert!(push_pcm(new, &[2, 0]));
        assert_eq!(rx.try_recv().unwrap(), vec![2]);
        detach_capture(id);

        let player = std::thread::spawn(|| play_sentence(vec![1], || true, || {}));
        let seq = next_play(&mut rx2);
        assert!(!on_report(old, "done", seq), "old phone's report is refused");
        assert!(lock(&PLAYBACK).as_ref().map(|p| !p.finished).unwrap_or(false));
        assert!(on_report(new, "done", seq));
        assert!(!player.join().unwrap());
        detach_phone(new);
    }

    #[test]
    fn playing_then_done_drives_the_player_in_order() {
        let _t = lock(&TEST_LOCK);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = attach_phone(tx);
        let (ev_tx, ev_rx) = mpsc::channel();
        let player = std::thread::spawn(move || {
            let started = play_sentence(vec![0xff], || true, || ev_tx.send("playing").unwrap());
            (started, ev_rx)
        });
        let seq = next_play(&mut rx);
        on_report(conn, "done", seq + 100); // another sentence's report: ignored
        on_report(conn, "playing", seq);
        on_report(conn, "done", seq);
        let (started, ev_rx) = player.join().unwrap();
        assert!(started);
        assert_eq!(ev_rx.try_recv().unwrap(), "playing");
        assert!(detach_phone(conn));
    }

    #[test]
    fn done_without_playing_reports_not_started() {
        let _t = lock(&TEST_LOCK);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = attach_phone(tx);
        let player = std::thread::spawn(|| play_sentence(vec![1], || true, || panic!("never played")));
        let seq = next_play(&mut rx);
        on_report(conn, "done", seq);
        assert!(!player.join().unwrap());
        detach_phone(conn);
    }

    #[test]
    fn clear_releases_the_player_and_tells_the_phone() {
        let _t = lock(&TEST_LOCK);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = attach_phone(tx);
        let player = std::thread::spawn(|| play_sentence(vec![1], || true, || {}));
        let seq = next_play(&mut rx);
        on_report(conn, "playing", seq);
        std::thread::sleep(Duration::from_millis(20));
        clear();
        assert!(player.join().unwrap(), "it had started");
        assert!(matches!(rx.try_recv(), Ok(Outbound::Clear)));
        on_report(conn, "done", seq); // late report from the cleared sentence: harmless
        assert!(lock(&PLAYBACK).is_none());
        detach_phone(conn);
    }

    #[test]
    fn stale_generation_is_never_sent() {
        let _t = lock(&TEST_LOCK);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = attach_phone(tx);
        assert!(!play_sentence(vec![1], || false, || panic!("stale")));
        assert!(rx.try_recv().is_err());
        detach_phone(conn);
    }

    #[test]
    fn disconnect_releases_the_player_and_old_connection_detach_is_a_no_op() {
        let _t = lock(&TEST_LOCK);
        let (tx1, _rx1) = tokio::sync::mpsc::unbounded_channel();
        let old = attach_phone(tx1);
        let (tx2, mut rx2) = tokio::sync::mpsc::unbounded_channel();
        let new = attach_phone(tx2);
        assert!(!detach_phone(old), "replaced connection no longer owns the slot");
        let player = std::thread::spawn(|| play_sentence(vec![1], || true, || {}));
        next_play(&mut rx2);
        assert!(detach_phone(new));
        assert!(!player.join().unwrap());
        assert!(!play_sentence(vec![1], || true, || {}), "no phone: nothing is sent");
    }

    // ── Camera snapshot ──

    const JPEG_B64: &str = "/9j/4AAQ"; // FF D8 FF E0 00 10
    const ON: Option<Camera> = Some(Camera { facing: Some(Facing::Back), switching: false });

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap().block_on(f)
    }

    async fn next_snapshot(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Outbound>) -> u64 {
        loop {
            match rx.recv().await {
                Some(Outbound::Snapshot { id }) => return id,
                Some(_) => continue,
                None => panic!("no Snapshot sent"),
            }
        }
    }

    #[test]
    fn snapshot_without_an_open_camera_is_the_camera_off_fact() {
        let _t = lock(&TEST_LOCK);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = attach_phone(tx);
        assert_eq!(block_on(snapshot()), Err(CAMERA_OFF));
        assert!(on_camera(conn, ON));
        assert!(is_camera_on());
        assert!(on_camera(conn, None));
        assert_eq!(block_on(snapshot()), Err(CAMERA_OFF));
        detach_phone(conn);
        assert_eq!(block_on(snapshot()), Err(CAMERA_OFF), "no phone");
    }

    #[test]
    fn snapshot_returns_the_frame_the_current_phone_sends() {
        let _t = lock(&TEST_LOCK);
        let (tx_old, _rx_old) = tokio::sync::mpsc::unbounded_channel();
        let old = attach_phone(tx_old);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = attach_phone(tx);
        assert!(!on_camera(old, ON), "replaced phone can't open the camera");
        assert!(on_camera(conn, ON));
        let frame = block_on(async {
            let request = tokio::spawn(snapshot());
            let id = next_snapshot(&mut rx).await;
            assert!(!on_snapshot(old, id, Some("forged".into())), "old connection's reply is refused");
            on_snapshot(conn, id + 1000, Some("other".into())); // unknown id: ignored
            assert!(on_snapshot(conn, id, Some(JPEG_B64.into())));
            request.await.unwrap()
        });
        assert_eq!(frame, Ok(JPEG_B64.to_string()));
        detach_phone(conn);
        assert!(!is_camera_on());
    }

    #[test]
    fn snapshot_ends_with_a_fact_when_the_phone_closes_goes_away_or_sends_junk() {
        let _t = lock(&TEST_LOCK);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = attach_phone(tx);
        on_camera(conn, ON);
        let (closed, junk, off) = block_on(async {
            let request = tokio::spawn(snapshot());
            next_snapshot(&mut rx).await;
            on_camera(conn, None);
            let closed = request.await.unwrap();

            on_camera(conn, ON);
            let request = tokio::spawn(snapshot());
            let id = next_snapshot(&mut rx).await;
            on_snapshot(conn, id, Some("bm90IGEganBlZw==".into()));
            let junk = request.await.unwrap();

            let request = tokio::spawn(snapshot());
            let id = next_snapshot(&mut rx).await;
            on_snapshot(conn, id, None);
            (closed, junk, request.await.unwrap())
        });
        assert_eq!(closed, Err(CAMERA_OFF));
        assert_eq!(junk, Err(SNAPSHOT_INVALID));
        assert_eq!(off, Err(CAMERA_OFF));

        let replaced = block_on(async {
            let request = tokio::spawn(snapshot());
            next_snapshot(&mut rx).await;
            let (tx2, _rx2) = tokio::sync::mpsc::unbounded_channel();
            let next = attach_phone(tx2);
            let replaced = request.await.unwrap();
            detach_phone(next);
            replaced
        });
        assert_eq!(replaced, Err(CAMERA_OFF), "a replaced phone's pending request ends");
        assert!(!is_camera_on(), "a new phone starts with the camera closed");
    }

    #[test]
    fn camera_wait_ends_on_the_wanted_state_or_the_timeout() {
        let _t = lock(&TEST_LOCK);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = attach_phone(tx);
        let front = Some(Camera::ready(Facing::Front));
        let flipped = block_on(async {
            let wait = tokio::spawn(wait_camera(move |c| c == front, Duration::from_secs(5)));
            tokio::time::sleep(Duration::from_millis(20)).await;
            on_camera(conn, ON); // still the back lens: keeps waiting
            on_camera(conn, Some(Camera { facing: Some(Facing::Front), switching: true }));
            tokio::time::sleep(Duration::from_millis(20)).await;
            on_camera(conn, front);
            wait.await.unwrap()
        });
        assert_eq!(flipped, front);
        on_camera(conn, None);
        assert_eq!(block_on(wait_camera(|c| c.is_some(), Duration::from_millis(50))), None, "never reported on");
        detach_phone(conn);
    }

    #[test]
    fn no_frame_is_asked_for_while_the_lens_is_switching() {
        let _t = lock(&TEST_LOCK);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = attach_phone(tx);
        on_camera(conn, Some(Camera { facing: Some(Facing::Front), switching: true }));
        assert!(is_camera_on());
        assert_eq!(block_on(snapshot()), Err(CAMERA_SWITCHING));
        assert!(rx.try_recv().is_err(), "nothing was sent to the phone");
        detach_phone(conn);
    }

    #[test]
    fn a_parked_phone_is_not_the_source_and_reconnecting_keeps_it_parked() {
        let _t = lock(&TEST_LOCK);
        crate::api::remote::set_ui_remote_active(true); // the phone page drives
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = attach_phone(tx);
        assert!(phone_is_source());
        set_parked(true);
        assert!(!phone_is_source(), "parked → local mic and speaker");

        let (cap_tx, cap_rx) = mpsc::channel();
        let id = attach_capture(Box::new(move |s: &[i16], _| cap_tx.send(s.to_vec()).unwrap()));
        assert!(push_pcm(conn, &[1, 0]), "parked phone stays connected");
        assert!(cap_rx.try_recv().is_err(), "its PCM is not listened to");

        // A report while parked changes nothing, even for a sentence being waited on.
        *lock(&PLAYBACK) = Some(Pending { seq: 7, played: false, finished: false });
        assert!(on_report(conn, "done", 7));
        assert!(lock(&PLAYBACK).as_ref().is_some_and(|p| !p.finished));
        *lock(&PLAYBACK) = None;

        // The app comes back to the foreground: a new connection, still parked.
        let (tx2, _rx2) = tokio::sync::mpsc::unbounded_channel();
        let again = attach_phone(tx2);
        assert!(is_parked() && !phone_is_source());
        assert!(push_pcm(again, &[2, 0]));
        assert!(cap_rx.try_recv().is_err());

        set_parked(false);
        assert!(phone_is_source());
        assert!(push_pcm(again, &[3, 0]));
        assert_eq!(cap_rx.try_recv().unwrap(), vec![3]);
        detach_capture(id);
        detach_phone(again);
        crate::api::remote::set_ui_remote_active(false);
    }

    #[test]
    fn wake_reaches_the_phone_only_while_it_is_the_audio_source() {
        let _t = lock(&TEST_LOCK);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = attach_phone(tx);
        ONLINE.store(false, Ordering::Release);
        assert!(!wake_phone(), "local mic heard it: the phone stays dark");
        assert!(rx.try_recv().is_err());
        ONLINE.store(true, Ordering::Release);
        assert!(wake_phone());
        assert!(matches!(rx.try_recv(), Ok(Outbound::Wake)));
        detach_phone(conn);
        assert!(!wake_phone(), "no phone");
        ONLINE.store(false, Ordering::Release);
    }

    #[test]
    fn snapshot_times_out_when_the_phone_never_answers() {
        let _t = lock(&TEST_LOCK);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = attach_phone(tx);
        on_camera(conn, ON);
        assert_eq!(block_on(snapshot()), Err(SNAPSHOT_TIMED_OUT));
        assert!(lock(&SNAPSHOTS).is_empty(), "timed-out request is forgotten");
        detach_phone(conn);
    }
}
