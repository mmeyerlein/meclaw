//! D58 note (GH #1477, contract point 4) -- a `once` note never goes to a
//! running session as a live feed.
//!
//! WHY this file exists: the policy tells a running live session every note
//! that is not in its held seed state (`live_feed` `renew`, through the twin
//! of `seed_notes`). A `once` note belongs to the next start window, never to
//! the session that is already running (`u-followup.md` contract 4); fed in,
//! it would stand in a second window. The turn's own window shows it; the
//! feed of the same closed turn carries the new `session` note, not the
//! `once` one.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/note_once.rs"]
#[allow(unused_imports)]
use crate::note_once; // harness_fold: mod note_once;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};
use note_once::*;

const ONCE: &str = "Tell them the lab results are fine.";
const SESSION: &str = "The daughter visits this afternoon.";

/// A turn of a running session: since D58 live (GH #1482) the curator knows
/// it only by the handle `live` the composition sets.
fn live_seam(h: &mut Hive, turn: &str) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": "talk", "needs": "", "families": "",
                     "live": "call-9"});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "talk"});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("and then?")]}),
    );
}

#[test]
fn a_once_note_never_rides_a_live_feed() {
    if !shipped() {
        return;
    }
    let mut h = once_hive(&[("policy", "live_feed", json!("1"))]);
    clear_of_midnight();
    // A first closed turn of the running session: its state is held.
    live_seam(&mut h, "t1");
    let c = h.routed("brain")[0].clone();
    h.tap(&c, "stop", json!({}), json!([said("fine")]));

    post(
        &mut h,
        json!([
            once("n1", ONCE, Some("r-1")),
            note("n2", "session", SESSION)
        ]),
    );
    live_seam(&mut h, "t2");
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert!(
        shows(&calls[0], ONCE),
        "the turn's window: {:?}",
        texts(&calls[0])
    );
    h.out.clear();
    h.tap(&calls[0], "stop", json!({}), json!([said("fine")]));
    let feeds = h.routed("live_feed");
    assert_eq!(feeds.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let fed = feeds[0].body.get("notes").cloned().unwrap_or(Value::Null);
    assert!(fed.to_string().contains(SESSION), "the session note: {fed}");
    assert!(
        !fed.to_string().contains(ONCE),
        "never the once note: {fed}"
    );
}
