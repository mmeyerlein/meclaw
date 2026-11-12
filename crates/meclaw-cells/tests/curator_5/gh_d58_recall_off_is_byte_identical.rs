//! GH #1466 (D58 recall) -- `recall_skip_sure` off is byte-identical.
//!
//! WHY this file exists: the rule decides whether a block enters the window,
//! and it does that only for a hive that turned the knob on. Shipped "0", a
//! turn whose stamp is the surest "no" -- yes-values on the seam and all --
//! gets the window of before: the fetched recall on every call, no mark, no
//! element left out in the snapshot, the plan with its recall block, not one
//! ledger operation more. Until a recall bench has measured the bound, the
//! knob stays off, so this is the state every colony runs. The arm with the
//! knob on is the control: without it the comparison would pass on a tree
//! that knows no such knob.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

/// The collector's own recall pair: the id form `call_recall_` plus 16 hex is
/// what the curator reads as "fetched for the turn, not asked by the model".
const AMBIENT_ID: &str = "call_recall_0000000000d58ca1";
const WORDS: &str = "I just got back from the lake.";
const THE_ROW: &str = "My sister Anna moved to Hamburg.";
const KNOB: &str = "recall_skip_sure";

fn said_before() -> Value {
    tool_result(AMBIENT_ID, &format!("WHAT WAS SAID\n  user: \"{THE_ROW}\""))
}

/// The person's words with the recall the collector fetched beside them.
fn with_recall() -> Value {
    json!([
        user(WORDS),
        tool_call(AMBIENT_ID, "memory_recall"),
        said_before()
    ])
}

/// The stamp keys of a judged `talk` turn as the seam carries them: no
/// `memory` among the needs, and the yes-values of the needs as the JSON text
/// of an object (`needs_yes`), `memory` at `yes_memory`.
fn judged(yes_memory: f64) -> Value {
    json!({"path": "talk", "needs": "", "families": "",
           "needs_yes": json!({"memory": yes_memory, "tool": 0.1}).to_string()})
}

/// One call of `turn`/`iter` on `in_curate` with the stamp keys `stamp` on the
/// hop; returns the call that left for the model.
fn call(h: &mut Hive, turn: &str, iter: u32, stamp: &Value, msgs: Value) -> Msg {
    let mut hop = json!({"session_id": "s1", "turn_id": turn, "iter": iter.to_string(),
                         "phase": ""});
    let mut ctx = json!({"session_id": "s1", "turn_id": turn, "iter": iter.to_string(),
                         "channel": "test", "audience_set": ROUND_E});
    for (k, v) in stamp.as_object().expect("the stamp keys are an object") {
        hop[k] = v.clone();
    }
    if let Some(path) = stamp.get("path") {
        ctx["path"] = path.clone();
    }
    h.out.clear();
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": msgs, "system": mode("Be brief.")}),
    );
    let brain = h.routed("brain");
    assert_eq!(brain.len(), 1, "{:?} {:?}", h.out, h.stderr);
    brain[0].clone()
}

fn text(c: &Msg) -> String {
    sj::to_string(&c.messages()).expect("messages")
}

/// The fetched recall is out of the window, the pair whole.
fn left_out(c: &Msg) -> bool {
    let t = text(c);
    t.contains(WORDS) && !t.contains(THE_ROW) && !t.contains(AMBIENT_ID)
}

/// The fetched recall is in the window the call sends.
fn carries(c: &Msg) -> bool {
    text(c).contains(THE_ROW)
}

/// The `recall_skipped` marks of the ledger: `(turn, value)`.
fn skip_marks(h: &Hive) -> Vec<(String, Value)> {
    h.rows("SELECT turn_id, value FROM marks WHERE kind = 'recall_skipped' ORDER BY seq")
        .iter()
        .map(|r| {
            (
                r[0].as_str().unwrap_or_default().to_string(),
                sj::from_str(r[1].as_str().unwrap_or("{}")).expect("the mark is JSON"),
            )
        })
        .collect()
}

