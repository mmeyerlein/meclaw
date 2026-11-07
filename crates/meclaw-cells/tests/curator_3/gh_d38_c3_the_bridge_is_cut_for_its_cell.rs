//! GH #1450 (D38 C3b, OR-d38-3.o15.3) -- the bridge is a call of the cut too.
//!
//! WHY this file exists: the bridge (GH #1248) is a model call of its own --
//! its own `calls` row, its own cell (`./quick_bridge` for a speculating
//! talky, GH #1398, else `./quick`). Review o15-R1 I3 found it cut outside
//! the one stage 2 (`quick_items` straight in `win`), without a snapshot and
//! without its catalogue question, so the fourth talky edge
//! (`./curator -> ./quick_bridge` on `catalog_ask`) never carried a message.
//! One place cuts (T3-core § C3b): the bridge's window is the cut's, its
//! snapshot names target, ids and dropped blocks like every other call, and
//! with `catalog_ask` on the curator asks its cell for the row.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

fn hive(spec: bool, ask: bool) -> Hive {
    let mut over = vec![("policy", "role", json!("talky"))];
    if spec {
        over.push(("lead", "quick_speculative", json!("1")));
        over.push(("lead", "spec_tools", json!("web,weather")));
    }
    if ask {
        over.push(("policy", "catalog_ask", json!("1")));
    }
    Hive::with(&over)
}

fn spec_call(h: &mut Hive, turn: &str, text: &str) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": "", "spec": "1"});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": ""});
    h.lane("in_curate", ctx, hop, json!({"messages": [user(text)]}));
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
fn a_speculating_bridge_is_cut_for_quick_bridge() {
    if !shipped() {
        return;
    }
    let mut h = hive(true, false);
    spec_call(&mut h, "t1", "plan my trip to Rome");
    seam(&mut h, "t1", "complex", "plan my trip to Rome");
    let b = bridge(&h);
    assert_eq!(b.hop["spec"], json!("bridge"), "{:?}", b.hop);
    let cut = cut_of(&h, &b);
    assert_eq!(cut["target"], json!("quick_bridge"), "{cut}");
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
fn a_bridge_without_speculation_is_cut_for_quick() {
    if !shipped() {
        return;
    }
    let mut h = hive(false, false);
    seam(&mut h, "t1", "complex", "plan my trip to Rome");
    let cut = cut_of(&h, &bridge(&h));
    assert_eq!(cut["target"], json!("quick"), "{cut}");
}

#[test]
fn the_curator_asks_the_bridge_cell_for_its_row() {
    if !shipped() {
        return;
    }
    let mut h = hive(true, true);
    spec_call(&mut h, "t1", "plan my trip to Rome");
    seam(&mut h, "t1", "complex", "plan my trip to Rome");
    let asks: Vec<Value> = h
        .routed("catalog_ask")
        .iter()
        .map(|m| m.hop["catalog_for"].clone())
        .collect();
    assert!(
        asks.contains(&json!("quick_bridge")),
        "the bridge's cell is asked for its row once: {asks:?}"
    );
    assert_eq!(
        h.state("catalog_asked:quick_bridge"),
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
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "snapshot_keep", json!(1)),
    ]);
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
