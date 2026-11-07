//! GH #1448 (D38 C2, R-38-4 Nr. 5, R-38-3) -- the snapshot lists the plan.
//!
//! WHY this file exists: "was the context there?" must be a lookup, not a
//! reconstruction. Per call the ledger keeps the plan's id list with prio and
//! tokens as one row (table `snapshots`, the body a blob), kept for the last
//! `snapshot_keep` calls of a round; the operator and the battery read it
//! through the hive's one audience-gated reader (`./reader`, `what`
//! `snapshots`) -- a round never sees the snapshot of another.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const ROUND_F: &str = "[\"member:f\"]";

fn turn_in(h: &mut Hive, round: &str, session: &str, ask: &str, reply: &str) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": session, "turn_id": "t1", "iter": "0", "phase": ""});
    let ctx = json!({"session_id": session, "turn_id": "t1", "iter": "0",
                     "channel": "test", "audience_set": round});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(ask)], "system": mode("Be brief.")}),
    );
    let call = h.routed("brain").first().cloned().expect("a call");
    h.tap(&call, "stop", json!({}), json!([said(reply)]));
    call
}

fn read_snapshots(h: &mut Hive, round: &str) -> Value {
    h.out.clear();
    h.lane(
        "in_read",
        json!({"audience_set": round}),
        json!({"read_tag": "snap-1"}),
        json!({"what": "snapshots"}),
    );
    let read = h.routed("read");
    assert_eq!(read.len(), 1, "one answer: {:?} {:?}", h.out, h.stderr);
    assert_eq!(
        read[0].hop["error_code"],
        json!(""),
        "an answer: {:?}",
        read[0].hop
    );
    read[0].body["rows"].clone()
}

#[test]
fn every_call_leaves_its_plan_ids_with_prio_and_tokens() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    let call = turn_in(&mut h, ROUND_E, "s1", "q1", "a1");
    let snap = snapshot_of(&h, &call).expect("one snapshot row of the call");
    assert!(!snap.is_empty(), "the call's plan is listed");
    for b in &snap {
        assert!(b["id"].is_string(), "{b}");
        assert!(b["prio"].is_i64(), "{b}");
        assert!(b["tokens"].is_i64(), "{b}");
    }
    assert!(
        block_ids(&snap).contains(&block_id("words", &user("q1"))),
        "the person's words are a block of the call: {snap:?}"
    );
}

#[test]
fn the_reader_shows_a_round_its_own_snapshots_only() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    let e = turn_in(&mut h, ROUND_E, "s1", "q in e", "a in e");
    let f = turn_in(&mut h, ROUND_F, "s2", "q in f", "a in f");
    let id = |c: &Msg| c.hop["curator_call"].clone();
    for (round, own, other) in [(ROUND_E, &e, &f), (ROUND_F, &f, &e)] {
        let rows = read_snapshots(&mut h, round);
        let calls: Vec<Value> = rows
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|r| r["call_id"].clone())
            .collect();
        assert!(
            calls.contains(&id(own)),
            "{round} reads its own call: {rows}"
        );
        assert!(
            !calls.contains(&id(other)),
            "{round} never reads another round's call: {rows}"
        );
        let mine = rows
            .as_array()
            .and_then(|a| a.iter().find(|r| r["call_id"] == id(own)))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            mine["blocks"],
            json!(snapshot_of(&h, own).expect("the row")),
            "the reader hands the snapshot on as stored: {mine}"
        );
    }
}

#[test]
fn the_snapshot_keep_is_a_knob_with_its_preset() {
    if !shipped() {
        return;
    }
    let cfg = cell_config("policy");
    let v = &cfg["params"]["snapshot_keep"];
    assert!(
        v.as_i64().is_some_and(|n| n > 0),
        "policy declares snapshot_keep with a preset: {v}"
    );
    assert_eq!(
        &cfg["contract"]["settings"]["snapshot_keep"]["default"], v,
        "the setting's default is the param"
    );
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "snapshot_keep", json!(2)),
    ]);
    for i in 0..4 {
        let t = format!("t{i}");
        let call = h.curate(
            "s1",
            &t,
            0,
            json!([user(&format!("q{i}"))]),
            mode("Be brief."),
        );
        h.tap(&call, "stop", json!({}), json!([said(&format!("a{i}"))]));
    }
    let n = h.rows("SELECT count(*) FROM snapshots")[0][0].as_i64();
    assert_eq!(n, Some(2), "the round keeps its last two snapshots");
}

/// OR-d38-3.o10.2 (review I1): the snapshot lists every block the call sends
/// -- the system part as `frame`, the state line as `state`, a note as
/// `note` -- and a push line (a candidates pair) is a `push` block, never a
/// `tool_result`; the text sent is the text of before.
#[test]
fn the_snapshot_lists_the_system_part_the_state_line_and_the_notes() {
    if !shipped() {
        return;
    }
    const PUSH_ID: &str = "call_candidates_00000000000000c1";
    const PUSH_TEXT: &str = "[addendum -- notes handed to this conversation for this round; not said by the person]\n[Bring the umbrella.]";
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "state_block", json!("1")),
    ]);
    turn(
        &mut h,
        "s1",
        "t1",
        "We planted tomatoes.",
        "Lovely.",
        json!({}),
    );
    h.mark(
        backdated(0),
        "s1",
        "t1",
        "topic",
        &canonical(&json!({"movement": "start", "name": "the garden"})),
    );
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": "t2", "iter": "0", "phase": ""});
    let ctx = json!({"session_id": "s1", "turn_id": "t2", "iter": "0",
                     "channel": "test", "audience_set": ROUND_E,
                     "turn_notes": [{"id": "n1", "source": "app", "text": "The person is outside."}]});
    let msgs = json!([
        user("And now?"),
        tool_call(PUSH_ID, "memory_recall"),
        tool_result(PUSH_ID, PUSH_TEXT)
    ]);
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": msgs, "system": mode("Be brief.")}),
    );
    let call = h.routed("brain").first().cloned().expect("a call");
    let snap = snapshot_of(&h, &call).expect("one snapshot row of the call");
    let ids = block_ids(&snap);
    for kind in ["frame:", "state:", "note:"] {
        assert!(
            ids.iter().any(|i| i.starts_with(kind)),
            "the call's blocks hold a {kind} block: {ids:?}"
        );
    }
    assert!(
        ids.contains(&block_id("push", &tool_result(PUSH_ID, PUSH_TEXT))),
        "a push line is a push block: {ids:?}"
    );
}
