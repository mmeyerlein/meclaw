//! D58-3 kern (GH #1524, contract point 1, K.8) -- a target cut with its
//! `profile_fallback` is the source of its wall with that profile.
//!
//! WHY this file exists: a call to a target that holds its tree
//! (`system: delta`) takes the wall's values from that target's profile
//! (`wall_source`, planner's addendum of 11.11.): the guard's floor reads
//! them, and the call's projection (`window_est:<call>`) hands them to the
//! tap and the rebuild as `wall_profile`. While no profile is held, the
//! policy's `profile_fallback` stands in for the reported one -- the call is
//! cut with it, so the wall of that same call takes its values from it too,
//! not from the instance. Here `main` has a fallback that holds its tree and
//! names a wall target of 20 tokens, nothing was reported, and the first call
//! goes to `main`: its projection names `main` as the wall and carries the
//! fallback as `wall_profile`. The counter-arm is the curator as shipped, no
//! fallback: the projection names no wall profile, the instance rules the wall.
//!
//! The fallback stops at the call: the wall's values of the holder search (a
//! call that holds no tree), the push ask of the recall and the rebuild's
//! probe stay the instance's until the target's own row is held (the
//! description of `profile_fallback` says so).
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

/// The projection of the first call of the session to `main`.
fn first_projection(h: &mut Hive) -> Value {
    let c = say(h, Some("main"), "t1", "Plan my week.");
    assert_eq!(
        c.hop.get("target"),
        Some(&json!("main")),
        "check of the setup: the call leaves for `main`: {:?} {:?}",
        c.hop,
        h.stderr
    );
    let est = h.state(&format!(
        "window_est:{}",
        c.hop["curator_call"].as_str().unwrap_or_default()
    ));
    sj::from_str(&est).unwrap_or_else(|_| panic!("the call projects its window: {est:?}"))
}

#[test]
fn gh_d583_kern_a_fallback_target_is_its_walls_source() {
    if !shipped() {
        return;
    }
    let fallback = json!({"window": "wall", "system": "delta", "tools": "all",
                          "window_target": 20});
    let mut h = Hive::with(&[(
        "policy",
        "profile_fallback",
        json!({ "main": fallback.clone() }),
    )]);
    let est = first_projection(&mut h);
    assert_eq!(
        est["tree"],
        json!(1),
        "check of the setup: the fallback's `system: delta` cuts the call on its tree: {est}"
    );
    assert_eq!(
        est["wall"],
        json!("main"),
        "the call's own target is its wall's source: {est}"
    );
    assert_eq!(
        est["wall_profile"], fallback,
        "the wall's values come from the fallback the call was cut with, not from the \
         instance: {est}"
    );

    // Shipped: no fallback, the wall carries no profile.
    let mut h = Hive::new();
    let est = first_projection(&mut h);
    assert!(
        est.get("wall_profile").is_none(),
        "without a fallback the wall takes the instance's values: {est}"
    );
}
