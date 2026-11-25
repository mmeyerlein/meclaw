//! GH #1450 (D38 C3b, OR-d38-3.o15.3) -- the bridge is a call of the cut too.
//!
//! WHY this file exists: the bridge (GH #1248) is a model call of its own --
//! its own `calls` row, the cell `bridge_target` names (`./quick` in
//! talky; the `./quick_bridge` of a speculating talky left with the
//! speculation, d54s specaus GH #1584). Review o15-R1 I3 found it cut outside
//! the one stage 2 (`fast_items` straight in `win`), without a snapshot and
//! without its catalogue question.
//! One place cuts (T3-core § C3b): the bridge's window is the cut's, its
//! snapshot names target, ids and dropped blocks like every other call, and
//! while its cell has not reported (the ask is no knob since D58 target,
//! OR-d58-2.target.15) the curator asks its cell for the row.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

fn hive(ask: bool) -> Hive {
    let h = Hive::with(&[]);
    // `ask`: no model cell has reported its package yet, so the curator
    // asks; else the composite's cells reported (contract point 13).
    if ask {
        h
    } else {
        h.reporting_composite("talky")
    }
}

fn seam(h: &mut Hive, turn: &str, path: &str, text: &str) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": path, "needs": "", "families": "web"});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": path});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(text)], "system": mode("Be brief.")}),
    );
}

fn bridge(h: &Hive) -> Msg {
    h.routed("quick")
        .into_iter()
        .find(|m| {
            m.hop["curator_call"]
                .as_str()
                .unwrap_or("")
                .ends_with("-bridge")
        })
        .unwrap_or_else(|| panic!("a bridge: {:?} {:?}", h.out, h.stderr))
}

fn cut_of(h: &Hive, call: &Msg) -> Value {
    let id = call.hop["curator_call"].as_str().unwrap_or_default();
    let rows = h.rows(&format!("SELECT cut FROM snapshots WHERE call_id = '{id}'"));
    let text = rows
        .first()
        .and_then(|r| r.first())
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    sj::from_str::<Value>(&text)
        .unwrap_or_else(|_| panic!("the bridge {id} has a snapshot with its cut: {rows:?}"))
}

#[test]
fn a_bridge_is_cut_for_quick() {
    if !shipped() {
        return;
    }
    let mut h = hive(false);
    seam(&mut h, "t1", "complex", "plan my trip to Rome");
    let b = bridge(&h);
    assert_eq!(b.hop["target"], json!("quick"), "{:?}", b.hop);
    let cut = cut_of(&h, &b);
    assert_eq!(cut["target"], json!("quick"), "{cut}");
    let ids: Vec<String> = cut["ids"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|x| x.as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        ids.contains(&block_id("words", &user("plan my trip to Rome"))),
        "the bridge's snapshot lists the person's words it sends: {cut}"
    );
    assert_eq!(cut["tools"], json!([]), "the bridge calls no tool: {cut}");
}

#[test]
fn the_curator_asks_the_bridge_cell_for_its_row() {
    if !shipped() {
        return;
    }
    let mut h = hive(true);
    // The session's first call asks every cell at once (`catalog_ask`
    // without `catalog_for`); the bridge of the next turn asks its own cell.
    seam(&mut h, "t0", "talk", "hello");
    assert_eq!(h.routed("catalog_ask").len(), 1, "{:?}", h.out);
    seam(&mut h, "t1", "complex", "plan my trip to Rome");
    let asks: Vec<Value> = h
        .routed("catalog_ask")
        .iter()
        .map(|m| m.hop.get("catalog_for").cloned().unwrap_or_default())
        .collect();
    assert!(
        asks.contains(&json!("quick")),
        "the bridge's cell is asked for its row once: {asks:?}"
    );
    assert_eq!(
        // D58 target: the key of a target's state is `catalog:<target>:<field>`.
        h.state("catalog:quick:asked"),
        "s1",
        "the question is marked for the session"
    );
}

/// Review m6 of c3b (OR-d38-3.o15.5): a call with a bridge writes two
/// snapshot rows; with `snapshot_keep` 1 the round still keeps exactly one.
#[test]
fn snapshot_keep_one_with_a_bridge_keeps_one() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "snapshot_keep", json!(1))]).reporting_composite("talky");
    for t in ["t1", "t2", "t3"] {
        seam(&mut h, t, "complex", "plan my trip to Rome");
        bridge(&h);
    }
    let n = h.rows("SELECT count(*) FROM snapshots")[0][0].as_i64();
    assert_eq!(
        n,
        Some(1),
        "the round keeps snapshot_keep rows, bridge or not"
    );
}
