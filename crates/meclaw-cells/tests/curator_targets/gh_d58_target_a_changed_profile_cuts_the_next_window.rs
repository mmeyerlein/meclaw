//! D58 target (GH #1483, contract point 2) -- a changed profile cuts the
//! next window.
//!
//! WHY this file exists: a model cell reports its profile again whenever a
//! set changes it, on the same lane as its catalogue row. The curator keeps
//! the newest report per target and cuts the very next window by it -- no
//! restart, no second table. Shown on `short_ids`: on in the first report,
//! off in the second, and the windows before and after follow.
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
fn gh_d58_target_a_changed_profile_cuts_the_next_window() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    announce_profile(&mut h, "seat", json!({"short_ids": "1"}));
    turn_for(&mut h, "seat", "t1", TALK[0].0, TALK[0].1);
    let c = turn_for(&mut h, "seat", "t2", TALK[1].0, TALK[1].1);
    assert!(
        texts(&c).iter().any(|x| x.starts_with("[#")),
        "the first profile turns the ids on: {:?} {:?}",
        texts(&c),
        h.stderr
    );
    assert_eq!(
        held(&h, "seat")["window_profile"],
        json!({"short_ids": "1"}),
        "the curator holds the profile beside the row: {}",
        held(&h, "seat")
    );

    announce_profile(&mut h, "seat", json!({"short_ids": "0"}));
    assert_eq!(
        held(&h, "seat")["window_profile"],
        json!({"short_ids": "0"}),
        "the newest report replaces the held profile: {}",
        held(&h, "seat")
    );
    let c = say(&mut h, Some("seat"), "t3", TALK[2].0);
    assert!(
        !texts(&c).iter().any(|x| x.starts_with("[#")),
        "the next window follows the changed profile: {:?}",
        texts(&c)
    );
}
