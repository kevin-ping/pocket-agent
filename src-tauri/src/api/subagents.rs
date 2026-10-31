//! Background (async) subagent lamp. Hermes never wakes PA when an async delegation
//! finishes: the gateway stores the result as an `async_delegation_complete` row in the
//! session, folded into the next PA turn. So both edges are read from the session
//! transcript: the `delegate_task` dispatch row lights the lamp, the completion row marks
//! the turn that announces it, and the frontend settles at that turn's chat-audio-done.
//! Only delegation ids and session ids are kept; message text never leaves this module.
use super::runs::{authorized, http};
use crate::commands::config::{get_api_key, get_api_url};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Emitter};

const MESSAGE_PAGE: usize = 80;
const MAX_PENDING: usize = 16;
const MAX_SETTLED: usize = 64;

#[derive(Clone, Copy, PartialEq, Debug)]
enum State {
    Running,
    /// Its completion row exists: this turn announces it; settle at audio-done.
    Delivered,
    /// Hermes said it is finished or lost ([SUBAGENT_DONE:id]); settle at audio-done.
    Dismissed,
}

#[derive(Default)]
struct Ledger {
    pending: HashMap<String, (String, State)>,
    /// Ids already settled, so an old dispatch row still in the page never relights.
    settled: Vec<String>,
}

fn ledger() -> &'static Mutex<Ledger> {
    static LEDGER: OnceLock<Mutex<Ledger>> = OnceLock::new();
    LEDGER.get_or_init(|| Mutex::new(Ledger::default()))
}

fn id_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r#""delegation_id"\s*:\s*"(deleg_[0-9a-f]{1,64})""#).unwrap())
}

fn delivered_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"ASYNC DELEGATION [A-Z ]+— (deleg_[0-9a-f]{1,64})\b").unwrap())
}

fn dismiss_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\[SUBAGENT_DONE:(deleg_[0-9a-f]{1,64})\]").unwrap())
}

fn content(message: &Value) -> &str {
    message["content"].as_str().unwrap_or("")
}

/// Delegation ids from `delegate_task` tool results that dispatched in the background.
/// Other tools' output is untrusted text and must never light the lamp.
fn dispatched_ids(messages: &[Value]) -> Vec<String> {
    let mut ids = Vec::new();
    for message in messages.iter().filter(|m| m["role"] == "tool" && m["tool_name"] == "delegate_task") {
        let text = content(message);
        if !text.contains("\"dispatched\"") {
            continue;
        }
        for cap in id_re().captures_iter(text) {
            if !ids.contains(&cap[1].to_string()) {
                ids.push(cap[1].to_string());
            }
        }
    }
    ids
}

/// Delegation ids whose async completion row is in the transcript.
fn delivered_ids(messages: &[Value]) -> HashSet<String> {
    messages
        .iter()
        .filter(|m| m["display_kind"] == "async_delegation_complete")
        .flat_map(|m| delivered_re().captures_iter(content(m)).map(|c| c[1].to_string()).collect::<Vec<_>>())
        .collect()
}

/// Ids Hermes marked finished or lost in its reply.
pub fn dismiss_ids(reply: &str) -> Vec<String> {
    dismiss_re().captures_iter(reply).map(|c| c[1].to_string()).collect()
}

/// Session ids PA creates are plain (`pocket-agent-YYYY-MM-DD`); anything else is not queried.
fn valid_session(session: &str) -> bool {
    !session.is_empty()
        && session.len() <= 128
        && session.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

async fn recent_messages(session: &str) -> Option<Vec<Value>> {
    if !valid_session(session) {
        return None;
    }
    let url = format!(
        "{}/api/sessions/{}/messages?order=latest&limit={}",
        get_api_url().trim_end_matches('/'),
        session,
        MESSAGE_PAGE
    );
    let response = authorized(http().get(url), &get_api_key())
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        eprintln!("[subagents] session messages HTTP {}", response.status());
        return None;
    }
    let value: Value = response.json().await.ok()?;
    value["data"].as_array().cloned()
}

