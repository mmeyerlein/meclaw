//! GH #1435 (D38 K7) -- a `simple` turn that needs memory leaves on `brain`.
//!
//! WHY this file exists: the way guideline says a conversation about the
//! person's own people belongs to the conversation model; only a plain tool
//! request belongs to the fast one. A `simple` stamp on "how is my sister
//! doing?" is the stamp's error, not the way's, and the quick window carries
//! no memory rows by design (`QUICK_NO_KINDS`). So with the new policy knob
//! `memory_to_brain` ("0" shipped) a `simple` turn whose stamp names `memory`
//! in `needs` leaves on `brain` instead of `quick` (the way decision beside
//! `path_routes`, R-WP-1), its `route` mark says `reason: memory`, a held
//! speculative answer of that turn is dropped (the drop table's new row
//! `memory in needs`), and the brain call sees the recall rows of the turn
//! like every `talk` turn. A `simple` turn without the need takes the quick
//! way as before.
//!
//! Runs the shipped policy in the test hive (`support/curator_hive.rs`); the
//! route cell is stubbed by the hop's `needs`, no model is called.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const RECALL_ID: &str = "call_recall_00000000000000k7";
const SAID_BEFORE: &str = "WHAT WAS SAID\n  user: \"My sister Anna moved to Hamburg.\"";

fn memory_hive(spec: bool) -> Hive {
    let mut over = vec![
        ("policy", "role", json!("talky")),
        ("policy", "memory_to_brain", json!("1")),
    ];
    if spec {
        over.push(("policy", "quick_speculative", json!("1")));
        over.push(("policy", "spec_tools", json!("")));
    }
    Hive::with(&over)
}

/// The person's turn with the collector's recall pair ahead of the answer.
fn turn_with_recall(text: &str) -> Value {
    json!([
        user(text),
        tool_call(RECALL_ID, "memory_recall"),
        tool_result(RECALL_ID, SAID_BEFORE),
    ])
}

/// The normal seam of a stamped turn: `hop.path`, `hop.needs` as the collector
/// carries them (a comma list in stamp order).
fn seam(h: &mut Hive, turn: &str, path: &str, needs: &str, messages: Value) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": path, "needs": needs, "families": ""});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": path});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": messages, "system": mode("Be brief.")}),
    );
}

fn marks(h: &Hive, kind: &str) -> Vec<Value> {
    h.rows(&format!(
        "SELECT value FROM marks WHERE kind = '{kind}' ORDER BY seq"
    ))
    .iter()
    .map(|r| sj::from_str(r[0].as_str().unwrap_or("{}")).expect("the mark is JSON"))
    .collect()
}

fn mentions(call: &Msg, needle: &str) -> bool {
    sj::to_string(&call.messages())
        .expect("messages")
        .contains(needle)
}

/// The early curate of the collector (`quick_speculative`): same turn, `spec`
/// "1", no path yet.
fn spec_call(h: &mut Hive, turn: &str, text: &str) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": "", "spec": "1"});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": ""});
    h.lane("in_curate", ctx, hop, json!({"messages": [user(text)]}));
    let quick = h.routed("quick");
    assert_eq!(quick.len(), 1, "one spec call: {:?} {:?}", h.out, h.stderr);
    assert_eq!(quick[0].hop["spec"], json!("1"));
    quick[0].clone()
}

/// The quick voice's answer to the spec call, as talky's `./quick_spec ->
/// ./curator` edge hands it in: the call's context with `spec`, lane
/// `in_spec_llm`. Held until the stamp decides.
fn spec_answer(h: &mut Hive, call: &Msg, text: &str) {
    h.out.clear();
    let mut ctx = call.context.clone();
    for k in ["curator_call", "turn_id", "session_id", "iter"] {
        ctx.insert(k.into(), call.hop[k].clone());
    }
    ctx.insert("spec".into(), json!("1"));
    h.lane(
        "in_spec_llm",
        Value::Object(ctx),
        json!({"finish_reason": "stop", "model": "test-model", "tokens_prompt": 10}),
        json!({"messages": [said(text)]}),
    );
    assert!(h.out.is_empty(), "a held answer goes nowhere: {:?}", h.out);
}

