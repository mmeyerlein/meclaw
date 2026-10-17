//! GH #1225 (A13) -- a parked row whose step never came back is swept.
//!
//! WHY this file exists: a `pending:*` row of `state` holds a turn, a tap or
//! a summarizer's answer between two steps. A step that never came back (a
//! lost answer, a refused read twice) left it for good. Every parked row now
//! carries `parked_at` in its value, and the probe of a rebuild -- an event
//! that comes anyway, never a timer of its own (R-AG-1) -- takes down every
//! one older than `pending_sweep_s` by key and value, said as one
//! `pending_swept` mark. A row without `parked_at` is left alone.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

const S: &str = "s-a13";

fn has(h: &Hive, key: &str) -> bool {
    !h.rows(&format!("SELECT key FROM state WHERE key = '{key}'"))
        .is_empty()
}

#[test]
fn a_rebuild_sweeps_the_parked_rows_past_their_age() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "keep_recent", json!(1))]);
    let fresh = backdated(5).format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string();
    h.db.execute_batch(&format!(
        "INSERT INTO state (key, value) VALUES \
         ('pending:c-old', '{{\"parked_at\":\"2026-01-01T00:00:00.000000Z\",\"session\":\"x\"}}'), \
         ('pending:c-new', '{{\"parked_at\":\"{fresh}\",\"session\":\"x\"}}'), \
         ('pending:c-legacy', '{{\"session\":\"x\"}}');"
    ))
    .expect("seed the parked rows");
    turn(
        &mut h,
        S,
        "t-1",
        "cwA13 1: the parcel left.",
        "cwA13 1 reply.",
        json!({}),
    );
    assert!(has(&h, "pending:c-old"), "a turn sweeps nothing");
    turn(
        &mut h,
        S,
        "t-2",
        "cwA13 2: the lease is signed.",
        "cwA13 2 reply.",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
    );
    let order = h.clock_order_for(ROUND_E).expect("a strike");
    h.fire(&order);
    assert!(
        !has(&h, "pending:c-old"),
        "the old row is swept: {:?}",
        h.stderr
    );
    assert!(has(&h, "pending:c-new"), "a young row stays");
    assert!(has(&h, "pending:c-legacy"), "a row without its time stays");
    let marks: Vec<String> = h
        .rows("SELECT value FROM marks WHERE kind = 'pending_swept'")
        .into_iter()
        .map(|r| r[0].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(marks.len(), 1, "{marks:?}");
    assert!(marks[0].contains("pending:c-old"), "{marks:?}");
    assert_eq!(h.summ.len(), 1, "the sweep rides beside the rebuild");

    // The summarizer's answer is parked with its time, the form every
    // curator cell's parked rows share.
    h.answer("cwA13 summary.", "stop");
    let key = format!("pending:summary:{}", round_key(ROUND_E));
    let parked = h
        .ledger_ops
        .iter()
        .filter(|(from, op)| {
            from == "policy" && op["operation"] == "insert" && op["row"]["key"] == key.as_str()
        })
        .map(|(_, op)| op["row"]["value"].as_str().unwrap_or("").to_string())
        .collect::<Vec<_>>();
    assert!(
        !parked.is_empty() && parked.iter().all(|v| v.contains("\"parked_at\"")),
        "{parked:?}"
    );
}
