//! D58 frame (GH #1478, note per stamp class point 4) -- a class with
//! `pair` proposes question and answer.
//!
//! WHY this file exists: what a turn of a class answered and what the person
//! said next is a pair worth learning (how this person asks, GH #1279). It
//! was learnt for the one fixed class `clarify`, under fixed strings. Now the
//! entry's field `pair` names the field the proposal travels under, and every
//! other string is built from the class's name: the state key
//! `<class>:<session>`, the proposal's id `<class>:<uuid5 of session#turn>`,
//! its `source_ref` `curator:<class>:<session>#<turn>` and the id of the tool
//! call `<class>-<turn>`. The route `propose` and the shape of the value
//! (`question`, `answer`, `class`, `asked_turn`, `answered_turn`) stay. The
//! pair leaves once, at the person's next words; a class without `pair`
//! holds nothing.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;

use curator_hive::*;
use curator_target::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const QUESTION: &str = "Do you mean the river walk or the park?";

/// uuid5 of the policy's namespace over "s1#t1" (python
/// `uuid.uuid5(UUID("5f0c2a8e-8b1d-4b0e-9a43-0c0a7e6f8801"), "s1#t1")`).
const PAIR_UUID: &str = "0812f083-0125-5ee3-a92e-1308f267cb23";

fn turn(h: &mut Hive, turn: &str, path: &str, words: &str) -> Msg {
    let calls = curate_for(
        h,
        Some("seat"),
        turn,
        0,
        json!([user(words)]),
        json!({"path": path}),
    );
    the_call(h, &calls)
}

#[test]
fn gh_d58_frame_a_class_with_pair_proposes_question_and_answer() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[(
        "policy",
        "class_notes",
        json!({"vague": {"text": "Ask which one is meant.", "tools": "none",
                         "pair": "mx.hints.vague"},
               "plain": {"text": "Ask which one is meant."}}),
    )]);
    announce_profile(&mut h, "seat", json!({"system": "delta"}));
    let mut failures = Vec::new();

    let c = turn(&mut h, "t1", "vague", "The walk tomorrow.");
    answer(&mut h, &c, "stop", warm_usage(), json!([said(QUESTION)]));
    if !h.routed("propose").is_empty() {
        failures.push("the pair left before the person's next words".into());
    }
    if !h.state("vague:s1").contains("\"t1\"") {
        failures.push(format!(
            "no row `vague:s1` naming the asked turn: {:?} {:?}",
            h.state("vague:s1"),
            h.stderr
        ));
    }

    let c = turn(&mut h, "t2", "talk", "The river walk.");
    let p = h.routed("propose");
    if p.len() != 1 {
        failures.push(format!("one proposal: {:?} {:?}", h.out, h.stderr));
    } else {
        let m = &p[0].messages()[0];
        if m["type"] != json!("tool_call") || m["id"] != json!("vague-t1") {
            failures.push(format!("the tool call of the proposal: {m}"));
        }
        let args: Value = sj::from_str(m["text"].as_str().unwrap_or("null")).unwrap_or(Value::Null);
        let want = json!({
            "op": "propose",
            "id": format!("vague:{PAIR_UUID}"),
            "source_ref": "curator:vague:s1#t1",
            "entity_ref": "member:e",
            "field_path": "mx.hints.vague",
            "value": {"question": QUESTION, "answer": "The river walk.", "class": "talk",
                      "session_id": "s1", "asked_turn": "t1", "answered_turn": "t2"}
        });
        if args != want {
            failures.push(format!("the proposal: {args}\nwant {want}"));
        }
    }
    if !h.state("vague:s1").is_empty() {
        failures.push("the row `vague:s1` is not taken".into());
    }
    answer(&mut h, &c, "stop", warm_usage(), json!([said("Lovely.")]));

    // Exactly once.
    let c = turn(&mut h, "t3", "talk", "Thanks.");
    if !h.routed("propose").is_empty() {
        failures.push("a second proposal of the same pair".into());
    }
    answer(
        &mut h,
        &c,
        "stop",
        warm_usage(),
        json!([said("You are welcome.")]),
    );

    // A class without `pair` holds nothing and proposes nothing.
    let c = turn(&mut h, "t4", "plain", "That other thing.");
    answer(&mut h, &c, "stop", warm_usage(), json!([said(QUESTION)]));
    if !h.state("plain:s1").is_empty() {
        failures.push("a row `plain:s1` for a class without `pair`".into());
    }
    turn(&mut h, "t5", "talk", "The park.");
    if !h.routed("propose").is_empty() {
        failures.push("a proposal for a class without `pair`".into());
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