#[test]
fn a_simple_turn_that_needs_memory_leaves_on_brain_with_its_recall() {
    if !shipped() {
        return;
    }
    let mut h = memory_hive(false);
    seam(
        &mut h,
        "t1",
        "simple",
        "memory",
        turn_with_recall("How is my sister doing?"),
    );
    assert!(
        h.routed("quick").is_empty(),
        "a memory turn does not take the fast model: {:?} {:?}",
        h.out,
        h.stderr
    );
    let brain = h.routed("brain");
    assert_eq!(
        brain.len(),
        1,
        "one call of the conversation model: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(
        mentions(&brain[0], "My sister Anna moved to Hamburg."),
        "the brain call sees the recall rows of the turn: {:?}",
        brain[0].messages()
    );
    let route = marks(&h, "route");
    assert_eq!(
        route.len(),
        1,
        "one route mark for the one call with a path: {route:?}"
    );
    assert_eq!(route[0]["path"], json!("simple"), "{route:?}");
    assert_eq!(route[0]["way"], json!("brain"), "{route:?}");
    assert_eq!(
        route[0]["reason"],
        json!("memory"),
        "the mark says why: {route:?}"
    );
}

#[test]
fn a_simple_turn_without_the_memory_need_still_takes_the_quick_way() {
    if !shipped() {
        return;
    }
    let mut h = memory_hive(false);
    seam(
        &mut h,
        "t1",
        "simple",
        "tool",
        turn_with_recall("How warm is it in Berlin?"),
    );
    assert!(h.routed("brain").is_empty(), "{:?}", h.out);
    // A `tool` need on the quick way is the tool round: the required call and
    // its bridge beside it (GH #1248), both on the quick voice.
    let quick = h.routed("quick");
    let calls: Vec<&Msg> = quick
        .iter()
        .filter(|m| !m.hop.contains_key("bridge"))
        .collect();
    assert_eq!(
        calls.len(),
        1,
        "the quick way as before: {:?} {:?}",
        h.out,
        h.stderr
    );
    for m in &quick {
        assert!(
            !mentions(m, "WHAT WAS SAID"),
            "no memory rows on the quick way (QUICK_NO_KINDS): {:?}",
            m.messages()
        );
    }
    let route = marks(&h, "route");
    assert_eq!(route.len(), 1, "{route:?}");
    assert_eq!(route[0]["way"], json!("quick"), "{route:?}");
    assert!(
        route[0].get("reason").is_none(),
        "no reason on the way of before: {route:?}"
    );

    // `memory` named, but the stamp says `talk`: the brain as before, no reason.
    seam(
        &mut h,
        "t2",
        "talk",
        "memory",
        turn_with_recall("Tell me about Anna."),
    );
    assert_eq!(h.routed("brain").len(), 1, "{:?}", h.out);
    let route = marks(&h, "route");
    assert_eq!(route.len(), 2, "{route:?}");
    assert_eq!(route[1]["way"], json!("brain"), "{route:?}");
    assert!(
        route[1].get("reason").is_none(),
        "a talk turn changes nothing: {route:?}"
    );
}

#[test]
fn a_held_speculation_of_a_memory_turn_is_dropped() {
    if !shipped() {
        return;
    }
    let mut h = memory_hive(true);
    let call = spec_call(&mut h, "t1", "How is my sister doing?");
    spec_answer(&mut h, &call, "SPECTEXT she is fine");
    seam(
        &mut h,
        "t1",
        "simple",
        "memory",
        turn_with_recall("How is my sister doing?"),
    );
    assert_eq!(
        h.routed("brain").len(),
        1,
        "the normal call goes on brain: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(h.routed("quick").is_empty(), "{:?}", h.out);
    assert!(
        h.routed("spec_release").is_empty(),
        "the held answer is not released: {:?}",
        h.out
    );
    assert!(
        !h.out.iter().any(|m| sj::to_string(&m.messages())
            .expect("messages")
            .contains("SPECTEXT")),
        "the spec text leaves nowhere: {:?}",
        h.out
    );
    let spec = marks(&h, "quick_spec");
    assert_eq!(spec.len(), 1, "{spec:?}");
    assert_eq!(spec[0]["verdict"], json!("dropped"), "{spec:?}");
    assert_eq!(
        spec[0]["reason"],
        json!("memory"),
        "the drop table's new row: {spec:?}"
    );
    let route = marks(&h, "route");
    assert_eq!(route.len(), 1, "{route:?}");
    assert_eq!(route[0]["way"], json!("brain"), "{route:?}");
    assert_eq!(route[0]["reason"], json!("memory"), "{route:?}");
}

/// The voice path of the sets (`delta_speculation` with `quick_speculative`):
/// talky's route stamps a partial of the turn (`in_delta_stamp`) before the
/// collector has the memory leg.
fn delta_hive(memory_to_brain: &str) -> Hive {
    Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "memory_to_brain", json!(memory_to_brain)),
        ("policy", "quick_speculative", json!("1")),
        ("policy", "spec_tools", json!("")),
        ("policy", "delta_speculation", json!("1")),
    ])
}

