//! GH #1450 (D38 C3b, R-38-4 Nr. 3) -- the cut never adds.
//!
//! WHY this file exists: stage 2 of the curator turns the plan (stage 1, the
//! building blocks of the window) into the window of ONE target model --
//! budget, order, cache boundary, tool scope -- and may only cut and order,
//! never add (R-38-4 Nr. 3). Whatever a call sends is a block of its plan; a
//! cut that brought content of its own would be a second optimiser with its
//! own sources behind the one curator (R-38-4 Nr. 1).
//!
//! Read off what the call SENDS (review o15-R1 I2: the plan and the cut are
//! built from one list, so comparing the two proves nothing): every element
//! of `call.body` -- each system leaf as its `frame` block, each message as
//! `<kind>:<short id>` of itself or, formatted (session bound, person frame),
//! of the wall element whose text it carries -- is a block of the call's
//! plan (`snapshots.body`) and of its cut (`snapshots.cut.ids`), once; every
//! message block the cut names is one the call sends (review M2), in the
//! order it sends them (review M1).
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};
use std::collections::BTreeSet;

/// The cut stage 2 made for `call` (column `cut` of its snapshot row),
/// `None` while the ledger keeps no such column or no row of the call.
fn cut_of(h: &Hive, call: &Msg) -> Option<Value> {
    let has = h.rows("SELECT name FROM pragma_table_info('snapshots') WHERE name = 'cut'");
    if has.is_empty() {
        return None;
    }
    let id = call.hop["curator_call"].as_str().unwrap_or_default();
    let rows = h.rows(&format!("SELECT cut FROM snapshots WHERE call_id = '{id}'"));
    let text = rows.first()?.first()?.as_str()?.to_string();
    sj::from_str::<Value>(&text).ok()
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|x| x.as_str().unwrap_or_default().to_string())
        .collect()
}

/// One turn on `in_curate` stamped `path`; the call that left on `way`
/// (never a bridge), answered with `reply`.
fn turn_on(h: &mut Hive, turn: &str, path: &str, way: &str, ask: &str, reply: &str) -> Msg {
    turn_with(h, turn, path, way, json!([user(ask)]), reply)
}

/// `turn_on` with the turn's own `messages`.
fn turn_with(h: &mut Hive, turn: &str, path: &str, way: &str, msgs: Value, reply: &str) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": path, "needs": "", "families": ""});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "chat", "audience_set": ROUND_E, "path": path});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": msgs, "system": mode("Be brief.")}),
    );
    let call = h
        .routed(way)
        .into_iter()
        .find(|m| {
            m.hop.get("bridge").is_none()
                && !m.hop["curator_call"]
                    .as_str()
                    .unwrap_or("")
                    .ends_with("-bridge")
        })
        .unwrap_or_else(|| panic!("a call on {way}: {:?} {:?}", h.out, h.stderr));
    h.tap(&call, "stop", json!({}), json!([said(reply)]));
    call
}

/// The (path, text) leaves of a system tree, `$replace` markers skipped.
fn leaves(node: &Value, path: &str, out: &mut Vec<(String, String)>) {
    let Some(map) = node.as_object() else {
        return;
    };
    for (k, v) in map {
        if k == "$replace" {
            continue;
        }
        if k == "text" {
            if let Some(t) = v.as_str() {
                out.push((path.to_string(), t.to_string()));
            }
            continue;
        }
        let p = if path.is_empty() {
            k.clone()
        } else {
            format!("{path}.{k}")
        };
        leaves(v, &p, out);
    }
}

/// The plan id a sent message stands for: `<kind>:<short id>` of the element
/// itself, or of the wall element of `known` (same origin and type) whose
/// text it carries -- a message the window formatted.
fn message_id(
    el: &Value,
    kinds: &BTreeSet<String>,
    plan: &BTreeSet<String>,
    known: &[Value],
) -> Option<String> {
    let direct = kinds
        .iter()
        .map(|k| block_id(k, el))
        .find(|id| plan.contains(id));
    if direct.is_some() {
        return direct;
    }
    let text = el["text"].as_str().unwrap_or_default();
    known
        .iter()
        .filter(|w| w["origin"] == el["origin"] && w["type"] == el["type"])
        .filter(|w| {
            !w["text"].as_str().unwrap_or_default().is_empty()
                && text.contains(w["text"].as_str().unwrap_or_default())
        })
        .flat_map(|w| kinds.iter().map(move |k| block_id(k, w)))
        .find(|id| plan.contains(id))
}

