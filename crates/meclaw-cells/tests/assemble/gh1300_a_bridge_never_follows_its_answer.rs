//! GH #1300 (OR-EF-22): a tool round's bridge is said at most once and never
//! after its turn's answer.
//!
//! The curator sends the bridge to `./quick` beside the required call, so the
//! two race. When the call comes back without its call, the round goes to the
//! brain, and the brain's answer reached the reply exit before the bridge in
//! the export-tree run of the 0.70.6 release gate. Both meet in
//! `collector/assemble`, and there they meet in the store: the bridge reads its
//! round back, the answer drops the round, and `./window` answers both in the
//! order they came. Each carries its sentence through the store in `hop.gate`
//! (back as `context.col_gate`), so a store that refuses the op never takes the
//! answer with it. These pins drive the shipped script one message at a time
//! with the store's replies written out, so the order "answer first" is
//! deterministic here.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::*;
use serde_json::{Value, json};

const T: &str = "turn-1300";
const BRIDGE: &str = "One moment, I am looking.";
const ANSWER: &str = "The brain looked: rain in Hamburg tomorrow.";

fn text(m: &str) -> Value {
    json!([{"origin": "assistant", "type": "text", "text": m}])
}

/// The bridge's sentence as the dispatcher hands it on: interim, under the
/// curator's `<call>-bridge`, in a stamped turn. D58 moveout (contract point
/// 3): the composite lifts the call's kind (`hop.call_kind` "side") into the
/// context, and that field -- not the ending of the id -- makes it a bridge.
fn bridge_in() -> Value {
    lane(
        "in_answer",
        json!({"interim": "1", "finish_reason": "stop"}),
        json!({"turn_id": T, "iter": "0", "path": "simple", "curator_call": "c-9-bridge",
               "call_kind": "side"}),
        text(BRIDGE),
    )
}

/// The brain's answer for the call the quick voice gave up: its kind
/// `fallback` in the context (D58 moveout, contract point 3; the id keeps its
/// ending `-esc`, nobody reads it).
fn escalated_answer_in() -> Value {
    lane(
        "in_answer",
        json!({"finish_reason": "stop"}),
        json!({"turn_id": T, "iter": "0", "path": "simple", "curator_call": "c-9-esc",
               "call_kind": "fallback"}),
        text(ANSWER),
    )
}

fn row(role: &str) -> Value {
    json!({"role": role})
}

/// What a gate carries through the store: the sentence and its marks.
fn gate(m: &str, mark: Value) -> String {
    json!({"messages": text(m), "mark": mark}).to_string()
}

fn gate_of(msg: &Value) -> Value {
    serde_json::from_str(&hop_str(msg, "gate")).expect("gate json")
}

/// The bridge gate's reply: ONE select, the round's rows, the bridge in context.
fn bridge_reply(rows: Value, hop: Value) -> Value {
    let mut h = json!({"operation": "select", "rows_affected": 0});
    for (k, v) in hop.as_object().expect("hop object") {
        h[k] = v.clone();
    }
    json!({
        "header": {"context": {"session_id": SESSION, "turn_id": T, "iter": "0",
                               "col_phase": "bridge-gate", "store_origin": "collector",
                               "col_gate": gate(BRIDGE, json!({"interim": "1",
                                                                "finish_reason": "stop"}))},
                   "hop": h},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "c-bridge-gate",
                      "text": rows.to_string()}]
    })
}

/// The answer gate's reply: the round's drop, the answer in context.
fn answer_reply() -> Value {
    let mut r = bundle_reply(
        "answer-gate",
        T,
        &[("c-drop-turns", json!([])), ("c-drop-round", json!([]))],
    );
    r["header"]["context"]["col_gate"] = json!(gate(
        ANSWER,
        json!({"finish_reason": "stop", "truncated": ""})
    ));
    r
}

