//! Durable, append-only escrow event journal — the dispute/audit trail.
//!
//! Every consequential money-path event (room created, DKG completed, deposit
//! detected, settlement finalized with both co-signatures, payout broadcast /
//! failed, client-reported fault) is appended as one JSON line to an on-disk
//! file. Unlike the in-memory `EscrowRoom` state — which evaporates on restart —
//! this survives, so a room can be reconstructed and adjudicated after the fact.
//!
//! The strongest dispute artifact is the pair of player co-signatures captured in
//! the `settlement_finalized` event: non-repudiable proof of who agreed to what.
//!
//! Format: newline-delimited JSON (`events.jsonl`), one object per line:
//!   {"ts":<unix_ms>,"code":"<room>","kind":"<event>","data":{...}}
//!
//! Reads/writes are serialised by a process-global mutex. Events are low-frequency
//! (a handful per game), so the synchronous append under the lock is negligible.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

struct Journal {
    path: PathBuf,
    lock: Mutex<()>,
}

static JOURNAL: OnceLock<Journal> = OnceLock::new();

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Initialise the journal at `path`. Creates parent dirs. Idempotent-ish: a second
/// call is ignored (first wins). Call once at startup; if never called, `record`
/// and `read_room` become no-ops (so tests / trusted-dealer mode don't need a file).
pub fn init(path: impl Into<PathBuf>) {
    let path = path.into();
    if let Some(dir) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(dir) {
            tracing::warn!("journal: create_dir_all({}) failed: {}", dir.display(), e);
            return;
        }
    }
    let _ = JOURNAL.set(Journal { path, lock: Mutex::new(()) });
    tracing::info!("journal: enabled");
}

/// Append one event. Never panics; a write failure is logged and swallowed so the
/// money path is never blocked by journal IO. No-op if the journal was not `init`ed.
pub fn record(code: &str, kind: &str, data: Value) {
    let Some(j) = JOURNAL.get() else { return };
    let entry = serde_json::json!({
        "ts": now_ms(),
        "code": code,
        "kind": kind,
        "data": data,
    });
    let line = match serde_json::to_string(&entry) {
        Ok(s) => s,
        Err(e) => { tracing::warn!("journal: serialize failed: {}", e); return; }
    };
    let _guard = j.lock.lock().unwrap_or_else(|e| e.into_inner());
    match OpenOptions::new().create(true).append(true).open(&j.path) {
        Ok(mut f) => {
            if let Err(e) = writeln!(f, "{}", line) {
                tracing::warn!("journal: append failed: {}", e);
            }
        }
        Err(e) => tracing::warn!("journal: open({}) failed: {}", j.path.display(), e),
    }
}

/// True if the journal was `init`ed (durable audit trail active). Powers `/status`.
pub fn is_enabled() -> bool {
    JOURNAL.get().is_some()
}

/// Read back every event for one room, in journal (chronological) order. Returns
/// an empty vec if the journal is disabled or unreadable. Powers `GET /audit/{code}`.
pub fn read_room(code: &str) -> Vec<Value> {
    let Some(j) = JOURNAL.get() else { return Vec::new() };
    let _guard = j.lock.lock().unwrap_or_else(|e| e.into_inner());
    let content = match std::fs::read_to_string(&j.path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    content
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v.get("code").and_then(|c| c.as_str()) == Some(code))
        .collect()
}

/// Read back every event across all rooms, in journal order. Powers the `/disputes`
/// list (which groups + classifies by room). Empty if disabled/unreadable.
pub fn read_all() -> Vec<Value> {
    let Some(j) = JOURNAL.get() else { return Vec::new() };
    let _guard = j.lock.lock().unwrap_or_else(|e| e.into_inner());
    let content = match std::fs::read_to_string(&j.path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    content
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_roundtrip_and_room_filter() {
        // unique temp path so the test is hermetic and doesn't collide with a real journal
        let dir = std::env::temp_dir().join(format!("escrow-journal-test-{}", std::process::id()));
        let path = dir.join("events.jsonl");
        let _ = std::fs::remove_file(&path);
        init(&path);

        // two rooms interleaved
        record("room-A", "room_created", serde_json::json!({"required_deposit": 200000}));
        record("room-B", "room_created", serde_json::json!({"required_deposit": 100000}));
        record("room-A", "deposit_detected", serde_json::json!({"seat": 0, "txid": "abcd", "value_zat": 200000}));
        record("room-A", "settlement_finalized", serde_json::json!({"player_a_sig": "sigA", "player_b_sig": "sigB"}));

        let a = read_room("room-A");
        assert_eq!(a.len(), 3, "room-A must see exactly its 3 events, not room-B's");
        assert_eq!(a[0]["kind"], "room_created");
        assert_eq!(a[1]["kind"], "deposit_detected");
        assert_eq!(a[2]["kind"], "settlement_finalized");
        // the dispute-critical artifact survives the round-trip
        assert_eq!(a[2]["data"]["player_a_sig"], "sigA");
        assert_eq!(a[2]["data"]["player_b_sig"], "sigB");
        assert!(a[0]["ts"].as_u64().is_some(), "every event carries a timestamp");

        let b = read_room("room-B");
        assert_eq!(b.len(), 1, "room-B isolated from room-A");

        let _ = std::fs::remove_file(&path);
    }
}
