//! D58 target (GH #1483; wave d58-2 close, inventory 62, target review R2
//! m4) -- a call that leaves the composite is no curate without a target.
//!
//! WHY this file exists: a mark `target_unknown` with `missing: target`
//! says that a curate named no target, so the omission of a composite shows
//! in the ledger (OR-d58-2.target.6). A call that leaves the composite has no
//! target by contract point 4 -- its verdict says it departs, and it is cut
//! with the departure's bundle, no profile. It got the mark all the same, on
//! every errand that left (in talky on every `complex` and `research` turn),
//! and the mark no longer told an omission from an errand. Now the departing
//! call carries none; a curate that stays in the composite and names no
//! target is still marked.
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

/// The lead's knobs: the path `errand` leaves the composite as the class
/// `away`, the path `stay` names no target.
fn over() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        ("lead", "path_routes", json!("errand:away")),
        ("lead", "departures", json!("away:10000")),
    ]
}

/// The `target_unknown` marks of `call` that say the target was missing.
fn missing_target(h: &Hive, call: &str) -> Vec<Value> {
    marks_of_kind(h, TARGET_MARK)
        .into_iter()
        .filter(|m| m["call"] == json!(call) && m["missing"] == json!("target"))
        .collect()
}

/// The curate of `turn` on the path `path`; returns the call ids it sent.
fn curate_on(h: &mut Hive, turn: &str, path: &str) -> Vec<String> {
    curate_for(
        h,
        None,
        turn,
        0,
        json!([user("Plan my week, please.")]),
        json!({"path": path}),
    )
    .iter()
    .filter_map(|m| m.hop["curator_call"].as_str().map(str::to_string))
    .collect()
}

#[test]
fn a_call_that_leaves_the_composite_is_not_marked_as_missing_its_target() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&over());
    let calls = curate_on(&mut h, "t1", "errand");
    let away = calls
        .iter()
        .find(|c| !c.ends_with("-bridge") && !c.contains("-side"))
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "check of the setup: the errand leaves as a call: {calls:?} {:?}",
                h.stderr
            )
        });
    let marks = marks_of_kind(&h, "route");
    assert!(
        marks
            .iter()
            .any(|m| m["call"] == json!(away) && m["way"] == json!("away")),
        "check of the setup: the call departs as the class `away`: {marks:?}"
    );
    assert!(
        missing_target(&h, &away).is_empty(),
        "a call that leaves the composite has no target by contract; it is no curate \
         that forgot one: {:?}",
        marks_of_kind(&h, TARGET_MARK)
    );

    // Control: a curate that stays in the composite and names no target is
    // still marked.
    let calls = curate_on(&mut h, "t2", "stay");
    let stay = calls.first().cloned().unwrap_or_else(|| {
        panic!(
            "check of the setup: the turn leaves as a call: {:?}",
            h.stderr
        )
    });
    assert_eq!(
        missing_target(&h, &stay).len(),
        1,
        "a curate without a target is marked: {:?}",
        marks_of_kind(&h, TARGET_MARK)
    );
}