fn answers(out: &[Value]) -> Vec<&Value> {
    out.iter()
        .filter(|m| m["header"]["route"].as_str() == Some("answer"))
        .collect()
}

#[test]
fn an_answer_that_may_follow_a_bridge_leaves_through_the_store() {
    let out = assemble(&[], escalated_answer_in());
    assert!(answers(&out).is_empty(), "not at once: {out:?}");
    let g = in_phase(&out, "answer-gate");
    let ids: Vec<String> = calls_of(g).into_iter().map(|(id, _)| id).collect();
    assert_eq!(
        ids,
        ["c-said-read", "c-said-row", "c-drop-turns", "c-drop-round"],
        "the round dropped -- the one op the bridge's read queues behind -- with \
         the turn's `said` rows read and the answer's own written in front (GH #1505)"
    );
    let kept = gate_of(g);
    assert_eq!(kept["messages"][0]["text"], ANSWER);
    assert_eq!(kept["mark"]["finish_reason"], "stop");
}

/// Review minor 3: the `iter >= 1` arm of `may_follow_bridge` -- an answer of a
/// stamped turn after a tool result, not only the escalated one.
#[test]
fn an_answer_from_iteration_one_of_a_stamped_turn_leaves_through_the_store() {
    let after_tool = lane(
        "in_answer",
        json!({"finish_reason": "stop"}),
        json!({"turn_id": T, "iter": "1", "path": "simple", "curator_call": "c-10"}),
        text(ANSWER),
    );
    let out = assemble(&[], after_tool);
    assert!(answers(&out).is_empty(), "not at once: {out:?}");
    assert_eq!(
        gate_of(in_phase(&out, "answer-gate"))["messages"][0]["text"],
        ANSWER
    );
}

#[test]
fn the_answer_comes_back_from_the_store_and_leaves() {
    let (out, stderr) = run_cell(ASSEMBLE, &[], answer_reply());
    let said = answers(&out);
    assert_eq!(said.len(), 1, "{out:?}");
    assert_eq!(said[0]["messages"][0]["text"], ANSWER);
    assert_eq!(hop_str(said[0], "turn_id"), T);
    assert_eq!(hop_str(said[0], "finish_reason"), "stop");
    assert!(said[0]["header"].get("interim").is_none(), "{}", said[0]);
    assert!(
        said[0]["header"].get("store_error").is_none(),
        "{}",
        said[0]
    );
    assert!(stderr.is_empty(), "{stderr}");
}

/// Review minor 2, error injection: the store refuses the drop. The answer
/// still leaves -- it rode beside the op, not in it -- and the refusal is said
/// on the answer and on stderr, never in place of the answer.
#[test]
fn a_store_refusal_never_swallows_the_answer() {
    let mut reply = answer_reply();
    reply["header"]["hop"]["bundle_errors"] = json!(1);
    reply["results"][1]["error_code"] = json!("sql_error");
    reply["results"][1]["operation"] = json!("delete");
    let (out, stderr) = run_cell(ASSEMBLE, &[], reply);
    let said = answers(&out);
    assert_eq!(said.len(), 1, "{out:?}");
    assert_eq!(
        said[0]["messages"][0]["text"], ANSWER,
        "the answer, not a report"
    );
    assert_eq!(hop_str(said[0], "store_error"), "sql_error");
    assert!(said[0]["header"].get("degraded").is_none(), "{}", said[0]);
    assert!(
        stderr.contains("refused") && stderr.contains("sql_error"),
        "said: {stderr}"
    );
}

#[test]
fn a_bridge_reads_its_round_back() {
    let out = assemble(&[], bridge_in());
    assert!(answers(&out).is_empty(), "not at once: {out:?}");
    let g = in_phase(&out, "bridge-gate");
    assert_eq!(call(g, "c-bridge-gate")["where"], json!({"turn_id": T}));
    let kept = gate_of(g);
    assert_eq!(kept["messages"][0]["text"], BRIDGE);
    assert_eq!(kept["mark"]["interim"], "1");
}

