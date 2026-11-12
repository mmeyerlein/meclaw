//! GH #1451 (D38 V2, fix round 1, review o15-Rv1 I1) -- the call whose start
//! window carried the open wellbeing is not fed it again.
//! Since D58 followup (GH #1479) against the hive `assistant` and the curator.
//!
//! WHY this file exists: the seed of a call carried the line and booked it
//! closed in the same bundle; the live feed of that call once compared
//! against a state that read the line as closed for the call too -- one
//! needless `live_feed` and, under `renew`, a seed rebuilt without it (the
//! cache head broken). The concern is now a note of scope `once` from the cell
//! `./followup` of `assistant`. The statements stay: a closed turn of the
//! carrying call sends no `live_feed` with the concern (a `once` note never
//! rides a live feed, note strand point 4); a renewed start window of the same
//! call carries it on and answers no second receipt; a later call does not
//! see it. Off, nothing is sent and nothing is shown.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/assistant_followup.rs"]
#[allow(unused_imports)]
use crate::assistant_followup; // harness_fold: mod assistant_followup;

use assistant_followup::*;
use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const CALL: &str = "call-9";
const CLAIM: &str = "Ada feels unwell and slept badly.";

fn unwell() -> Value {
    json!([fact("Ada", "has_symptom", CLAIM, None, true)])
}

/// Pin: seed with the concern, then a closed turn of the same call -- no
/// feed carries it; the renewal carries it on without a second receipt; a
/// later call sees none.
#[test]
fn the_carrying_call_is_not_fed_its_line_again() {
    if !shipped() {
        return;
    }
    let mut a = Assistant::on_at(NOON);
    a.leave("./talky", memory_out("text-1", "t1", unwell()));
    session_ends(&mut a, "./talky", "text-1");
    let mut h = curator(&[("policy", "live_feed", json!("1"))]);
    deliver_notes(&a, "./talky", &mut h);
    let first = seed(&mut h, CALL, "start", 0);
    assert_eq!(count_in(&first, CLAIM), 1, "{:?}", first.messages());
    assert_eq!(h.routed("note_receipt").len(), 1, "{:?}", h.out);
    closed_turn_of(&mut h, CALL, "t5");
    let fed: Vec<Msg> = h
        .routed("live_feed")
        .into_iter()
        .filter(|m| count_in(m, CLAIM) > 0 || format!("{:?}", m.body).contains(CLAIM))
        .collect();
    assert!(fed.is_empty(), "no feed with the concern: {fed:?}");
    let again = seed(&mut h, CALL, "renew", 1);
    assert_eq!(
        count_in(&again, CLAIM),
        1,
        "the renewal carries it on: {:?}",
        again.messages()
    );
    assert!(
        h.routed("note_receipt").is_empty(),
        "no second receipt: {:?}",
        h.out
    );
    let later = seed(&mut h, "call-10", "start", 0);
    assert_eq!(count_in(&later, CLAIM), 0, "{:?}", later.messages());
}

/// Lock: off (shipped) nothing is sent, nothing shown, nothing fed.
#[test]
fn off_no_line_and_no_feed() {
    if !shipped() {
        return;
    }
    let mut a = Assistant::with(&[(KNOB_CLOCK, json!(NOON))]);
    a.leave("./talky", memory_out("text-1", "t1", unwell()));
    session_ends(&mut a, "./talky", "text-1");
    assert!(a.into.is_empty(), "{:?}", a.into);
    let mut h = curator(&[("policy", "live_feed", json!("1"))]);
    let s = seed(&mut h, CALL, "start", 0);
    assert_eq!(count_in(&s, CLAIM), 0);
    closed_turn_of(&mut h, CALL, "t5");
    assert!(
        h.routed("live_feed")
            .iter()
            .all(|m| !format!("{:?}", m.body).contains(CLAIM)),
        "{:?}",
        h.routed("live_feed")
    );
}
