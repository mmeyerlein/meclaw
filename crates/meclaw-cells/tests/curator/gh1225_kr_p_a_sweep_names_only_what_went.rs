//! GH #1225 (A13, A3, K-5, policy) -- the sweep says only what went, a
//! refused sweep read is said, and a model pin is one row.
//!
//! WHY this file exists: the `pending_swept` mark went in the same bundle as
//! its deletes and named every key it meant to take down, also one a newer
//! step had parked anew or another sweep had taken first (two marks, one
//! deletion). A refused read of the parked rows (`p-pend`) swept nothing and
//! said nothing. `pending_sweep_s` took any count of at least 1 (a sweep
//! every second of a parked turn on its way). And the unique row of a model
//! pin -- the same pin twice is one row, also without an audience, another
//! round's is its own -- stood only as the form of the insert.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const S: &str = "s-krs";

fn has(h: &Hive, key: &str) -> bool {
    !h.rows(&format!("SELECT key FROM state WHERE key = '{key}'"))
        .is_empty()
}

fn marks(h: &Hive, kind: &str) -> Vec<String> {
    h.rows(&format!(
        "SELECT value FROM marks WHERE kind = '{kind}' ORDER BY seq"
    ))
    .into_iter()
    .map(|r| r[0].as_str().unwrap_or("").to_string())
    .collect()
}

/// Two turns of the round, the second ordering the strike of its cold cache.
fn strike(h: &mut Hive) -> Msg {
    turn(
        h,
        S,
        "t-1",
        "cwKRS 1: the parcel left.",
        "cwKRS 1 reply.",
        json!({}),
    );
    turn(
        h,
        S,
        "t-2",
        "cwKRS 2: the lease is signed.",
        "cwKRS 2 reply.",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
    );
    h.clock_order_for(ROUND_E)
        .unwrap_or_else(|| panic!("no strike ordered: {:?}", h.clock))
}

#[test]
fn the_sweep_mark_names_only_the_rows_that_went() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "keep_recent", json!(1)),
        ("intake", "pending_sweep_s", json!("31536000")),
    ]);
    let order = strike(&mut h);
    h.db.execute_batch(
        "INSERT INTO state (key, value) VALUES \
         ('pending:c-old', '{\"parked_at\":\"2026-01-01T00:00:00.000000Z\",\"session\":\"x\"}'), \
         ('pending:c-gone', '{\"parked_at\":\"2026-01-01T00:00:00.000000Z\",\"session\":\"x\"}');",
    )
    .expect("seed the parked rows");
    // Between the probe's read and the sweep, another step takes one down.
    h.sql_at_phase = Some((
        "swept".to_string(),
        "DELETE FROM state WHERE key = 'pending:c-gone'".to_string(),
    ));
    h.fire(&order);
    assert!(h.sql_at_phase.is_none(), "the sweep ran: {:?}", h.stderr);
    assert!(!has(&h, "pending:c-old"), "the old row is swept");
    let swept = marks(&h, "pending_swept");
    assert_eq!(swept.len(), 1, "{swept:?} {:?}", h.stderr);
    let value: Value = meclaw_core::serde_json::from_str(&swept[0]).expect("canonical JSON");
    assert_eq!(
        value,
        json!({"keys": ["pending:c-old"], "older_than_s": 600}),
        "only the row that went"
    );
}

#[test]
fn a_refused_read_of_the_parked_rows_is_said_and_the_rebuild_runs() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "keep_recent", json!(1))]);
    let order = strike(&mut h);
    h.ledger_may_refuse = true;
    h.refuse_legs = vec![("p-pend".to_string(), "store_error".to_string())];
    h.fire(&order);
    assert!(h.refuse_legs.is_empty(), "the read was refused");
    assert!(
        marks(&h, "store_error")
            .iter()
            .any(|v| v.contains("\"step\":\"rb-probe\"") && v.contains("p-pend")),
        "{:?} {:?}",
        marks(&h, "store_error"),
        h.stderr
    );
    assert_eq!(h.summ.len(), 1, "the rebuild runs: {:?}", h.stderr);
}

#[test]
fn the_sweep_age_has_a_floor_of_a_minute() {
    if !shipped() {
        return;
    }
    for (set, want) in [
        (json!(5), 60),
        (json!("59"), 60),
        (json!(60), 60),
        (json!(900), 900),
        (json!(null), 600),
        (json!(""), 600),
        (json!("soon"), 600),
    ] {
        let (got, err) = policy_scope(
            json!({"pending_sweep_s": set}),
            "PENDING_SWEEP_S",
            json!(null),
        );
        assert_eq!(got, json!(want), "pending_sweep_s {set}: {err}");
    }
}

#[test]
fn a_model_pin_is_one_row_per_hash_source_and_audience() {
    if !shipped() {
        return;
    }
    let h = Hive::new();
    let a = round_canon(ROUND_E).expect("the round");
    let b = "[\"member:b\"]".to_string();
    let args = json!([
        ["h-1", a],
        ["h-1", a],
        ["h-1", null],
        ["h-1", null],
        ["h-1", b],
    ]);
    let (ops, err) = policy_scope(
        json!({}),
        "[pin_op(0, x[0], x[1], '2026-10-16T00:00:00.000000Z')[1] for x in ARGS]",
        args,
    );
    for op in ops.as_array().unwrap_or_else(|| panic!("{ops} {err}")) {
        meclaw_cells::store::ops::dispatch(&h.db, op).unwrap_or_else(|e| panic!("{op}: {e}"));
    }
    let auds: Vec<Value> = h
        .rows("SELECT audience_set FROM pins WHERE hash = 'h-1' AND source = 'model' ORDER BY audience_set")
        .into_iter()
        .map(|r| r[0].clone())
        .collect();
    assert_eq!(
        auds,
        vec![Value::Null, json!(b), json!(a)],
        "the same pin twice is one row, NULL included (`is_null`); another round's is its own"
    );
}
