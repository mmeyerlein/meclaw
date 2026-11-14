//! GH #1436 (d54g) -- a carried note born in a running call moves no feed.
//!
//! WHY this file exists: a running call is fed what moved in the facts of
//! its start window (`live_feed`, #1279: `./history` keeps the facts of the
//! seed it sent, `./policy` compares a closed turn's facts against them and
//! feeds what moved). A carried note is a fact of a seed -- scope `until`
//! -- but never of the session it was born in (`not_session`). Were it a
//! fact of the feed, the crisis call itself would be told "raised earlier,
//! answered, do not bring it up" while it is still in the crisis, and the
//! digest would part on every closed turn (the C2 class of
//! `gh1436_e_an_aged_seed_is_not_fed_back`). `seed_notes` leaves the row out
//! in its birth session, in both cells (one text, pinned in
//! `curator_cells`): no feed, and the call's facts stay the seed's.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`) and
//! `./history` alone for the seed (`support/history_judge.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/history_judge.rs"]
#[allow(unused_imports)]
use crate::history_judge; // harness_fold: mod history_judge;

use curator_hive::*;
use history_judge::{history_params, only, seed_ask, seed_ask_for};
use meclaw_core::serde_json::{Value, json};

const CALL: &str = "call-9";
const NOTE: &str = "The person may be in danger: answer with care.";
const CARRY: &str = "Earlier the person spoke of a crisis and it was answered. Do not bring it up unasked; if they come back to it, stay with it.";

fn crisis() -> Value {
    json!([{"id": "route.risk", "source": "route.risk", "text": NOTE, "priority": 0}])
}

/// One closed turn of the running call's session `s1` (handle `live`), with
/// `notes` as its turn notes: its curate and the model's answer.
fn closed_turn(h: &mut Hive, turn: &str, notes: Value) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": "talk", "needs": "", "families": "", "live": CALL});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "talk",
                     "turn_notes": notes});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("I do not want to go on.")]}),
    );
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let call = calls[0].clone();
    h.out.clear();
    h.tap(&call, "stop", json!({}), json!([said("I am with you.")]));
    call
}

#[test]
fn a_carried_note_born_in_a_running_call_moves_no_feed() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "notes_share", json!(0.04)),
        ("policy", "live_feed", json!("1")),
        (
            "policy",
            "carry_notes",
            json!({"route.risk": {"text": CARRY, "until": "day"}}),
        ),
    ])
    .reporting_composite("talky");
    let params = history_params(&[], &["now_utc"]);
    let seed = only(&seed_ask(&h, &params, CALL), "seed");
    let held = h.state(&format!("seed_facts:{CALL}"));
    assert!(
        !held.is_empty(),
        "the seed keeps its facts: {:?}",
        seed.body
    );

    closed_turn(&mut h, "t1", crisis());
    let born = h.rows("SELECT not_session FROM notes WHERE source = 'carry.route.risk'");
    assert_eq!(
        born,
        vec![vec![json!("s1")]],
        "the crisis turn of the call left its carried note: {:?}",
        h.stderr
    );
    assert!(
        h.routed("live_feed").is_empty(),
        "no feed from the crisis turn: {:?}",
        h.routed("live_feed")
    );
    // The next closed turn reads the row the crisis turn wrote.
    let next = closed_turn(&mut h, "t2", json!([]));
    assert!(
        !texts(&next).iter().any(|t| t.contains(CARRY)),
        "the call's own window does not show it: {:?}",
        texts(&next)
    );
    assert!(
        h.routed("live_feed").is_empty(),
        "the carried note moves no feed into the call it was born in: {:?}",
        h.routed("live_feed")
    );
    assert_eq!(
        h.state(&format!("seed_facts:{CALL}")),
        held,
        "the call's facts stay the seed's"
    );
}

/// One closed turn of the running call `CALL` under the generation's session
/// `session` (a voice call opens a new one with every provider session, the
/// call keeps its handle): its curate and the model's answer.
fn call_turn(h: &mut Hive, session: &str, turn: &str, notes: Value) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": session, "turn_id": turn, "iter": "0", "phase": "",
                     "path": "talk", "needs": "", "families": "", "live": CALL});
    let ctx = json!({"session_id": session, "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "talk",
                     "turn_notes": notes});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("I do not want to go on.")]}),
    );
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let call = calls[0].clone();
    h.out.clear();
    h.tap(&call, "stop", json!({}), json!([said("I am with you.")]));
    call
}

/// WHY: the seed of a call carries the call's id as its session (`seed_ask`),
/// the call's turns the session of their generation -- two ids for one call.
/// A carried note compared by the turn's session alone showed in the renewed
/// start window of the very call it was born in, and in the window of the
/// call's next generation: the person still in the crisis would be told it
/// was answered. The note names the call it was born in, and the seed, the
/// feed and the window all judge by it. The seed here carries the call's id
/// `CALL`, the turns `s1` and `s2`; a later call still sees the note.
#[test]
fn a_carried_note_hides_from_every_seed_and_window_of_its_call() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "notes_share", json!(0.04)),
        ("policy", "live_feed", json!("1")),
        (
            "policy",
            "carry_notes",
            json!({"route.risk": {"text": CARRY, "until": "day"}}),
        ),
    ])
    .reporting_composite("talky");
    let params = history_params(&[], &["now_utc"]);
    only(&seed_ask(&h, &params, CALL), "seed");

    call_turn(&mut h, "s1", "t1", crisis());
    let born = h.rows("SELECT count(*) FROM notes WHERE source = 'carry.route.risk'");
    assert_eq!(
        born,
        vec![vec![json!(1)]],
        "the crisis turn left its carried note: {:?}",
        h.stderr
    );

    // The call renews its provider session: the renewed start window.
    let renewed = only(&seed_ask_for(&h, &params, CALL, "renew", 1), "seed");
    let body = meclaw_core::serde_json::to_string(&renewed.body).expect("the seed's body");
    assert!(
        !body.contains(CARRY),
        "the renewed start window of the birth call does not carry it: {body}"
    );

    // The call's next generation: a turn under a new session, same handle.
    let next = call_turn(&mut h, "s2", "t1", json!([]));
    assert!(
        !texts(&next).iter().any(|t| t.contains(CARRY)),
        "the window of the call's next generation does not show it: {:?}",
        texts(&next)
    );
    assert!(
        h.routed("live_feed").is_empty(),
        "no feed of the carried note into its own call: {:?}",
        h.routed("live_feed")
    );

    // A later call: its start window carries the note.
    let later = only(&seed_ask(&h, &params, "call-10"), "seed");
    let body = meclaw_core::serde_json::to_string(&later.body).expect("the seed's body");
    assert!(
        body.contains(CARRY),
        "the start window of a later call carries it: {body}"
    );
}