fn assert_cut_within_plan(h: &Hive, call: &Msg, known: &[Value], what: &str) {
    let plan: BTreeSet<String> = block_ids(&snapshot_of(h, call).expect("the plan snapshot"))
        .into_iter()
        .collect();
    let kinds: BTreeSet<String> = plan
        .iter()
        .filter_map(|id| id.split(':').next().map(str::to_string))
        .collect();
    let cut = cut_of(h, call).unwrap_or_else(|| {
        panic!("{what}: the snapshot names no cut (column `cut` of `snapshots`)")
    });
    let ids = strs(&cut["ids"]);
    assert!(!ids.is_empty(), "{what}: the cut sends blocks: {cut}");
    let once: BTreeSet<&String> = ids.iter().collect();
    assert_eq!(once.len(), ids.len(), "{what}: every block once: {ids:?}");
    // The system part the call sends: each leaf a `frame` of the plan.
    let mut sys = Vec::new();
    leaves(&call.body["system"], "", &mut sys);
    for (path, text) in &sys {
        let id = block_id("frame", &json!({"path": path, "text": text}));
        assert!(
            plan.contains(&id) && ids.contains(&id),
            "{what}: the call sends the leaf {path}, which is no block of its plan/cut -- \
             stage 2 added: {id} plan {plan:?}"
        );
    }
    // The messages the call sends: each a block of the plan and of the cut.
    let mut sent = Vec::new();
    for el in call.messages() {
        let id = message_id(&el, &kinds, &plan, known).unwrap_or_else(|| {
            panic!(
                "{what}: the call sends {el}, which no block of its plan names -- \
                    stage 2 added: plan {plan:?}"
            )
        });
        assert!(
            ids.contains(&id),
            "{what}: {id} is sent but not in the cut: {cut}"
        );
        if !sent.contains(&id) {
            sent.push(id);
        }
    }
    // Review M2: a message block the cut names is one the call sends; M1:
    // in the order it sends them.
    let named: Vec<String> = ids
        .iter()
        .filter(|id| !id.starts_with("frame:") || sent.contains(id))
        .cloned()
        .collect();
    assert_eq!(
        named, sent,
        "{what}: the cut's message blocks are the ones sent, in their order: {cut}"
    );
}

#[test]
fn the_brain_window_holds_only_blocks_of_its_plan() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    let mut last = None;
    for i in 0..3 {
        last = Some(turn_on(
            &mut h,
            &format!("t{i}"),
            "talk",
            "brain",
            &format!("question {i}"),
            &format!("answer {i}"),
        ));
    }
    let known: Vec<Value> = (0..3)
        .flat_map(|i| [user(&format!("question {i}")), said(&format!("answer {i}"))])
        .collect();
    assert_cut_within_plan(&h, &last.expect("a call"), &known, "brain");
}

#[test]
fn the_quick_window_holds_only_blocks_of_its_plan() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    for i in 0..4 {
        turn_on(
            &mut h,
            &format!("t{i}"),
            "talk",
            "brain",
            &format!("question {i}"),
            &format!("answer {i}"),
        );
    }
    let call = turn_on(
        &mut h,
        "t9",
        "simple",
        "quick",
        "What day is it?",
        "Monday.",
    );
    let known: Vec<Value> = (0..4)
        .flat_map(|i| [user(&format!("question {i}")), said(&format!("answer {i}"))])
        .chain([user("What day is it?")])
        .collect();
    assert_cut_within_plan(&h, &call, &known, "quick");
}

/// Review m2 of c3b (OR-d38-3.o15.5): the ambient recall pair of a turn is
/// sent before the person's words (`ambient_first`), so the send order is
/// not the wall's; the cut names the blocks in the order they are sent
/// (`stages[3]`) -- without it this scenario is red.
#[test]
fn a_recall_turn_keeps_the_send_order_in_the_cut() {
    if !shipped() {
        return;
    }
    const RECALL: &str = "call_recall_00000000000000c3";
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    turn_on(&mut h, "t0", "talk", "brain", "question 0", "answer 0");
    let recall = "WHAT WAS SAID\n  user: \"An older turn.\"";
    let call = turn_with(
        &mut h,
        "t1",
        "talk",
        "brain",
        json!([
            user("question 1"),
            tool_call(RECALL, "memory_recall"),
            tool_result(RECALL, recall)
        ]),
        "answer 1",
    );
    let known = vec![
        user("question 0"),
        said("answer 0"),
        user("question 1"),
        tool_call(RECALL, "memory_recall"),
        tool_result(RECALL, recall),
    ];
    assert_cut_within_plan(&h, &call, &known, "recall");
}
