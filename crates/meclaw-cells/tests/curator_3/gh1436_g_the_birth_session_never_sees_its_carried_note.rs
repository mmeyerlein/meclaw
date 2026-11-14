//! GH #1436 (d54g) -- the birth session never sees its carried note.
//!
//! WHY this file exists: the carried note (`carry_notes`, curator/policy)
//! tells a LATER session that a matter was raised and answered (rule R-K,
//! no unasked return). In the session where the matter came up it would be
//! wrong: the person is still in it, and the note would sit beside the
//! crisis handling and its early interruption. The row names its birth
//! session in `not_session`; `live_notes` leaves such a row out of every
//! window of that session. The control: the very next session shows it.
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
        json!({"messages": [user("Tell me something.")]}),
    );
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "one call: {:?} {:?}", h.out, h.stderr);
    let call = calls[0].clone();
    h.tap(&call, "stop", json!({}), json!([said("Here I am.")]));
    call
}

fn shows(call: &Msg, text: &str) -> bool {
    texts(call).iter().any(|t| t.contains(text))
}

#[test]
fn the_birth_session_never_sees_its_carried_note() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "notes_share", json!(0.04)),
        ("policy", "carry_notes", carry()),
    ]);
    let crisis_turn = talk(&mut h, "s1", "t1", crisis());
    assert!(
        !shows(&crisis_turn, CARRY),
        "the crisis turn itself: {:?}",
        texts(&crisis_turn)
    );
    let born = h.rows("SELECT not_session FROM notes WHERE source = 'carry.route.risk'");
    assert_eq!(born, vec![vec![json!("s1")]], "{:?}", h.stderr);
    for t in ["t2", "t3"] {
        let same = talk(&mut h, "s1", t, json!([]));
        assert!(
            !shows(&same, CARRY),
            "turn {t} of the birth session does not see the carried note: {:?}",
            texts(&same)
        );
    }
    let later = talk(&mut h, "s2", "t1", json!([]));
    assert!(
        shows(&later, CARRY),
        "the next session sees it: {:?}",
        texts(&later)
    );
}
