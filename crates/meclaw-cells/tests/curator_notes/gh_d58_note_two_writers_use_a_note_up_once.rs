//! D58 note (GH #1477, contract point 2) -- of two writers that use the same
//! note up, exactly one wins.
//!
//! WHY this file exists: two windows of one round (two sessions of the same
//! person, or a turn beside a start window) can read the same open `once`
//! note before either books it (plan review W5, W7). The use is a
//! conditional write of `used_turn` -- only while the field is empty, the
//! pattern of the `turn` note's use -- so the first write to reach the store
//! wins and the second changes nothing. Both may answer (contract point 7:
//! the sender takes several receipts); the note is used up by one key.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/note_once.rs"]
#[allow(unused_imports)]
use crate::note_once; // harness_fold: mod note_once;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};
use note_once::*;

const TEXT: &str = "Ask about the new glasses.";

fn curate_msg(session: &str) -> (&'static str, Value, Value, Value) {
    (
        "in_curate",
        json!({"session_id": session, "turn_id": "t1", "iter": "0", "channel": "test",
               "audience_set": ROUND_E, "path": "talk"}),
        json!({"session_id": session, "turn_id": "t1", "iter": "0", "phase": "",
               "path": "talk", "needs": "", "families": ""}),
        json!({"messages": [user("hello")]}),
    )
}

/// The `used_turn` values the ledger was asked to set on `notes`, in the
/// order the store took them, each with whether it asked for an empty field.
fn uses(h: &Hive) -> Vec<(String, bool)> {
    h.ledger_ops
        .iter()
        .map(|(_, op)| op)
        .filter(|op| op["table"] == "notes")
        .filter_map(|op| {
            let set = op["set"]["used_turn"].as_str()?;
            Some((set.to_string(), op["where"]["used_turn"] == json!("")))
        })
        .filter(|(set, _)| !set.is_empty())
        .collect()
}

#[test]
fn two_writers_use_a_note_up_once() {
    if !shipped() {
        return;
    }
    let mut h = once_hive(&[]);
    post(&mut h, json!([once("n1", TEXT, Some("r-1"))]));
    h.out.clear();
    h.ledger_ops.clear();
    // Two sessions of one round curate at once: both read before either writes.
    h.lanes_together(vec![curate_msg("s1"), curate_msg("s2")]);
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 2, "{:?} {:?}", h.out, h.stderr);
    assert!(
        calls.iter().all(|c| shows(c, TEXT)),
        "both read it open: {:?}",
        calls.iter().map(texts).collect::<Vec<_>>()
    );

    // Both ask to use it up, each only while the field is empty; the first
    // to reach the store wins and the second changes nothing.
    let asked = uses(&h);
    assert_eq!(asked.len(), 2, "{asked:?} {:?}", h.ledger_ops);
    assert!(
        asked.iter().all(|(_, cond)| *cond),
        "conditional: {asked:?}"
    );
    assert_eq!(used(&h, "n1"), asked[0].0, "the first write holds");

    // No later window of either session shows it again.
    for s in ["s1", "s2"] {
        let c = h.curate(s, "t2", 0, json!([user("and?")]), mode("Be brief."));
        assert!(!shows(&c, TEXT), "{s}: {:?}", texts(&c));
    }
}
