//! D58 target (GH #1483, contract point 1) -- a curate is cut for the target
//! it names.
//!
//! WHY this file exists: the curator used to cut a window for a way it read
//! off the route stamp and a table of its own; now the caller names the
//! target (`hop.target`, the name its composite wires the model cell under)
//! and the curator cuts for what that cell reported. Two targets with two
//! rows: the one with a hard line under the window gets the window cut to
//! it, the roomy one does not -- whichever was named, and nothing is asked
//! for a target whose row is held.
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
use meclaw_core::serde_json::json;

#[test]
fn gh_d58_target_a_curate_is_cut_for_the_target_it_names() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    announce(
        &mut h,
        "narrow",
        row("test/narrow", 20, 30),
        json!({}),
        json!([]),
    );
    announce(&mut h, "wide", roomy("test/wide"), json!({}), json!([]));

    let to_wide = say(&mut h, Some("wide"), "t1", "Tell me about the river walk.");
    assert_eq!(
        to_wide.hop.get("target"),
        Some(&json!("wide")),
        "the call carries the target it was cut for: {:?} {:?}",
        to_wide.hop,
        h.stderr
    );
    assert!(
        !has_overflow_cut(&to_wide),
        "the roomy row cuts nothing: {:?}",
        to_wide.hop
    );
    assert!(
        h.routed("catalog_ask").is_empty(),
        "a held row asks nothing"
    );
    answer(
        &mut h,
        &to_wide,
        "stop",
        warm_usage(),
        json!([said("It is a fine walk.")]),
    );

    let to_narrow = say(&mut h, Some("narrow"), "t2", "And the old bridge there?");
    assert_eq!(
        to_narrow.hop.get("target"),
        Some(&json!("narrow")),
        "{:?}",
        to_narrow.hop
    );
    assert!(
        has_overflow_cut(&to_narrow),
        "the narrow row's hard line stands on the call cut for it: {:?}",
        to_narrow.hop
    );
    assert!(
        h.routed("catalog_ask").is_empty(),
        "a held row asks nothing"
    );
}
