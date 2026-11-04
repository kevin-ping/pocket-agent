//! Latest voice setup / voice service state, so a frontend that subscribes
//! late (still loading, or a phone taking over) can read what it missed.
//! Every `venv-setup-*` and `voice-service-status` event goes through
//! `publish`, which updates the snapshot and adds its `seq` to the payload;
//! the frontend drops anything older than what it has applied.

use serde::Serialize;
use serde_json::Value;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SetupSnapshot {
    pub seq: u64,
    /// `idle` (nothing reported yet), `installing`, `error` or `ready`.
    pub state: &'static str,
    pub phase: String,
    pub detail: String,
}

static SNAPSHOT: Mutex<SetupSnapshot> = Mutex::new(SetupSnapshot {
    seq: 0,
    state: "idle",
    phase: String::new(),
    detail: String::new(),
});

pub fn snapshot() -> SetupSnapshot {
    SNAPSHOT.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

/// Record the new state and return its sequence number. The event is emitted
/// after the lock is released: event listeners (the /ui forwarder) take their
/// own locks, and the frontend drops anything older than what it has applied,
/// so events need not leave in sequence order.
fn record(state: &'static str, phase: &str, detail: &str) -> u64 {
    let mut s = SNAPSHOT.lock().unwrap_or_else(|p| p.into_inner());
    s.seq += 1;
    s.state = state;
    s.phase = phase.to_string();
    s.detail = detail.to_string();
    s.seq
}

/// Emit `event` with `payload` (an object) plus `seq`, after recording
/// `state` / `phase` / `detail` as the current setup state.
pub fn publish(app: &AppHandle, event: &str, mut payload: Value, state: &'static str, phase: &str, detail: &str) {
    let seq = record(state, phase, detail);
    if let Value::Object(map) = &mut payload {
        map.insert("seq".into(), seq.into());
    }
    let _ = app.emit(event, payload);
}

#[tauri::command]
pub fn voice_setup_status() -> SetupSnapshot {
    snapshot()
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test owns the global snapshot so parallel tests cannot interleave.
    #[test]
    fn snapshot_follows_events_in_sequence() {
        let start = snapshot().seq;

        let first = record("installing", "installing-deps", "Collecting numpy");
        assert_eq!(
            snapshot(),
            SetupSnapshot {
                seq: start + 1,
                state: "installing",
                phase: "installing-deps".into(),
                detail: "Collecting numpy".into(),
            }
        );
        let second = record("error", "installing-deps", "pip failed");
        let s = snapshot();
        assert_eq!((s.seq, s.state, s.detail.as_str()), (start + 2, "error", "pip failed"));
        assert_eq!((first, second), (start + 1, start + 2), "the event carries the snapshot's seq");

        // Concurrent publishers (pip's stdout and stderr readers) get unique,
        // increasing numbers, and the snapshot ends on the highest one.
        let base = snapshot().seq;
        let seqs: Vec<u64> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        (0..50).map(|_| record("installing", "installing-deps", "line")).collect::<Vec<_>>()
                    })
                })
                .collect();
            handles.into_iter().flat_map(|h| h.join().unwrap()).collect()
        });
        for w in seqs.chunks(50) {
            assert!(w.windows(2).all(|p| p[0] < p[1]), "seq increases per publisher");
        }
        let mut sorted = seqs.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 400, "no seq handed out twice");
        assert_eq!(snapshot().seq, base + 400);
    }
}
