//! D58 lead (GH #1474, contract point 7) -- the ask leaves with the ledger
//! reads.
//!
//! WHY this file exists: R-38-4 Nr. 6 keeps the latency of a turn at its
//! station times, so the switch must never be one more round trip in a row:
//! the ask leaves in the same emission as the ledger reads of `win-hashes`,
//! and `win` starts when both halves are in. With the switch a hive of its
//! own the ask takes two more routing steps each way (two hive boundaries,
//! no cell run), and nothing else may change: while one half is held -- the
//! ledger's answer to the reads of `win`, or the lead's verdict on its lane
//! `in_lead` at the curator's rim -- the lead has already been asked and has
//! read its rows, no model call has left, none of the three lanes to the
//! lead is counted as output of the curator, and once the half comes exactly
//! one call leaves, the same in both orders. No time is measured -- the
//! order of the steps is the proof.
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

/// One `talk` turn of a wired lead with one half of `win` held: the
/// ledger's answer to the reads of `win` (`ledger`) or the lead's verdict
/// on its lane `in_lead` (`verdict`). Returns the one model call.
fn joined(hold_the_verdict: bool) -> Msg {
    let mut h = wired(&[]);
    assert!(h.wired, "the harness wires the lead as talky ships it");
    announce_profile(&mut h, "brain", json!({}));
    let first = say(&mut h, Some("brain"), "t0", "hello");
    answer(&mut h, &first, "stop", warm_usage(), json!([said("hi")]));

    h.out.clear();
    if hold_the_verdict {
        // GH #1474: the verdict reaches the curator over its rim; it is held
        // on its lane, not at an edge inside a hive.
        h.hold_lane = Some(FROM_LEAD.to_string());
    } else {
        h.hold = Some(("./ledger".to_string(), Some("win".to_string())));
    }
    let lead_ops_before = h.lead_ops.len();
    let hop = json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "phase": "",
                     "target": "brain", "path": "talk", "needs": [], "families": []});
    let ctx = json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "channel": "test",
                     "audience_set": ROUND_E, "path": "talk"});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("how are you?")], "system": system_tree()}),
    );
    let which = if hold_the_verdict {
        "the verdict"
    } else {
        "the ledger's answer"
    };
    assert!(
        !h.held.is_empty(),
        "{which} to the call is held: {:?}",
        h.stderr
    );
    if hold_the_verdict {
        assert!(
            h.held.iter().all(|m| m.route() == FROM_LEAD),
            "only the verdict is held: {:?}",
            h.held
        );
    }
    assert!(
        h.lead_ops.len() > lead_ops_before,
        "the lead was asked and read its rows in the same pass as the ledger reads -- \
         the ask left beside them, not after"
    );
    assert!(
        model_calls(&h).is_empty(),
        "no model call leaves while one half is out: {:?}",
        model_calls(&h)
    );
    assert!(
        to_lead(&h).is_empty(),
        "the lanes to the lead are no output of the curator: the composite's \
         bridge takes them: {:?}",
        to_lead(&h)
    );
    h.release_held();
    let calls = model_calls(&h);
    assert_eq!(
        calls.len(),
        1,
        "exactly one call once both halves are in: {:?} {:?}",
        h.out,
        h.stderr
    );
    calls[0].clone()
}

#[test]
fn gh_d58_lead_the_ask_leaves_with_the_ledger_reads() {
    if !shipped() {
        return;
    }
    let verdict_first = joined(false);
    let legs_first = joined(true);
    assert_eq!(
        verdict_first.body.get("messages"),
        legs_first.body.get("messages"),
        "the order of the two halves changes nothing in the call"
    );
}
