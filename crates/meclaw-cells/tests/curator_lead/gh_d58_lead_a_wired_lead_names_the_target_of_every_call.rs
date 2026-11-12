//! D58 lead (GH #1474, contract points 4 and 5) -- a wired lead names the
//! target of every call.
//!
//! WHY this file exists: where a composite wires a lead beside its curator
//! (talky), the lead's verdict decides the target of each call and the
//! curate's own `hop.target` is only what the policy falls back to when the
//! verdict names none. With the lead on the far side of the curator's
//! boundary, every call must still be asked about (one `lead_ask` out, the
//! verdict home on `in_lead`), and every call must leave under the target
//! the verdict named -- for each stamp the composite's `path_routes` knows,
//! and for one it does not (`*`). Each call books its `lead` mark.
//!
//! Runs the shipped curator hive with the lead wired the way talky wires it
//! (`support/curator_hive.rs`); no model.

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
fn gh_d58_lead_a_wired_lead_names_the_target_of_every_call() {
    if !shipped() {
        return;
    }
    // The routes of talky's lead: talk -> brain, simple -> quick, every
    // other stamp -> brain (`*`). The curate names `north` every time.
    let mut h = wired(&[(
        "lead",
        "path_routes",
        json!("talk:brain,simple:quick,*:brain"),
    )]);
    for t in ["brain", "quick", "north"] {
        announce_profile(&mut h, t, json!({}));
    }
    let mut calls_made = 0;
    for (turn, path, want) in [
        ("t1", "talk", "brain"),
        ("t2", "simple", "quick"),
        ("t3", "unheard", "brain"),
    ] {
        let lead_ops_before = h.lead_ops.len();
        let calls = curate_for(
            &mut h,
            Some("north"),
            turn,
            0,
            json!([user("hello there")]),
            json!({"path": path, "needs": [], "families": []}),
        );
        let call = the_call(&h, &calls);
        assert_eq!(
            call.hop.get("target"),
            Some(&json!(want)),
            "stamp {path:?}: the call leaves under the target the verdict named: {:?} {:?}",
            call.hop,
            h.stderr
        );
        assert!(
            h.lead_ops.len() > lead_ops_before,
            "stamp {path:?}: the lead was asked and read its rows"
        );
        calls_made += 1;
        answer(&mut h, &call, "stop", warm_usage(), json!([said("Hello.")]));
    }
    assert_eq!(
        marks_of_kind(&h, "lead").len(),
        calls_made,
        "one `lead` mark per call: {:?}",
        marks_of_kind(&h, "lead")
    );
}