/// THE PIN: the answer dropped the round first, so the bridge finds no
/// opening row (a departure outlives every round) -- it falls, and the cell
/// says so.
#[test]
fn a_bridge_after_its_answer_is_dropped_and_said() {
    let (out, stderr) = run_cell(
        ASSEMBLE,
        &[],
        bridge_reply(json!([row("depart")]), json!({})),
    );
    assert!(out.is_empty(), "never after the answer: {out:?}");
    assert!(
        stderr.contains("came after its answer"),
        "dropped, not silently: {stderr}"
    );
}

/// Review minor 1: a late tool result parks a row in the dropped round
/// (`in_tool` has no guard). The round is still over -- its opening row
/// `leg-window` falls only with the answer's drop -- so the bridge falls.
#[test]
fn a_late_tool_result_does_not_reopen_the_round_for_its_bridge() {
    let (out, stderr) = run_cell(
        ASSEMBLE,
        &[],
        bridge_reply(json!([row("depart"), row("tool")]), json!({})),
    );
    assert!(out.is_empty(), "never after the answer: {out:?}");
    assert!(stderr.contains("came after its answer"), "{stderr}");
}

#[test]
fn a_bridge_before_its_answer_is_said_once() {
    let out = assemble(
        &[],
        bridge_reply(json!([row("leg-window"), row("assistant")]), json!({})),
    );
    let said = answers(&out);
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(said.len(), 1, "{out:?}");
    assert_eq!(said[0]["messages"][0]["text"], BRIDGE);
    assert_eq!(hop_str(said[0], "interim"), "1");
    assert_eq!(hop_str(said[0], "turn_id"), T);
}

/// Review minor 2, error injection: a store that cannot say whether the round
/// is open does not get the bridge said -- and no store report takes its place
/// on the answer route either. The refusal is on stderr.
#[test]
fn a_bridge_the_store_could_not_check_is_never_said() {
    let (out, stderr) = run_cell(
        ASSEMBLE,
        &[],
        bridge_reply(json!([]), json!({"error_code": "sql_error"})),
    );
    assert!(out.is_empty(), "nothing said: {out:?}");
    assert!(
        stderr.contains("refused") && stderr.contains("sql_error"),
        "said: {stderr}"
    );
}

/// A store that holds the open round of the turn: its opening row is there.
fn open_round() -> WindowStore {
    WindowStore::holding(
        "round",
        vec![json!({"turn_id": T, "session_id": SESSION, "iter": 0,
                    "role": "leg-window", "turn": "{}", "fired": 1,
                    "recorded_at": ""})],
    )
}

