//! GH #1451 (D38 V3, T-8, OR-38-78) -- the turn path parks beside a delegation
//! that runs under another session.
//!
//! WHY this file exists: in the R039 acceptance (09:07:30/38) two model runs
//! answered one delegation and the voice spoke both facts. The park of #1407
//! writes `pending:delegation:<session>#<label>` under the session of the
//! DELEGATION round (the call id); the turn path reads it under ITS session
//! (`phone-...`), so the keys never meet -- no such row in any R039 blob, no
//! "parks beside delegation" line in the journal since 06:00. The pin of #1407
//! (`d53_fx_the_turn_path_parks_beside_its_delegation`) runs both rounds under
//! one session and cannot see it. With `delegation_park_by_call` "1" the park
//! is keyed by the duplex call and the turn label, both rounds carry the
//! call id. Shipped "0": the key of before.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const ASK: &str = "When do I call my mother?";
/// A deadline no run of this test reaches (2100-01-01).
const OPEN_UNTIL: u64 = 4_102_444_800_000;
const CALL: &str = "call-9";
const PHONE: &str = "phone-2026-11-07T09:06:50Z";

fn seam(h: &mut Hive, session: &str, turn: &str, iter: &str, extra: Value, body: Value) {
    h.out.clear();
    let mut hop = json!({"session_id": session, "turn_id": turn, "iter": iter, "phase": "",
                         "path": "simple", "needs": "", "families": "",
                         "engine": "duplex", "call_id": CALL});
    for (k, v) in obj(extra) {
        hop[k] = v;
    }
    let ctx = json!({"session_id": session, "turn_id": turn, "iter": iter,
                     "channel": "test", "audience_set": ROUND_E, "path": hop["path"].clone()});
    h.lane("in_curate", ctx, hop, body);
}

fn calls(h: &Hive) -> usize {
    h.routed("quick").len() + h.routed("brain").len()
}

fn parked(h: &Hive) -> usize {
    h.rows("SELECT value FROM marks WHERE kind = 'turn_parked'")
        .iter()
        .filter(|r| sj::from_str::<Value>(r[0].as_str().unwrap_or("")).is_ok())
        .count()
}

fn run(knob: &str) -> (usize, usize) {
    let (calls, parked, _) = run_rows(knob);
    (calls, parked)
}

fn run_rows(knob: &str) -> (usize, usize, Vec<Value>) {
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "delegation_park_by_call", json!(knob)),
    ]);
    let label = format!("{CALL}#1");
    // The delegation round: its own session (the call id), keyed `<label>~<deadline>~<hex8>`.
    seam(
        &mut h,
        CALL,
        &format!("{label}~{OPEN_UNTIL}~ab12cd34"),
        "0",
        json!({"path": "", "delegation_id": "item_d1"}),
        json!({"messages": [user(&format!("[delegation item_d1 from the voice channel] {ASK}"))]}),
    );
    assert_eq!(
        calls(&h),
        1,
        "the delegation makes its call: {:?}",
        h.stderr
    );
    // The turn path of the same turn, under the phone session.
    seam(
        &mut h,
        PHONE,
        &label,
        "0",
        json!({}),
        json!({"messages": [user(ASK)]}),
    );
    let rows = h
        .rows("SELECT value FROM state WHERE key LIKE 'pending:delegation:%'")
        .iter()
        .filter_map(|r| sj::from_str::<Value>(r[0].as_str().unwrap_or("")).ok())
        .collect();
    (calls(&h), parked(&h), rows)
}

/// Pin: two sessions, one turn -- the turn path sends no second call.
#[test]
fn the_turn_path_parks_beside_a_delegation_of_another_session() {
    if !shipped() {
        return;
    }
    assert_eq!(run("1"), (0, 1), "one answer per delegation");
}

/// Lock: shipped off, the key of before -- the turn path goes (the R039 defect).
#[test]
fn off_the_key_of_before_stays() {
    if !shipped() {
        return;
    }
    assert_eq!(run("0"), (1, 0));
}

/// Pin (OR-d38-3.o15.8): the park carries the round's audience, so a rebuild
/// that knows no call can still tell the member's row; off, the row of before.
#[test]
fn the_park_carries_its_audience() {
    if !shipped() {
        return;
    }
    for (knob, want) in [("1", json!(ROUND_E)), ("0", Value::Null)] {
        let (_, _, rows) = run_rows(knob);
        assert_eq!(rows.len(), 1, "{knob}: {rows:?}");
        assert_eq!(
            rows[0].get("audience_set").cloned().unwrap_or(Value::Null),
            want,
            "{knob}: {rows:?}"
        );
    }
}
