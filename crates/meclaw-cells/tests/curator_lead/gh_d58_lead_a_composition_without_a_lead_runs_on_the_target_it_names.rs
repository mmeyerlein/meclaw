//! D58 lead (GH #1474, contract point 3) -- a composite without a lead runs
//! on the target it names.
//!
//! WHY this file exists: with the switch outside the curator, a composite
//! that does not choose between targets (cogny, simple-agent) wires no lead.
//! Its curator must then neither ask nor wait: the branch of `win` that
//! waits for the verdict hangs on the instance knob, not on an empty
//! verdict, every field of the verdict has its empty value, and the target
//! is the curate's own `hop.target`. A route stamp that a lead would have
//! turned elsewhere (`simple`) changes nothing, no `lead` mark is booked,
//! and the switch's store is never touched.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`) without a lead;
//! no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_lead.rs"]
#[allow(unused_imports)]
use crate::curator_lead; // harness_fold: mod curator_lead;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;

use curator_hive::*;
use curator_lead::*;
use curator_target::*;
use meclaw_core::serde_json::json;

#[test]
fn gh_d58_lead_a_composition_without_a_lead_runs_on_the_target_it_names() {
    if !shipped() {
        return;
    }
    let mut h = bare(&[]);
    announce_profile(&mut h, "north", json!({}));
    for (turn, path) in [("t1", "simple"), ("t2", "talk"), ("t3", "")] {
        let calls = curate_for(
            &mut h,
            Some("north"),
            turn,
            0,
            json!([user("what is the weather like?")]),
            json!({"path": path, "needs": [], "families": []}),
        );
        let call = the_call(&h, &calls);
        assert_eq!(
            call.hop.get("target"),
            Some(&json!("north")),
            "stamp {path:?}: the call runs on the target the curate names: {:?} {:?}",
            call.hop,
            h.stderr
        );
        assert!(
            to_lead(&h).is_empty(),
            "stamp {path:?}: no lane to a lead leaves: {:?}",
            to_lead(&h)
        );
        answer(&mut h, &call, "stop", warm_usage(), json!([said("Sunny.")]));
    }
    // Review A I1: a spec curate (`hop.spec` "1", the collector's
    // speculation) is parked without a lead, as before the move, where the
    // lead's empty knobs parked it (GH #1275). Without the park it ran as a
    // second full call beside the turn's normal one.
    let spec_calls = curate_for(
        &mut h,
        Some("north"),
        "t4",
        0,
        json!([user("and tomorrow?")]),
        json!({"spec": "1", "path": "", "needs": [], "families": []}),
    );
    assert!(
        spec_calls.is_empty(),
        "without a lead a spec curate is parked, no call: {spec_calls:?} {:?}",
        h.stderr
    );
    assert!(
        to_lead(&h).is_empty(),
        "a parked spec curate sends no lane to a lead: {:?}",
        to_lead(&h)
    );
    assert!(
        h.stderr.iter().any(|l| l.contains("parked, no call")),
        "the park is said on stderr: {:?}",
        h.stderr
    );
    assert!(
        h.lead_ops.is_empty(),
        "without a lead nobody touches its store: {:?}",
        h.lead_ops
    );
    assert!(
        marks_of_kind(&h, "lead").is_empty(),
        "without a lead no `lead` mark is booked: {:?}",
        marks_of_kind(&h, "lead")
    );
    assert!(
        marks_of_kind(&h, TARGET_MARK).is_empty(),
        "the named target is held, nothing is missing: {:?}",
        marks_of_kind(&h, TARGET_MARK)
    );
}
