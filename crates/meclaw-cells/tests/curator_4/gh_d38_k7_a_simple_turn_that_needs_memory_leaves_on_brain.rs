//! GH #1435 (D38 K7) -- a `simple` turn that needs memory leaves on `brain`.
//!
//! WHY this file exists: the way guideline says a conversation about the
//! person's own people belongs to the conversation model; only a plain tool
//! request belongs to the fast one. A `simple` stamp on "how is my sister
//! doing?" is the stamp's error, not the way's, and the quick window carries
//! no memory rows by design (`FAST_NO_KINDS`). So with the new policy knob
//! `recall_to_fallback` ("0" shipped) a `simple` turn whose stamp names `memory`
//! in `needs` leaves on `brain` instead of `quick` (the way decision beside
//! `path_routes`, R-WP-1), its `route` mark says `reason: memory`, and the
//! brain call sees the recall rows of the turn
//! like every `talk` turn. A `simple` turn without the need takes the quick
//! way as before.
//!
//! Runs the shipped policy in the test hive (`support/curator_hive.rs`); the
//! route cell is stubbed by the hop's `needs`, no model is called.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const RECALL_ID: &str = "call_recall_00000000000000k7";
const SAID_BEFORE: &str = "WHAT WAS SAID\n  user: \"My sister Anna moved to Hamburg.\"";

fn memory_hive() -> Hive {
    Hive::with(&[("lead", "recall_to_fallback", json!("1"))]).reporting_composite("talky")
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

#[test]
fn a_simple_turn_that_needs_memory_leaves_on_brain_with_its_recall() {
    if !shipped() {
        return;
    }
    let mut h = memory_hive();
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
    let mut h = memory_hive();
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
        .filter(|m| m.hop.get("call_kind").is_none())
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
            "no memory rows on the quick way (FAST_NO_KINDS): {:?}",
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
