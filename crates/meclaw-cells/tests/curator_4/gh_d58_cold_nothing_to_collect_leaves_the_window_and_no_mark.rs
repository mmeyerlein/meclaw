//! GH #1464 (D58 coldgc, contract point 2) -- nothing to collect leaves the
//! window and no mark.
//!
//! WHY this file exists: a collection that finds nothing must not look like
//! one. A plan written for nothing would be a new window epoch for no cut,
//! and a `gc` or `cold` mark for nothing would fill the watcher's figures
//! (`gc_share`, `rebuild_rate`) with strokes of the clock instead of work.
//! A conversation without garbage is left as it is: no plan, no mark, and
//! the next call begins with the window of the last one.
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
use meclaw_core::serde_json::Value;

#[test]
fn gh_d58_cold_nothing_to_collect_leaves_the_window_and_no_mark() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::new(&cold_knobs(Some("1"))).reporting_composite("talky");
    t.turn("t1", WORDS[0], ANSWERS[0]);
    let last = t.turn("t2", WORDS[3], ANSWERS[3]);
    cache_expires(&mut t);

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

    // The window is untouched: the next call begins with the last one's.
    let before = texts(&last);
    let after = next_window(&mut t, "t3");
    assert_eq!(
        after[..before.len()],
        before[..],
        "the next call begins with the window of the last one"
    );
}
