//! D58 target (GH #1483, contract point 5) -- an unheard target is asked
//! beside its call and marked.
//!
//! WHY this file exists: a curate may name a target whose cell has not told
//! the curator its package yet. The standing decision holds (OR-d38-3.c3b.1):
//! the curator asks beside the call and never instead of it. The ask is no
//! longer behind a knob -- it always goes, as the empty params slot on
//! `catalog_ask`: the first call of a session to a target without a row asks
//! every cell at once (no `hop.catalog_for`, OR-d58-2.target.15), a later
//! call to another unheard target asks that one cell (`hop.catalog_for`),
//! each once per session -- and every window cut on the instance's values
//! is booked as a `target_unknown` mark (`missing` "profile").
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

#[test]
fn gh_d58_target_an_unheard_target_is_asked_beside_its_call_and_marked() {
    if !shipped() {
        return;
    }
    assert!(
        cell_config("policy")["params"].get("catalog_ask").is_none(),
        "the ask is no knob any more: it always goes"
    );
    let mut h = Hive::new();
    let c1 = say(&mut h, Some("stranger"), "t1", "Hello there.");
    assert_eq!(
        c1.hop.get("target"),
        Some(&json!("stranger")),
        "the call leaves -- the ask does not replace it: {:?} {:?}",
        c1.hop,
        h.stderr
    );
    let asks = h.routed("catalog_ask");
    assert_eq!(asks.len(), 1, "one ask beside the call: {:?}", h.out);
    assert!(
        asks[0].hop.get("catalog_for").is_none(),
        "the first ask of the session goes to every cell: {:?}",
        asks[0].hop
    );
    assert_eq!(
        Value::Object(asks[0].body.clone()),
        json!({"system": {}, "params": {}}),
        "the ask is the empty params slot"
    );
    let marks = marks_of_kind(&h, TARGET_MARK);
    assert_eq!(marks.len(), 1, "{marks:?}");
    assert_eq!(marks[0]["missing"], json!("profile"), "{marks:?}");
    assert_eq!(marks[0]["target"], json!("stranger"), "{marks:?}");

    answer(&mut h, &c1, "stop", warm_usage(), json!([said("Hello!")]));
    say(&mut h, Some("stranger"), "t2", "How are you?");
    assert!(
        h.routed("catalog_ask").is_empty(),
        "once per session and target -- no polling: {:?}",
        h.out
    );

    // Another target no cell has answered for: asked on its own, once.
    let c3 = say(&mut h, Some("other"), "t3", "And you?");
    assert_eq!(c3.hop.get("target"), Some(&json!("other")), "{:?}", c3.hop);
    let asks = h.routed("catalog_ask");
    assert_eq!(asks.len(), 1, "one ask for the other target: {:?}", h.out);
    assert_eq!(
        asks[0].hop["catalog_for"],
        json!("other"),
        "a later ask names its cell: {:?}",
        asks[0].hop
    );
    assert_eq!(
        Value::Object(asks[0].body.clone()),
        json!({"system": {}, "params": {}}),
        "the ask is the empty params slot"
    );

    // The answer of the cell: from now on the target is heard.
    announce_profile(&mut h, "stranger", json!({}));
    say(&mut h, Some("stranger"), "t4", "Good.");
    assert!(
        h.routed("catalog_ask").is_empty(),
        "a heard target asks nothing: {:?}",
        h.out
    );
    assert_eq!(
        marks_of_kind(&h, TARGET_MARK).len(),
        3,
        "the three calls before a report are marked, the one after is not"
    );
}