fn emit_count(app: &AppHandle, ledger: &Ledger) {
    let _ = app.emit("background-subagents", json!({"count": ledger.pending.len()}));
}

fn running_ids(ledger: &Ledger) -> Vec<String> {
    let mut ids: Vec<String> = ledger
        .pending
        .iter()
        .filter(|(_, (_, state))| *state == State::Running)
        .map(|(id, _)| id.clone())
        .collect();
    ids.sort();
    ids
}

/// Before a UI turn: mark delegations whose result is now in the session. Returns the
/// reminder for Hermes about the ones still out, or None.
pub async fn before_turn(app: &AppHandle) -> Option<String> {
    let sessions: HashSet<String> = {
        let ledger = ledger().lock().unwrap();
        ledger.pending.values().filter(|(_, s)| *s == State::Running).map(|(sess, _)| sess.clone()).collect()
    };
    for session in sessions {
        let Some(messages) = recent_messages(&session).await else { continue };
        let delivered = delivered_ids(&messages);
        let mut ledger = ledger().lock().unwrap();
        for (id, (_, state)) in ledger.pending.iter_mut() {
            if *state == State::Running && delivered.contains(id) {
                *state = State::Delivered;
            }
        }
    }
    let ledger = ledger().lock().unwrap();
    emit_count(app, &ledger);
    reminder(&running_ids(&ledger))
}

fn reminder(running: &[String]) -> Option<String> {
    if running.is_empty() {
        return None;
    }
    Some(format!(
        "\nBackground subagent(s) {} were dispatched earlier and have not reported back. \
If you determine one is finished or lost, append [SUBAGENT_DONE:<id>] to your reply.",
        running.join(", ")
    ))
}

/// After a UI turn that called delegate_task: register the background dispatches.
/// The transcript may land just after the stream ends, so look twice.
pub async fn after_delegate(app: &AppHandle, session: &str) {
    for delay in [0u64, 1500] {
        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        let Some(messages) = recent_messages(session).await else { continue };
        let delivered = delivered_ids(&messages);
        let mut ledger = ledger().lock().unwrap();
        let fresh: Vec<String> = dispatched_ids(&messages)
            .into_iter()
            .filter(|id| !delivered.contains(id) && !ledger.settled.contains(id) && !ledger.pending.contains_key(id))
            .collect();
        for id in &fresh {
            if ledger.pending.len() < MAX_PENDING {
                ledger.pending.insert(id.clone(), (session.to_string(), State::Running));
            }
        }
        if !fresh.is_empty() {
            emit_count(app, &ledger);
            return;
        }
    }
}

/// Hermes marked these finished or lost; they go dark at this turn's settle.
pub fn dismiss(ids: &[String]) {
    let mut ledger = ledger().lock().unwrap();
    for id in ids {
        if let Some((_, state)) = ledger.pending.get_mut(id) {
            *state = State::Dismissed;
        }
    }
}

fn settle_ledger(ledger: &mut Ledger) {
    let done: Vec<String> = ledger
        .pending
        .iter()
        .filter(|(_, (_, state))| *state != State::Running)
        .map(|(id, _)| id.clone())
        .collect();
    for id in done {
        ledger.pending.remove(&id);
        ledger.settled.push(id);
    }
    let excess = ledger.settled.len().saturating_sub(MAX_SETTLED);
    ledger.settled.drain(..excess);
}

