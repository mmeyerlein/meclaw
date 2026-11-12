//! GH #1462 (D58 snap) -- `snapshot_window` off is byte-identical.
//!
//! WHY this file exists: the `sent` list is a record, never a part of the
//! window. Off (the shipped "0") the curator writes what it wrote before --
//! the snapshot row without a `sent` value, no block more -- and on or off
//! the model is sent the same bytes. The arm with the knob on is the
//! control: without it the comparison would pass on a tree that knows no
//! such knob.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// Two turns and a third call with a bridge, in a hive with `over`; the
/// calls of the third round.
fn run(over: &[(&str, &str, Value)]) -> (Hive, Msg, Msg) {
    let mut h = Hive::with(over);
    turn(
        &mut h,
        "s1",
        "t1",
        "first question",
        "first answer",
        json!({}),
    );
    turn(
        &mut h,
        "s1",
        "t2",
        "second question",
        "second answer",
        json!({}),
    );
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": "t3", "iter": "0", "phase": "",
                     "path": "complex", "needs": "", "families": "web"});
    let ctx = json!({"session_id": "s1", "turn_id": "t3", "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "complex"});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("plan my trip to Rome")], "system": mode("Be brief.")}),
    );
    let (call, bridge) = calls_out(&h);
    let bridge = bridge.unwrap_or_else(|| panic!("a bridge: {:?} {:?}", h.out, h.stderr));
    (h, call, bridge)
}

/// The snapshot rows a hive wrote, as the store was asked to write them.
fn snapshot_inserts(h: &Hive) -> Vec<Value> {
    h.ledger_ops
        .iter()
        .map(|(_, op)| op.clone())
        .filter(|op| op["operation"] == json!("insert") && op["table"] == json!("snapshots"))
        .collect()
}

fn count(h: &Hive, table: &str) -> Option<i64> {
    h.rows(&format!("SELECT count(*) FROM {table}"))[0][0].as_i64()
}

#[test]
fn off_writes_what_was_written_before_and_on_sends_the_same_bytes() {
    if !shipped() {
        return;
    }
    let (plain, p_call, p_bridge) = run(&[]);
    let (off, o_call, o_bridge) = run(&[("policy", "snapshot_window", json!("0"))]);
    let (on, n_call, n_bridge) = run(&[("policy", "snapshot_window", json!("1"))]);

    // The window never depends on the knob.
    for (name, c) in [("off", &o_call), ("on", &n_call)] {
        assert_eq!(c.body, p_call.body, "the call with the knob {name}");
    }
    for (name, c) in [("off", &o_bridge), ("on", &n_bridge)] {
        assert_eq!(c.body, p_bridge.body, "the bridge with the knob {name}");
    }

    // Off is the hive of before: no list, no key, no block more.
    for (name, h, call, bridge) in [
        ("unset", &plain, &p_call, &p_bridge),
        ("off", &off, &o_call, &o_bridge),
    ] {
        assert!(
            sent_of(h, call).is_none(),
            "{name}: the call's row has no list"
        );
        assert!(
            sent_of(h, bridge).is_none(),
            "{name}: the bridge's row has no list"
        );
        let rows = snapshot_inserts(h);
        assert_eq!(rows.len(), 4, "{name}: three calls and a bridge: {rows:?}");
        for op in &rows {
            assert!(
                op["row"].get("sent").is_none(),
                "{name}: the row is written as before, without the key: {op}"
            );
        }
    }
    for table in ["blocks", "snapshots", "call_blocks", "wall"] {
        assert_eq!(count(&off, table), count(&plain, table), "{table}");
    }
    assert_eq!(
        off.ledger_ops.len(),
        plain.ledger_ops.len(),
        "off runs no ledger operation more"
    );

    // The control: on, the same run leaves the list -- for both calls.
    assert!(
        sent_of(&on, &n_call).is_some(),
        "with `snapshot_window` \"1\" the call's row carries `sent`"
    );
    assert!(
        sent_of(&on, &n_bridge).is_some(),
        "with `snapshot_window` \"1\" the bridge's row carries `sent`"
    );
    assert_eq!(
        count(&on, "wall"),
        count(&plain, "wall"),
        "the wall is not touched"
    );
    assert_eq!(
        count(&on, "snapshots"),
        count(&plain, "snapshots"),
        "on writes no row more, the list stands in the call's own row"
    );
}