/// GH #1505 changed what this case holds. It used to say that a turn's first
/// answer, a sentence beside the model's own calls and an unstamped answer
/// leave at once, because no bridge can precede them. Since every interim is a
/// bridge and no model text leaves without a look at the store, none of the
/// three leaves at once: each goes through its gate and leaves from there when
/// nothing went out in its turn; a second interim is held.
#[test]
fn a_send_no_bridge_can_precede_still_leaves_through_its_gate() {
    let first = lane(
        "in_answer",
        json!({"finish_reason": "stop"}),
        json!({"turn_id": T, "iter": "0", "path": "simple", "curator_call": "c-9"}),
        text(ANSWER),
    );
    let out = assemble(&[], first.clone());
    assert!(answers(&out).is_empty(), "not at once: {out:?}");
    assert_eq!(
        gate_of(in_phase(&out, "answer-gate"))["messages"][0]["text"],
        ANSWER
    );
    let left = drive(&mut open_round(), first);
    assert_eq!(answers(&left).len(), 1, "it leaves from its gate: {left:?}");
    assert!(left[0]["header"].get("repeat").is_none(), "{}", left[0]);

    let beside = lane(
        "in_answer",
        json!({"interim": "1", "finish_reason": "tool_calls"}),
        json!({"turn_id": T, "iter": "1", "path": "simple", "curator_call": "c-10"}),
        text("Let me look."),
    );
    let out = assemble(&[], beside.clone());
    assert!(answers(&out).is_empty(), "not at once: {out:?}");
    assert_eq!(
        gate_of(in_phase(&out, "bridge-gate"))["messages"][0]["text"],
        "Let me look."
    );
    let mut store = open_round();
    let left = drive(&mut store, beside.clone());
    assert_eq!(
        answers(&left).len(),
        1,
        "nothing went out in the turn yet, so it leaves: {left:?}"
    );
    assert_eq!(hop_str(answers(&left)[0], "interim"), "1");
    let again = drive(&mut store, beside);
    assert!(
        answers(&again).is_empty(),
        "a second interim of the turn is held: {again:?}"
    );

    let unstamped = lane(
        "in_answer",
        json!({"finish_reason": "stop"}),
        json!({"turn_id": T, "iter": "1"}),
        text(ANSWER),
    );
    let out = assemble(&[], unstamped.clone());
    assert!(answers(&out).is_empty(), "not at once: {out:?}");
    assert_eq!(
        gate_of(in_phase(&out, "answer-gate"))["messages"][0]["text"],
        ANSWER
    );
    let left = drive(&mut open_round(), unstamped);
    assert_eq!(answers(&left).len(), 1, "it leaves from its gate: {left:?}");
}

// D58 target (GH #1483, contract point 11): the collector knows a bridge by
// a field of its context (`context.bridge`, the composite lifts the curator's
// `hop.bridge`), not by the ending `-bridge` of its call id. Since GH #1505
// every interim is a bridge, with or without the field: the ending of an id
// still decides nothing.

fn d58_interim(ctx: Value) -> Value {
    lane(
        "in_answer",
        json!({"interim": "1", "finish_reason": "stop"}),
        ctx,
        json!([{"origin": "assistant", "type": "text", "text": "One moment."}]),
    )
}

fn d58_gated(out: &[Value]) -> bool {
    out.iter().any(|m| {
        m["header"]["route"].as_str() == Some("cstore")
            && m["header"]["phase"].as_str() == Some("bridge-gate")
    })
}

fn d58_left_at_once(out: &[Value]) -> bool {
    out.iter()
        .any(|m| m["header"]["route"].as_str() == Some("answer"))
}

#[test]
fn d58_target_an_interim_with_the_bridge_field_is_a_bridge() {
    let out = assemble(
        &[],
        d58_interim(
            json!({"turn_id": T, "iter": "0", "path": "simple", "curator_call": "c-1",
                       "call_kind": "side"}),
        ),
    );
    assert!(d58_gated(&out), "the field makes it a bridge: {out:?}");
    assert!(!d58_left_at_once(&out), "{out:?}");
}

/// GH #1505 changed what this case holds. It used to say that an interim
/// without the field is no bridge and leaves at once, whatever its id ends in.
/// Now it goes through the bridge gate like every interim -- and like the same
/// interim under an id without the ending: the id is still no kind.
#[test]
fn d58_target_an_interim_without_the_field_goes_through_the_bridge_gate_too() {
    for id in ["c-1-bridge", "c-1"] {
        let interim = d58_interim(json!({"turn_id": T, "iter": "0", "path": "simple",
                       "curator_call": id}));
        let out = assemble(&[], interim.clone());
        assert!(d58_gated(&out), "{id}: every interim is gated: {out:?}");
        assert!(!d58_left_at_once(&out), "{id}: {out:?}");
        let left = drive(&mut open_round(), interim);
        assert_eq!(
            answers(&left).len(),
            1,
            "{id}: nothing went out in the turn yet, so it leaves: {left:?}"
        );
        assert_eq!(hop_str(answers(&left)[0], "interim"), "1", "{id}");
    }
}