/// Called by the frontend at turn end (chat-audio-done / chat-stream-error).
#[tauri::command]
pub fn settle_background_subagents(app: AppHandle) {
    let mut ledger = ledger().lock().unwrap();
    settle_ledger(&mut ledger);
    emit_count(&app, &ledger);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(text: &str) -> Value {
        json!({"role": "tool", "tool_name": "delegate_task", "content": text})
    }

    #[test]
    fn dispatched_ids_read_background_tool_results_only() {
        let single = tool(r#"{"status": "dispatched", "mode": "background", "count": 1, "delegation_id": "deleg_8bceaf11", "goals": ["secret goal"]}"#);
        let units = tool(r#"{"status":"dispatched","delegation_id":"deleg_aa","units":[{"delegation_id":"deleg_aa"},{"delegation_id":"deleg_bb"}]}"#);
        let sync = tool(r#"{"status": "completed", "delegation_id": "deleg_cc", "summary": "x"}"#);
        let user = json!({"role": "user", "content": r#""status": "dispatched", "delegation_id": "deleg_dd""#});
        let bad = tool(r#"{"status": "dispatched", "delegation_id": "deleg_../../x"}"#);
        let web = json!({"role": "tool", "tool_name": "web_extract",
            "content": r#"page says "status": "dispatched", "delegation_id": "deleg_ee""#});
        assert!(dispatched_ids(&[web]).is_empty());
        assert_eq!(dispatched_ids(&[single]), vec!["deleg_8bceaf11"]);
        assert_eq!(dispatched_ids(&[units]), vec!["deleg_aa", "deleg_bb"]);
        assert!(dispatched_ids(&[sync, user, bad]).is_empty());
    }

    #[test]
    fn delivered_ids_match_single_and_batch_rows() {
        let single = json!({"role": "user", "display_kind": "async_delegation_complete",
            "content": "[ASYNC DELEGATION COMPLETE — deleg_3a512e19]\nchild output"});
        let batch = json!({"role": "user", "display_kind": "async_delegation_complete",
            "content": "[IMPORTANT: 2 background subagent delegations completed]\n\n[ASYNC DELEGATION BATCH COMPLETE — deleg_aa]\n...\n\n[ASYNC DELEGATION COMPLETE — deleg_bb]"});
        let quoted = json!({"role": "user", "content": "[ASYNC DELEGATION COMPLETE — deleg_ff]"});
        let got = delivered_ids(&[single, batch, quoted]);
        assert_eq!(got, HashSet::from(["deleg_3a512e19".into(), "deleg_aa".into(), "deleg_bb".into()]));
    }

    #[test]
    fn dismiss_ids_only_accept_well_formed_markers() {
        assert_eq!(dismiss_ids("好的。[SUBAGENT_DONE:deleg_ab12] 结束"), vec!["deleg_ab12"]);
        assert!(dismiss_ids("[SUBAGENT_DONE:rm -rf] [SUBAGENT_DONE:deleg_XY]").is_empty());
    }

    #[test]
    fn session_ids_are_restricted_to_plain_names() {
        assert!(valid_session("pocket-agent-2026-10-31"));
        assert!(!valid_session("bridge:chess:1"));
        assert!(!valid_session("../x"));
        assert!(!valid_session(""));
    }

    #[test]
    fn settle_keeps_running_and_remembers_settled() {
        let mut ledger = Ledger::default();
        for (id, state) in [("deleg_1", State::Running), ("deleg_2", State::Delivered), ("deleg_3", State::Dismissed)] {
            ledger.pending.insert(id.into(), ("s".into(), state));
        }
        settle_ledger(&mut ledger);
        assert_eq!(ledger.pending.keys().collect::<Vec<_>>(), vec!["deleg_1"]);
        assert!(ledger.settled.contains(&"deleg_2".into()) && ledger.settled.contains(&"deleg_3".into()));
        ledger.settled = (0..100).map(|i| format!("deleg_{i}")).collect();
        settle_ledger(&mut ledger);
        assert_eq!(ledger.settled.len(), MAX_SETTLED);
        assert_eq!(ledger.settled[0], "deleg_36");
    }

    #[test]
    fn reminder_lists_running_ids_and_the_marker() {
        assert!(reminder(&[]).is_none());
        let text = reminder(&["deleg_1".into()]).unwrap();
        assert!(text.contains("deleg_1") && text.contains("[SUBAGENT_DONE:<id>]"));
    }
}
