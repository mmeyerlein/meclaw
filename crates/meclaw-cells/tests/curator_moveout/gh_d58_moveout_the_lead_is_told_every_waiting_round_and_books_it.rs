//! D58 moveout (GH #1476, plan `u-moveout.md` contract point 7; plan review
//! W4) -- the lead is told every waiting round and books it.
//!
//! WHY this file exists: a family without a default learns its first
//! duration only because the round is booked even when no seconds were said
//! (the turn row was written with `seconds: null`). If the lead heard of a
//! round only where it put a feed into the verdict, such a family would
//! never be booked and never advised. So the policy tells the lead on
//! `lead_note` of EVERY round in which one of the two conditions held, with
//! the condition and the time of the seam, whether a feed went or not; the
//! end rides the existing note `turn_end`; the lead books the durations in
//! its own store (no `horizon_rounds` in the curator's ledger any more).
//! Under both conditions: a tool round (`must_call`) and an errand
//! (`external`), with and without a feed. And per turn (review B M-2): two
//! waiting rounds of one session that overlap keep a clock each and are
//! both booked -- one row per session lost the first round's clock to the
//! second.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_moveout.rs"]
#[allow(unused_imports)]
use crate::curator_moveout; // harness_fold: mod curator_moveout;

use curator_hive::*;
use curator_moveout::*;
use meclaw_core::serde_json::json;

fn curator_keys(h: &Hive, prefix: &str) -> Vec<String> {
    h.rows("SELECT key FROM state ORDER BY key")
        .iter()
        .filter_map(|r| r[0].as_str().map(str::to_string))
        .filter(|k| k.starts_with(prefix))
        .collect()
}

#[test]
fn gh_d58_moveout_the_lead_is_told_every_waiting_round_and_books_it() {
    if !shipped() {
        return;
    }
    let mut h = moveout_hive(&horizon_knobs(json!({"weather": 2000})));
    // A family without a default: nothing is said, the round is told all the same.
    seam(
        &mut h,
        "t1",
        "0",
        tool_stamp("orga"),
        json!({"messages": [user("what is on my list today?")]}),
    );
    assert!(
        horizons(&h).is_empty(),
        "no seconds known, none said: {:?}",
        h.out
    );
    assert_eq!(
        side_calls(&h).len(),
        1,
        "a waiting round: {:?} {:?}",
        h.out,
        h.stderr
    );
    finish_round(&mut h, "t1", "orga");

    // Booked in the lead's own store, nothing in the curator's ledger.
    assert_eq!(
        booked(&h, "orga").len(),
        1,
        "the lead booked the round of `orga`: {:?}",
        h.lead_rows("SELECT key, value FROM state ORDER BY key")
    );
    assert!(
        curator_keys(&h, "pending:horizon:").is_empty(),
        "no turn row in the ledger"
    );
    assert!(
        curator_keys(&h, "horizon_rounds").is_empty(),
        "no durations in the ledger"
    );

    // The family has learned its first duration: the next round says it.
    seam(
        &mut h,
        "t2",
        "0",
        tool_stamp("orga"),
        json!({"messages": [user("and tomorrow?")]}),
    );
    let feeds = horizons(&h);
    assert_eq!(
        feeds.len(),
        1,
        "the booked round gives the advise: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert_eq!(feeds[0].body["family"], json!("orga"));
    assert!(feeds[0].body["seconds"].is_u64(), "{:?}", feeds[0].body);
    let t2_call = required_call(&h);

    // A second waiting round of the session while t2's still runs: a clock each.
    seam(
        &mut h,
        "t3",
        "0",
        tool_stamp("weather"),
        json!({"messages": [user("and in Hamburg?")]}),
    );
    assert_eq!(horizons(&h).len(), 1, "{:?} {:?}", h.out, h.stderr);
    let t3_call = required_call(&h);
    assert_eq!(
        table_turns(&h, "horizon_turn:s1"),
        vec!["t2".to_string(), "t3".to_string()],
        "two rounds of one session at once, a clock each: {:?}",
        h.lead_rows("SELECT key, value FROM state ORDER BY key")
    );
    h.out.clear();
    h.out.push(t2_call);
    finish_round(&mut h, "t2", "orga");
    h.out.clear();
    h.out.push(t3_call);
    finish_round(&mut h, "t3", "weather");
    assert_eq!(
        (booked(&h, "orga").len(), booked(&h, "weather").len()),
        (2, 1),
        "both overlapping rounds are booked: {:?}",
        h.lead_rows("SELECT key, value FROM state ORDER BY key")
    );
    assert!(
        h.lead_state("horizon_turn:s1").is_empty(),
        "every clock taken down with its booking"
    );

    // Under `external` without a feed (no seconds known for `web`): the lead
    // is told of the round all the same.
    seam(
        &mut h,
        "t4",
        "0",
        errand_stamp("complex"),
        json!({"messages": [user("plan my week around the move")]}),
    );
    assert!(
        horizons(&h).is_empty(),
        "no seconds known, none said: {:?}",
        h.out
    );
    assert_eq!(
        side_calls(&h).len(),
        1,
        "a waiting round under `external`: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert_eq!(
        table_turns(&h, "horizon_turn:s1"),
        vec!["t4".to_string()],
        "the lead was told of the errand's round: {:?}",
        h.lead_rows("SELECT key, value FROM state ORDER BY key")
    );
}

/// The required call of the round that just left (not its side call).
fn required_call(h: &Hive) -> Msg {
    let calls: Vec<Msg> = main_calls(h)
        .into_iter()
        .filter(|m| m.route() == "model_call")
        .collect();
    assert_eq!(calls.len(), 1, "{:?} {:?}", h.out, h.stderr);
    calls[0].clone()
}
