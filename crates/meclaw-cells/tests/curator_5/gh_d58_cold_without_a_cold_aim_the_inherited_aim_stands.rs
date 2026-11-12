//! GH #1464 (D58 coldgc, addendum 10.11.) -- without a cold aim the
//! inherited aim stands.
//!
//! WHY this file exists: `cold_to` ships empty, and empty means the line
//! between "only collect" and "rebuild with the summarizer" is the aim the
//! order of the cache inherited from the last answer -- the aim of a `soft`
//! rebuild of its package, as before the knob. The same long conversation
//! is under that aim with a large package (it only collects) and over it
//! with a small one (the summarizer runs); the role's `window_target` plays
//! no part while `cold_to` is empty.
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
use meclaw_core::serde_json::{Value, json};

fn knobs() -> Vec<(&'static str, &'static str, Value)> {
    let mut over = cold_knobs(Some("1"));
    // A target the cold aim would read if `cold_to` were set (3000 x any
    // share is under this conversation): unset, it must not count.
    over.push(("policy", "window_target", json!(3000)));
    over.push(("policy", "keep_recent", json!(1)));
    over.push(("policy", "keep_recent_tokens", json!(0)));
    over
}

#[test]
fn gh_d58_cold_without_a_cold_aim_the_inherited_aim_stands() {
    if !shipped() {
        return;
    }
    // A large package: the inherited aim is 5000000 x 0.45 / 0.5.
    let (mut t, old) = long_talk("talky", &knobs(), warm_usage());
    assert_eq!(
        t.h.state_in("aim", ROUND_E),
        "4500000",
        "check of the setup: the order inherits the soft aim of its package"
    );
    cache_expires(&mut t);
    assert!(
        t.h.summ.is_empty(),
        "under the inherited aim the cache only collects: {:?}",
        t.h.stderr
    );
    assert_eq!(
        shrunk(&t),
        vec![short_id(&old)],
        "the collection ran: {:?}",
        t.h.stderr
    );
    assert_eq!(cold_marks(&t.h).len(), 1, "the cold collection says so");
    let window = next_window(&mut t, "t5");
    assert!(
        stands(&window, &long_words(1)),
        "the long words stand raw under the inherited aim: {window:?}"
    );

    // A small package: the inherited aim is 3000 x 0.45 / 0.5 = 2700 tokens,
    // and the collected conversation stays over it.
    let (mut t, _) = long_talk("talky", &knobs(), package_usage(3000));
    assert_eq!(
        t.h.state_in("aim", ROUND_E),
        "2700",
        "check of the setup: the order inherits the soft aim of its package"
    );
    cache_expires(&mut t);
    assert_eq!(
        t.h.summ.len(),
        1,
        "over the inherited aim the summarizer runs: {:?}",
        t.h.stderr
    );
    t.h.answer("Three stories about a garden; two files were read.", "stop");
    assert!(
        marks_of(&t.h, "over_aim").is_empty(),
        "the rebuild lands on the inherited aim: {:?}",
        marks_of(&t.h, "over_aim")
    );
    assert!(
        cold_marks(&t.h).is_empty(),
        "a rebuild with the summarizer is no cold collection"
    );
}