/// Two turns under a sure-no stamp, the first with a fetched recall. Returns
/// the hive and the calls of `t1` and `t2`.
fn run(over: &[(&str, &str, Value)]) -> (Hive, Vec<Msg>) {
    let mut all = vec![("policy", "snapshot_window", json!("1"))];
    all.extend(over.iter().cloned());
    let mut h = Hive::with(&all);
    let c1 = call(&mut h, "t1", 0, &judged(0.05), with_recall());
    h.tap(&c1, "stop", json!({}), json!([said("Nice.")]));
    let c2 = call(
        &mut h,
        "t2",
        0,
        &judged(0.05),
        json!([user("And the boat?")]),
    );
    (h, vec![c1, c2])
}

fn cuts(c: &Msg) -> Value {
    c.hop.get("cuts").cloned().unwrap_or(Value::Null)
}

/// The kinds of the plan's blocks, in their order.
fn plan_kinds(h: &Hive) -> Vec<String> {
    plan_blocks(h, ROUND_E)
        .iter()
        .map(|b| b["kind"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn gh_d58_recall_off_is_byte_identical() {
    if !shipped() {
        return;
    }
    let policy = cell_config("policy");
    assert_eq!(policy["params"][KNOB], json!("0"), "ships off");
    assert_eq!(
        policy["contract"]["settings"][KNOB]["default"],
        json!("0"),
        "the setting says the same"
    );

    let (plain, p) = run(&[]);
    let (off, o) = run(&[("policy", KNOB, json!("0"))]);
    let (on, n) = run(&[("policy", KNOB, json!("1"))]);

    for (i, name) in ["t1", "t2"].iter().enumerate() {
        // Off is the hive that never heard of the knob, byte for byte.
        assert_eq!(o[i].body, p[i].body, "the call of {name}");
        assert_eq!(cuts(&o[i]), cuts(&p[i]), "the cuts of {name}");
        assert_eq!(
            sent_of(&off, &o[i]),
            sent_of(&plain, &p[i]),
            "the list `sent` of {name}"
        );
        // And it is the window of before: the recall on every call, nothing
        // left out, nothing said about it.
        assert!(
            carries(&o[i]),
            "{name}: off, the recall stays: {}",
            text(&o[i])
        );
        assert!(
            !cuts(&o[i]).to_string().contains(KNOB),
            "{name}: no cut of this kind on the hop: {}",
            cuts(&o[i])
        );
        let sent = sent_of(&off, &o[i]).expect("the snapshot row carries `sent`");
        let w = sent_window(&off, &sent);
        assert!(
            w.left_out.is_empty(),
            "{name}: off, the list names no element left out: {sent:?}"
        );
        assert_eq!(
            w.messages,
            o[i].messages(),
            "{name}: the list is the window"
        );
    }
    assert_eq!(
        skip_marks(&off),
        Vec::<(String, Value)>::new(),
        "off writes no mark"
    );
    assert_eq!(plan_kinds(&off), plan_kinds(&plain), "the plan of before");
    assert!(
        plan_kinds(&off).iter().any(|k| k == "recall"),
        "off, the plan keeps the recall block: {:?}",
        plan_kinds(&off)
    );
    for table in ["blocks", "snapshots", "call_blocks", "wall", "marks"] {
        let count = |h: &Hive| h.rows(&format!("SELECT count(*) FROM {table}"))[0][0].as_i64();
        assert_eq!(count(&off), count(&plain), "{table}");
    }
    assert_eq!(
        off.ledger_ops.len(),
        plain.ledger_ops.len(),
        "off runs no ledger operation more"
    );

    // The control: on, the same run leaves the fetched recall out.
    assert!(
        left_out(&n[0]),
        "with `recall_skip_sure` \"1\" the recall leaves the call of t1: {}",
        text(&n[0])
    );
    assert_ne!(n[0].body, o[0].body, "on and off differ");
    assert_eq!(skip_marks(&on).len(), 1, "on, the skip is marked");
}
