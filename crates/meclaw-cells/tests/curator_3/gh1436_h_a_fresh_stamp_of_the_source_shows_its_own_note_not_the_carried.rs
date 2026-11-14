//! GH #1436 (d54g) -- a fresh stamp of the source shows its own note, not
//! the carried one.
//!
//! WHY this file exists: a carried note says "raised earlier and answered;
//! do not bring it up unasked". When a later session stamps the same source
//! again (a new crisis), that sentence is wrong for this turn: the turn's
//! window shows the fresh note alone, and the carried note never holds back
//! a crisis note (#1512: no crisis note is lost in the first turn after an
//! acute statement). The fresh turn leaves its own carried note, born in its
//! own session.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const NOTE: &str = "The person may be in danger: answer with care.";
const CARRY: &str = "Earlier the person spoke of a crisis and it was answered. Do not bring it up unasked; if they come back to it, stay with it.";

fn carry() -> Value {
    json!({"route.risk": {"text": CARRY, "until": "day"}})
}

fn crisis() -> Value {
    json!([{"id": "route.risk", "source": "route.risk", "text": NOTE, "priority": 0}])
}

fn talk(h: &mut Hive, session: &str, turn: &str, notes: Value) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": session, "turn_id": turn, "iter": "0", "phase": "",
                     "path": "talk", "needs": "", "families": ""});
    let ctx = json!({"session_id": session, "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "talk",
                     "turn_notes": notes});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("I cannot go on.")]}),
    );
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "one call: {:?} {:?}", h.out, h.stderr);
    let call = calls[0].clone();
    h.tap(&call, "stop", json!({}), json!([said("I am with you.")]));
    call
}

fn shows(call: &Msg, text: &str) -> bool {
    texts(call).iter().any(|t| t.contains(text))
}

#[test]
fn a_fresh_stamp_of_the_source_shows_its_own_note_not_the_carried() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "notes_share", json!(0.04)),
        ("policy", "carry_notes", carry()),
    ]);
    talk(&mut h, "s1", "t1", crisis());
    let born = h.rows("SELECT not_session FROM notes WHERE source = 'carry.route.risk'");
    assert_eq!(born, vec![vec![json!("s1")]], "{:?}", h.stderr);

    // A later session, its first turn stamped acute again.
    let fresh = talk(&mut h, "s2", "t1", crisis());
    assert!(
        shows(&fresh, NOTE),
        "the fresh crisis note stands: {:?}",
        texts(&fresh)
    );
    assert!(
        !shows(&fresh, CARRY),
        "the carried note gives way to the fresh one: {:?}",
        texts(&fresh)
    );
    let reborn = h.rows("SELECT not_session FROM notes WHERE source = 'carry.route.risk'");
    assert_eq!(
        reborn,
        vec![vec![json!("s2")]],
        "one carried note per source, now born in s2"
    );
    // Its own session does not see the renewed note either.
    let next = talk(&mut h, "s2", "t2", json!([]));
    assert!(!shows(&next, CARRY), "{:?}", texts(&next));
}
