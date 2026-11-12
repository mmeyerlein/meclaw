//! GH #1447 (D38 C1, OR-38-74) -- `win` waits for the ledger and the lead.
//!
//! WHY this file exists: R-38-4 Nr. 6 keeps the phone's latency per station
//! at R039 or below, so the lead must not be one more hop in a row. Its ask
//! leaves beside the ledger round trip of `win-hashes`, and `win` starts when
//! BOTH are in -- the critical path is the longer of the two, not their sum.
//! A join holds only if it holds in either order: here the lead's verdict is
//! held back once (the ledger legs come first) and the ledger's answer once
//! (the verdict comes first). In both orders no model call leaves while one
//! half is out, the call stays parked under `pending:<call>` meanwhile, and
//! exactly one call leaves after -- the same call. And the join ends without
//! the policy's RAM too (a fresh child for every message, OR-d38-3.o9.4).
//! Since GH #1474 the lead is a hive of its own beside the curator, wired by
//! talky's shipped edges; its verdict is held on its lane `in_lead` at the
//! curator's rim.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The parked calls of the ledger (`pending:<call>`, without the turn rows
/// that share the prefix).
fn parked(h: &Hive) -> Vec<String> {
    h.rows("SELECT key FROM state WHERE key LIKE 'pending:%'")
        .into_iter()
        .filter_map(|r| r[0].as_str().map(str::to_string))
        .filter(|k| {
            ![
                "pending:horizon:",
                // D58 feed (GH #1481): the one claim of a feed replaced the
                // claims `pending:interrupt:` and `pending:fact_tool:`.
                "pending:feed:",
                "pending:delegation:",
            ]
            .iter()
            .any(|p| k.starts_with(p))
                && !k.ends_with("-esc")
        })
        .collect()
}

/// The half of `win` a run holds back.
#[derive(Clone, Copy, Debug)]
enum Half {
    /// The ledger's answer to the reads of `win`.
    Ledger,
    /// The lead's verdict, on its lane `in_lead` at the curator's rim
    /// (GH #1474: the lead is a hive of its own, no inner edge carries it).
    Verdict,
}

/// One talk turn with `hold` held back; returns the brain call's messages
/// after the held half was let through.
fn joined(hold: Half, cold: bool) -> Value {
    assert!(
        lead_shipped(),
        "templates/lead/lead/config.json is missing: the lead is not shipped (GH #1447, GH #1474)"
    );
    let mut h = Hive::with(&[]).reporting_composite("talky");
    assert!(
        h.wired,
        "the harness wires the lead as talky ships it (GH #1474)"
    );
    if cold {
        h.run_cold("policy");
    }
    turn(&mut h, "s1", "t0", "hello", "hi", json!({}));
    h.out.clear();
    match hold {
        Half::Ledger => h.hold = Some(("./ledger".to_string(), Some("win".to_string()))),
        Half::Verdict => h.hold_lane = Some(VERDICT_LANE.to_string()),
    }
    let hop = json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "phase": "",
                     "path": "talk", "needs": [], "families": []});
    let ctx = json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "channel": "test",
                     "audience_set": ROUND_E, "path": "talk"});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("how are you?")], "system": mode("Be brief.")}),
    );
    assert!(
        !h.held.is_empty(),
        "{hold:?}: the held half never came -- the lead is not asked beside the ledger"
    );
    assert!(
        h.routed("brain").is_empty(),
        "{hold:?}: a model call left while one half was out: {:?}",
        h.out
    );
    assert!(
        !parked(&h).is_empty(),
        "{hold:?}: the call waits parked under pending:<call>"
    );
    h.release_held();
    let brain = h.routed("brain");
    assert_eq!(
        brain.len(),
        1,
        "{hold:?}: one call after the join: {:?} {:?}",
        h.out,
        h.stderr
    );
    brain[0]
        .body
        .get("messages")
        .cloned()
        .unwrap_or(Value::Null)
}

#[test]
fn win_starts_once_the_ledger_and_the_lead_are_in_whichever_comes_first() {
    if !shipped() {
        return;
    }
    let legs_first = joined(Half::Verdict, false);
    let verdict_first = joined(Half::Ledger, false);
    assert_eq!(
        legs_first, verdict_first,
        "the join's order changes nothing in the call"
    );
}

/// OR-d38-3.o9.4: the join holds without the policy's RAM -- every message a
/// fresh child (a child restarted after each one). In either order the call
/// reaches the model exactly once, with the body it has when the child keeps
/// its RAM; the hive comes to rest (no ask, verdict, reread round without end).
#[test]
fn the_join_ends_without_the_policys_ram_in_either_order() {
    if !shipped() {
        return;
    }
    let resident = joined(Half::Verdict, false);
    let legs_first = joined(Half::Verdict, true);
    let verdict_first = joined(Half::Ledger, true);
    assert_eq!(
        legs_first, resident,
        "cold, legs first: the call of the resident child"
    );
    assert_eq!(
        verdict_first, resident,
        "cold, verdict first: the call of the resident child"
    );
}
