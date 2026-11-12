//! D58 target (GH #1483, contract point 3, precedence) -- a profile value
//! wins and the instance knob fills the gap.
//!
//! WHY this file exists: before, a knob set on the curator instance beat
//! the role of the main model (`knob_of`), and the fast targets knew no knob
//! at all. Now the order is one rule for every target: what the profile
//! names counts; what it does not name comes from the knobs of the curator
//! instance, else from their shipped value. The knob `short_ids` shows it in
//! the bytes of the window: the instance turns it on; a profile that says
//! "0" gets no ids, a profile that names only another value gets them.
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

/// Whether the earlier elements of the window carry short ids.
fn with_ids(profile: Value) -> (bool, Vec<String>) {
    ids_under(&[("policy", "short_ids", json!("1"))], profile)
}

/// [`with_ids`] on an instance with the knobs `over`.
fn ids_under(over: &[(&str, &str, Value)], profile: Value) -> (bool, Vec<String>) {
    let mut h = Hive::with(over);
    announce_profile(&mut h, "seat", profile);
    turn_for(&mut h, "seat", "t1", TALK[0].0, TALK[0].1);
    let c = say(&mut h, Some("seat"), "t2", TALK[1].0);
    let t = texts(&c);
    (t.iter().any(|x| x.starts_with("[#")), t)
}

#[test]
fn gh_d58_target_a_profile_value_wins_and_the_instance_knob_fills_the_gap() {
    if !shipped() {
        return;
    }
    let (ids, t) = with_ids(json!({"keep_recent": 40}));
    assert!(
        ids,
        "the profile does not name short_ids -- the instance knob fills it: {t:?}"
    );
    let (ids, t) = with_ids(json!({"keep_recent": 40, "short_ids": "0"}));
    assert!(
        !ids,
        "the profile names short_ids \"0\" -- it wins over the instance's \"1\": {t:?}"
    );
    // The third stage: neither the profile nor the instance names the
    // value, so its shipped value holds (`short_ids` ships off).
    let (ids, t) = ids_under(&[], json!({"keep_recent": 40}));
    assert!(
        !ids,
        "neither the profile nor the instance names short_ids -- the shipped value (off) \
         holds: {t:?}"
    );
    // The shipped value is no floor: a profile turns it on alone.
    let (ids, t) = ids_under(&[], json!({"keep_recent": 40, "short_ids": "1"}));
    assert!(
        ids,
        "the profile names short_ids \"1\" on an instance that names none: {t:?}"
    );
}
