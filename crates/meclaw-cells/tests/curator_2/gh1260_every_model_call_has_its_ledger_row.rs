//! GH #1260 -- every model call has its ledger row (lock W6).
//!
//! WHY this file exists: the ledger is where a turn's ways are counted (lab
//! `ledger_stats.py`, the `route` mark beside a `calls` row). Every window the
//! curator builds wrote its `calls` row (`trigger: curate`), but the bridge of
//! a `complex`/`research` errand -- a model call of its own on `./quick`,
//! `curator_call` = `<call>-bridge` -- wrote none, so the tap of its answer
//! updated nothing and its tokens fell out of the count. This lock walks every
//! way (talk -> brain, simple -> quick, complex -> consult + bridge, research
//! -> research + bridge, an escalation quick -> brain): each call that leaves
//! the curator has its `calls` row, each window row its `route` mark with path
//! and way, and the tap of the bridge books its usage on its own row.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const WAYS: [&str; 4] = ["brain", "quick", "consult", "research"];

fn curate_on(h: &mut Hive, turn_id: &str, iter: &str, path: &str, ask: &str) -> Vec<Msg> {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn_id, "iter": iter, "phase": "",
                     "path": path, "families": ""});
    let ctx = json!({"session_id": "s1", "turn_id": turn_id, "iter": iter,
                     "channel": "test", "audience_set": ROUND_E, "path": path});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(ask)], "system": mode("Be brief.")}),
    );
    h.out
        .iter()
        .filter(|m| WAYS.contains(&m.route()))
        .cloned()
        .collect()
}

fn call_row(h: &Hive, call: &str) -> Vec<Value> {
    let rows = h.rows(&format!(
        "SELECT trigger, tokens_prompt, model FROM calls WHERE call_id = '{call}'"
    ));
    assert_eq!(rows.len(), 1, "call {call} has one calls row: {rows:?}");
    rows[0].clone()
}

fn route_mark(h: &Hive, call: &str) -> Value {
    h.rows("SELECT value FROM marks WHERE kind = 'route'")
        .iter()
        .filter_map(|r| meclaw_core::serde_json::from_str::<Value>(r[0].as_str()?).ok())
        .find(|m| m["call"] == json!(call))
        .unwrap_or_else(|| panic!("call {call} has no route mark"))
}

#[test]
fn every_model_call_has_its_ledger_row() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "consult_pending_ms", json!(86_400_000)),
    ]);
    turn(&mut h, "s1", "t0", "hello", "hi", json!({}));

    // (turn, iter, path, the ways that leave, the way of the window)
    let cases = [
        ("t1", "0", "talk", vec!["brain"], "brain"),
        ("t2", "0", "simple", vec!["quick"], "quick"),
        ("t3", "0", "complex", vec!["consult", "quick"], "consult"),
        ("t4", "0", "research", vec!["research", "quick"], "research"),
        // Past `quick_max_iter` the quick voice gives the turn to the brain.
        ("t5", "3", "simple", vec!["brain"], "brain"),
    ];
    let mut bridge = None;
    for (turn_id, iter, path, ways, window_way) in cases {
        let out = curate_on(&mut h, turn_id, iter, path, "a question");
        let mut got: Vec<&str> = out.iter().map(Msg::route).collect();
        got.sort_unstable();
        let mut want = ways.clone();
        want.sort_unstable();
        assert_eq!(got, want, "{path}: the calls that leave: {:?}", h.stderr);
        for m in &out {
            let call = m.hop["curator_call"].as_str().unwrap_or_default();
            assert!(!call.is_empty(), "{path}: every call is named: {:?}", m.hop);
            let row = call_row(&h, call);
            if m.hop.get("bridge").is_some() {
                assert_eq!(row[0], json!("bridge"), "{path}: the bridge's own row");
                bridge = Some(m.clone());
            } else {
                assert_eq!(row[0], json!("curate"), "{path}: the window's row");
                let mark = route_mark(&h, call);
                assert_eq!(mark["path"], json!(path), "{path}: {mark}");
                assert_eq!(mark["way"], json!(window_way), "{path}: {mark}");
            }
        }
    }

    let b = bridge.expect("a bridge left");
    let call = b.hop["curator_call"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    h.tap(
        &b,
        "stop",
        json!({"tokens_prompt": 42, "model": "quick-model"}),
        json!([said("One moment, I am on it.")]),
    );
    let row = call_row(&h, &call);
    assert_eq!(
        row[1],
        json!(42),
        "the bridge's tap books its usage: {row:?}"
    );
    assert_eq!(row[2], json!("quick-model"), "{row:?}");
}
