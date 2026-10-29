//! GH #1352 -- a quick call that comes back without an answer goes to the
//! brain.
//!
//! WHY this file exists: in a lab run of the owner's product the quick voice
//! of a `simple` turn ran into its `tool_call_limit` (17 calls in one answer,
//! the dispatcher's budget is 16; GH #1350). The `llm` cell answered with an
//! error, talky sent it to the error drain, and the person got no answer at
//! all -- the session closed without a word. A tool round without its call
//! already went to the brain (GH #1260); since #1352 every quick call that
//! ends without an answer does: an error or a content filter takes the round
//! to the brain with ONE `escalate` mark, reason `no_answer`, that names how
//! the call ended and its error code. A bridge does not: its errand answers.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn seam(h: &mut Hive, turn_id: &str, path: &str, messages: Value) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn_id, "iter": "0", "phase": "",
                     "path": path, "families": "web"});
    let ctx = json!({"session_id": "s1", "turn_id": turn_id, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": path});
    h.lane("in_curate", ctx, hop, json!({"messages": messages}));
}

fn escalations(h: &Hive) -> Vec<Value> {
    h.rows("SELECT value FROM marks WHERE kind = 'escalate' ORDER BY seq")
        .iter()
        .map(|r| {
            meclaw_core::serde_json::from_str(r[0].as_str().unwrap_or("{}"))
                .expect("the mark is JSON")
        })
        .collect()
}

fn parked_copies(h: &Hive) -> Vec<Value> {
    h.rows("SELECT key FROM state WHERE key LIKE 'pending:%-esc' ORDER BY key")
        .into_iter()
        .map(|r| r[0].clone())
        .collect()
}

/// The quick call of a `simple` turn that needs no tool.
fn quick_call(h: &mut Hive, turn: &str) -> Msg {
    seam(h, turn, "simple", json!([user("please call my daughter")]));
    let quick = h.routed("quick");
    assert_eq!(quick.len(), 1, "one quick call: {:?} {:?}", h.out, h.stderr);
    assert!(
        quick[0].hop.get("interim").is_none(),
        "no tool round: {:?}",
        quick[0].hop
    );
    quick[0].clone()
}

#[test]
fn a_quick_call_without_an_answer_goes_to_the_brain() {
    if !shipped() {
        return;
    }
    // The cut of the lab run (`tool_call_limit`, GH #1350), a provider error
    // and a content filter -- each without an answer -- go to the brain.
    for (turn, finish, extra) in [
        (
            "t1",
            "error",
            json!({"error_code": "provider_error", "error_source": "wire"}),
        ),
        ("t2", "error", json!({"error_code": "timeout"})),
        ("t3", "content_filter", json!({})),
    ] {
        let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
        h.check_emits = true;
        let call = quick_call(&mut h, turn);
        h.out.clear();
        h.tap(&call, finish, extra.clone(), json!([]));
        let brain = h.routed("brain");
        assert_eq!(
            brain.len(),
            1,
            "{turn} {finish}: the round goes to the brain: {:?} {:?}",
            h.out,
            h.stderr
        );
        assert!(
            h.routed("quick").is_empty(),
            "{turn}: not to the quick voice again"
        );
        let marks = escalations(&h);
        assert_eq!(marks.len(), 1, "{turn}: one escalate mark: {marks:?}");
        assert_eq!(marks[0]["reason"], json!("no_answer"), "{marks:?}");
        assert_eq!(marks[0]["from"], json!("quick"));
        assert_eq!(marks[0]["to"], json!("brain"));
        assert_eq!(marks[0]["finish"], json!(finish), "{marks:?}");
        assert_eq!(
            marks[0].get("error"),
            extra.get("error_code"),
            "{turn}: the error rides in the mark: {marks:?}"
        );
        let shown = meclaw_core::serde_json::to_string(&brain[0].body["messages"]).unwrap();
        assert!(
            shown.contains("call my daughter"),
            "the person's turn is in the brain's window: {shown}"
        );
        assert!(
            parked_copies(&h).is_empty(),
            "{turn}: the copy is taken: {:?}",
            parked_copies(&h)
        );
        assert_eq!(h.emit_violations, Vec::<String>::new());
    }
}

#[test]
fn a_quick_call_with_an_answer_stays_quick_and_drops_its_copy() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    let call = quick_call(&mut h, "t1");
    assert_eq!(
        parked_copies(&h).len(),
        1,
        "the quick call keeps a copy of its turn"
    );
    h.out.clear();
    h.tap(
        &call,
        "stop",
        json!({}),
        json!([{"origin": "assistant", "type": "text", "text": "Who should I call?"}]),
    );
    assert!(h.routed("brain").is_empty(), "{:?} {:?}", h.out, h.stderr);
    assert!(escalations(&h).is_empty());
    assert!(
        parked_copies(&h).is_empty(),
        "an answer drops the copy: {:?}",
        parked_copies(&h)
    );
}

#[test]
fn a_failed_bridge_does_not_escalate() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    seam(
        &mut h,
        "t1",
        "complex",
        json!([user("plan my week around the move")]),
    );
    let quick = h.routed("quick");
    assert_eq!(quick.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert!(quick[0].hop.get("bridge").is_some(), "{:?}", quick[0].hop);
    h.out.clear();
    h.tap(
        &quick[0],
        "error",
        json!({"error_code": "provider_error"}),
        json!([]),
    );
    assert!(
        h.routed("brain").is_empty(),
        "the errand answers the turn: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(escalations(&h).is_empty());
}

/// talky's `./quick -> ./errors` edge leaves the error of every curator call
/// of the quick voice to the brain (the `escalate` mark carries it), except a
/// bridge's; a call that is no curator call (a push on `in_model`) still
/// reaches `./errors`.
#[test]
fn talky_leaves_a_quick_calls_error_to_the_brain() {
    if !shipped() {
        return;
    }
    let talky = read_json(&repo("templates/talky/config.json"));
    let edge = talky["params"]["graph"]["edges"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|e| e["from"] == "./quick" && e["to"] == "./errors")
        .unwrap_or_else(|| panic!("no ./quick -> ./errors edge"));
    let cond = edge["condition"].as_str().unwrap_or_default();
    assert!(
        cond.contains(
            "!(has(context.curator_call) && context.curator_call != '' \
             && !context.curator_call.endsWith('-bridge'))"
        ),
        "a curator call's error goes to the brain, a bridge's to ./errors: {cond}"
    );
}
