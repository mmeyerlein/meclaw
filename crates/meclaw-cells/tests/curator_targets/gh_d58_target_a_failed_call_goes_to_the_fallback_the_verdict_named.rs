//! D58 target (GH #1483, contract point 4) -- a failed call goes to the
//! fallback the verdict named.
//!
//! WHY this file exists: a fast call that came back without an answer (the
//! cell's error, a content filter) went to `brain` -- a way name fixed in
//! the policy (GH #1352). Now the lead's verdict names the fallback for the
//! call, the policy keeps it with the parked call and uses it when the
//! answer comes back failed: the round leaves again, for the fallback
//! target. A verdict without a fallback hands nothing on.
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
use meclaw_core::serde_json::{Value, json};

/// A turn stamped `simple` under the lead's `knobs`; the error of its call
/// comes back. Every call the error brought out.
fn failed(fallback: &str) -> (Msg, Vec<Msg>, Vec<String>) {
    let lead = lead_config("lead");
    assert!(
        lead["params"].get("fallback").is_some(),
        "the lead has a knob `fallback`: {:?}",
        lead["params"]
    );
    let mut h = Hive::with(&[
        ("lead", "path_routes", json!("talk:north,simple:south")),
        ("lead", "fallback", json!(fallback)),
    ]);
    announce_profile(&mut h, "north", json!({}));
    announce_profile(&mut h, "south", json!({}));
    let calls = curate_for(
        &mut h,
        None,
        "t1",
        0,
        json!([user("What is the weather like?")]),
        json!({"path": "simple", "needs": "", "families": ""}),
    );
    let c = the_call(&h, &calls);
    h.out.clear();
    answer(
        &mut h,
        &c,
        "error",
        json!({"error_code": "provider_error"}),
        json!([]),
    );
    (c, model_calls(&h), h.stderr.clone())
}

#[test]
fn gh_d58_target_a_failed_call_goes_to_the_fallback_the_verdict_named() {
    if !shipped() {
        return;
    }
    let (first, after, err) = failed("south:north");
    assert_eq!(
        first.hop.get("target"),
        Some(&json!("south")),
        "the verdict's target: {:?} {err:?}",
        first.hop
    );
    let to: Vec<Value> = after
        .iter()
        .map(|m| m.hop.get("target").cloned().unwrap_or(Value::Null))
        .collect();
    assert_eq!(
        to,
        vec![json!("north")],
        "the failed round leaves again, for the fallback: {after:?} {err:?}"
    );

    let (_, after, err) = failed("");
    assert!(
        after.is_empty(),
        "no fallback named, nothing handed on: {after:?} {err:?}"
    );
}