fn delta_stamp(h: &mut Hive, turn: &str, needs: Value, text_norm: &str) {
    h.out.clear();
    h.lane(
        "in_delta_stamp",
        json!({"session_id": "s1", "turn_id": turn, "delta": "1"}),
        json!({"turn_id": turn, "path": "simple"}),
        json!({"messages": [], "stamp": {"turn_id": turn, "path": "simple", "p": 0.9,
                                         "intent": "", "needs": needs, "families": [],
                                         "duration": 5, "text_norm": text_norm}}),
    );
}

/// Review I-1 of the strand: a delta stamp `simple` + `memory` with the same
/// text made the spec curate -- fired without the memory leg -- the turn's
/// normal curate, so the brain call left without the recall rows and the
/// later normal curate (which waited for them) parked. With the knob on, the
/// delta is refired instead: the spec call goes on the quick way as #1275,
/// the normal curate brings the recall, leaves on brain and drops the spec
/// whose answer is not in yet (reason `memory`).
#[test]
fn a_delta_of_a_memory_turn_is_refired_and_the_brain_reads_the_recall() {
    if !shipped() {
        return;
    }
    const ASK: &str = "How is my sister doing?";
    let mut h = delta_hive("1");
    delta_stamp(&mut h, "t1", json!(["memory"]), "how is my sister doing");
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "phase": "",
                     "path": "", "spec": "1"});
    let ctx = json!({"session_id": "s1", "turn_id": "t1", "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": ""});
    h.lane("in_curate", ctx, hop, json!({"messages": [user(ASK)]}));
    assert!(
        h.routed("brain").is_empty(),
        "no brain call without the recall: {:?} {:?}",
        h.out,
        h.stderr
    );
    let quick = h.routed("quick");
    assert_eq!(quick.len(), 1, "the spec call: {:?} {:?}", h.out, h.stderr);
    assert_eq!(
        quick[0].hop["spec"],
        json!("1"),
        "#1275: {:?}",
        quick[0].hop
    );
    let delta = marks(&h, "delta");
    assert_eq!(delta.len(), 1, "{delta:?}");
    assert_eq!(delta[0]["verdict"], json!("refired"), "{delta:?}");
    assert_eq!(delta[0]["reason"], json!("memory"), "{delta:?}");

    // The normal curate with the recall leg: brain, the recall rows, the spec
    // (answer not in yet) dropped at once.
    seam(&mut h, "t1", "simple", "memory", turn_with_recall(ASK));
    let brain = h.routed("brain");
    assert_eq!(brain.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert!(
        mentions(&brain[0], "My sister Anna moved to Hamburg."),
        "the brain call sees the recall rows: {:?}",
        brain[0].messages()
    );
    assert!(h.routed("quick").is_empty(), "{:?}", h.out);
    let spec = marks(&h, "quick_spec");
    assert_eq!(spec.len(), 1, "{spec:?}");
    assert_eq!(spec[0]["verdict"], json!("dropped"), "{spec:?}");
    assert_eq!(
        spec[0]["reason"],
        json!("memory"),
        "answer not in yet: {spec:?}"
    );
    let route = marks(&h, "route");
    assert_eq!(route.len(), 1, "{route:?}");
    assert_eq!(route[0]["way"], json!("brain"), "{route:?}");
    assert_eq!(route[0]["reason"], json!("memory"), "{route:?}");
}

/// Knob off, or no `memory` in the delta's needs: the delta is reused as
/// before (#1279).
#[test]
fn a_delta_without_the_knob_or_the_need_is_reused_as_before() {
    if !shipped() {
        return;
    }
    for (knob, needs) in [("0", json!(["memory"])), ("1", json!(["tool"]))] {
        let mut h = delta_hive(knob);
        delta_stamp(&mut h, "t1", needs.clone(), "how warm is it");
        h.out.clear();
        let hop = json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "phase": "",
                         "path": "", "spec": "1"});
        let ctx = json!({"session_id": "s1", "turn_id": "t1", "iter": "0",
                         "channel": "test", "audience_set": ROUND_E, "path": ""});
        h.lane(
            "in_curate",
            ctx,
            hop,
            json!({"messages": [user("How warm is it?")]}),
        );
        let delta = marks(&h, "delta");
        assert_eq!(delta.len(), 1, "{knob} {needs}: {delta:?} {:?}", h.stderr);
        assert_eq!(delta[0]["verdict"], json!("reused"), "{knob} {needs}");
        assert!(
            delta[0].get("reason").is_none(),
            "{knob} {needs}: {delta:?}"
        );
    }
}
