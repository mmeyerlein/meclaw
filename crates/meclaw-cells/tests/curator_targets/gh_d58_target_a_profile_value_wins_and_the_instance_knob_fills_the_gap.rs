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

/// The `curate_mark` of the newest order the answer of a call for the one
/// target `seat` armed: `seat` holds its tree (`system: delta`), so it is
/// the wall's source, under the knobs `over` and the profile `profile`.
/// `"target"` is the wall's own rebuild (`window_target` reached).
fn wall_mark(over: &[(&str, &str, Value)], profile: Value) -> Value {
    let mut h = Hive::with(over);
    let mut p = profile;
    p["system"] = json!("delta");
    announce_profile(&mut h, "seat", p);
    turn_for(&mut h, "seat", "t1", TALK[0].0, TALK[0].1);
    let c = say(&mut h, Some("seat"), "t2", TALK[1].0);
    let since = h.clock.len();
    answer(&mut h, &c, "stop", wall_usage(), json!([said(TALK[1].1)]));
    adds_since(&h, since)
        .last()
        .map(|m| m.body["emit_body"]["curate_mark"].clone())
        .unwrap_or(Value::Null)
}

/// The same three stages for a value of the wall (D58 close, review 1 m1
/// of the target strand: the pin above showed them on a value of the call
/// only). `window_target` of the tree-holding target: a window of about
/// 2000 tokens ([`wall_usage`]) over a target of 20 orders the wall's own
/// rebuild, 0 is off.
#[test]
fn gh_d58_target_a_profile_value_wins_and_the_instance_knob_fills_the_gap_on_the_wall() {
    if !shipped() {
        return;
    }
    let knob = [("policy", "window_target", json!(20))];
    assert_eq!(
        wall_mark(&knob, json!({})),
        json!("target"),
        "the profile does not name window_target -- the instance's 20 fills it"
    );
    assert_ne!(
        wall_mark(&knob, json!({"window_target": 0})),
        json!("target"),
        "the profile names window_target 0 -- it wins over the instance's 20"
    );
    assert_ne!(
        wall_mark(&[], json!({})),
        json!("target"),
        "neither names window_target -- the shipped value (off) holds"
    );
    assert_eq!(
        wall_mark(&[], json!({"window_target": 20})),
        json!("target"),
        "the profile names window_target 20 on an instance that names none"
    );
}
