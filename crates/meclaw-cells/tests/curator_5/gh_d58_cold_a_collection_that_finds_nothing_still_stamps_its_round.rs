//! GH #1464 (D58 coldgc, review I1) -- a collection that finds nothing still
//! stamps its round.
//!
//! WHY this file exists: every rebuild stamps its round in the registry
//! `window_plans` (GH #943), plan changed or not, and `prune_plans` drops a
//! round whose last stamp is older than `round_plan_idle_s` (or the oldest
//! one over `max_round_plans`) together with its plan and its summary --
//! "used" is the last rebuild, since a window lives no longer than the
//! provider's cache. Before `gc_on_cold` every expired cache ran through
//! `finish` and stamped. A cold collection that finds nothing ends without
//! a plan and without a mark (contract point 2); if it ended without the
//! stamp as well, a round that is used often but never long enough to hold
//! garbage would be dropped after 30 days while it is in use. The stamp is
//! the only thing such a collection writes.
//!
//! Runs the shipped curator hive (`support/curator_cold.rs`); no model.

// harness_fold: #[path = "support/curator_cold.rs"]
#[allow(unused_imports)]
use crate::curator_cold; // harness_fold: mod curator_cold;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_cold::*;
use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Map, Value};

/// The stamp the registry `window_plans` holds for the round `round`, if any.
fn stamp_of(h: &Hive, round: &str) -> Option<i64> {
    let reg: Map<String, Value> = sj::from_str(&h.state("window_plans")).unwrap_or_default();
    reg.get(&round_key(round)).and_then(Value::as_i64)
}

#[test]
fn gh_d58_cold_a_collection_that_finds_nothing_still_stamps_its_round() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::new(&cold_knobs(Some("1"))).reporting_composite("talky");
    t.turn("t1", WORDS[0], ANSWERS[0]);
    t.turn("t2", WORDS[3], ANSWERS[3]);
    assert_eq!(
        stamp_of(&t.h, ROUND_E),
        None,
        "check of the setup: no rebuild stamped the round before the stroke: {:?}",
        t.h.state("window_plans")
    );
    cache_expires(&mut t);

    // Contract point 2 stands: no plan, no mark.
    assert!(t.h.summ.is_empty(), "no model is asked: {:?}", t.h.stderr);
    assert!(
        !rebuilt(&t),
        "nothing to collect writes no plan: {} {:?}",
        t.h.rebuild_in(ROUND_E),
        t.h.stderr
    );
    assert_eq!(
        marks_of(&t.h, "gc"),
        Vec::<Value>::new(),
        "no `gc` mark for nothing"
    );
    assert_eq!(
        cold_marks(&t.h),
        Vec::<Value>::new(),
        "no `cold` mark for nothing"
    );

    // The round is stamped as used: the stamp is the stroke's own, since
    // no rebuild stamped it before.
    assert!(
        stamp_of(&t.h, ROUND_E).is_some(),
        "the collection that found nothing stamps its round in `window_plans`: {:?} {:?}",
        t.h.state("window_plans"),
        t.h.stderr
    );
}
