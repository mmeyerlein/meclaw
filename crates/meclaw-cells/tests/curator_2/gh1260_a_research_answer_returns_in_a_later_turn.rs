//! GH #1260 -- a research answer returns in a later turn.
//!
//! WHY this file exists: a `research` turn leaves for the backend in the
//! background; the quick voice names the waiting time (`research_wait_ms`,
//! two minutes by default) and the conversation goes on. The session's mark
//! `consult_pending:<session>` tells the speaking model a consult runs -- but
//! it ran out at `consult_pending_ms` (30 s) for both ways, the deadline of a
//! `complex` answer in the same turn. A research result due after two minutes
//! lost its mark at half a minute, and the voice could start the same errand
//! again. The mark's `until` now follows the way: `consult` at
//! `consult_pending_ms`, `research` at `research_wait_ms` plus that same
//! margin. The time is read off the mark's own stamps; no wall clock decides.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn curate_with(h: &mut Hive, turn_id: &str, path: &str, extra_ctx: Value, ask: &str) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn_id, "iter": "0", "phase": "",
                     "path": path});
    let mut ctx = json!({"session_id": "s1", "turn_id": turn_id, "iter": "0",
                         "channel": "test", "audience_set": ROUND_E, "path": path});
    for (k, v) in extra_ctx.as_object().cloned().unwrap_or_default() {
        ctx[k] = v;
    }
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(ask)], "system": mode("Be brief.")}),
    );
}

fn pending(h: &Hive) -> Value {
    meclaw_core::serde_json::from_str(&h.state("consult_pending:s1")).unwrap_or(Value::Null)
}

/// `until - at` of the pending mark, in milliseconds.
fn span_ms(mark: &Value) -> i64 {
    let at = |k: &str| {
        chrono::DateTime::parse_from_rfc3339(mark[k].as_str().unwrap_or_default())
            .unwrap_or_else(|e| panic!("{k} of {mark}: {e}"))
    };
    (at("until") - at("at")).num_milliseconds()
}

#[test]
fn a_research_answer_returns_in_a_later_turn() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    turn(&mut h, "s1", "t0", "hello", "hi", json!({}));

    // Turn 1: the errand leaves, the bridge names the wait.
    curate_with(
        &mut h,
        "t1",
        "research",
        json!({}),
        "find everything on the topic",
    );
    let errand = h.routed("research");
    assert_eq!(errand.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let cid = errand[0].hop["consult_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let bridge = h.routed("quick");
    assert_eq!(bridge.len(), 1, "the quick voice bridges: {:?}", h.out);
    let said = bridge[0].body["system"]["instructions"]["bridge"]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        said.contains("2 minutes") && said.contains("later turn"),
        "{said}"
    );
    let mark = pending(&h);
    assert_eq!(mark["consult_id"], json!(cid), "{mark}");
    assert_eq!(
        span_ms(&mark),
        120_000 + 120_000,
        "a research mark stands for research_wait_ms plus the margin: {mark}"
    );
    assert!(
        span_ms(&mark) > 31_000,
        "31 s after the errand the mark is still open: {mark}"
    );

    // Turn 2: the conversation goes on, on the conversation model.
    curate_with(&mut h, "t2", "talk", json!({}), "meanwhile, how are you?");
    let brain = h.routed("brain");
    assert_eq!(
        brain.len(),
        1,
        "the round is not blocked: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(
        brain[0].body["system"]["consult"]["pending"]["text"]
            .as_str()
            .is_some_and(|t| t.contains(&cid)),
        "the voice knows the research runs: {:?}",
        brain[0].body["system"]
    );

    // Later: the result comes back with its consult id and is spoken.
    curate_with(
        &mut h,
        "t3",
        "research",
        json!({"consult_id": cid}),
        "the research result",
    );
    let back = h.routed("brain");
    assert_eq!(
        back.len(),
        1,
        "the result is spoken on the brain: {:?}",
        h.out
    );
    assert!(h.routed("research").is_empty(), "never researched again");
    assert!(
        back[0].body["system"]["consult"].get("pending").is_none(),
        "the answer takes the mark down: {:?}",
        back[0].body["system"]
    );
    assert_eq!(pending(&h), Value::Null, "the mark is gone");

    // A consult's mark keeps the deadline of the same turn.
    curate_with(&mut h, "t4", "complex", json!({}), "plan my week");
    assert_eq!(h.routed("consult").len(), 1, "{:?}", h.out);
    assert_eq!(
        span_ms(&pending(&h)),
        120_000,
        "a consult mark stands for consult_pending_ms"
    );
}
