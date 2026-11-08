//! GH #1451 (D38 V2, fix round 1, review o15-Rv1 I1) -- the call whose seed
//! carried the open wellbeing keeps it in its facts: a closed turn of that
//! call sends no `live_feed`.
//!
//! WHY this file exists: the seed of a call carries the line and, in the same
//! bundle, books it `closed` (OR-d38-3.v.2); it writes the digest WITH the
//! line under `seed_facts:<call>`. The live feed of that call compared
//! against a state that read the closing mark as closed for the call too --
//! no line, another digest, one needless `live_feed` and, under `renew`, a
//! seed rebuilt without the line (the cache head broken). The call's own
//! closing mark does not close the line for the call; every later call is
//! closed. Pattern of `gh1436_e_an_aged_seed_is_not_fed_back.rs`.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/history_judge.rs"]
#[allow(unused_imports)]
use crate::history_judge; // harness_fold: mod history_judge;

use curator_hive::*;
use history_judge::{history_params, only, seed_ask};
use meclaw_core::serde_json::{Value, json};

const CALL: &str = "call-9";

fn closed_turn(h: &mut Hive, turn: &str) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": "talk", "needs": "", "families": "",
                     "engine": "duplex", "call_id": CALL});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "talk"});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("and then?")]}),
    );
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "{:?} {:?}", h.out, h.stderr);
    h.out.clear();
    h.tap(&calls[0], "stop", json!({}), json!([said("fine")]));
}

fn unwell_today(knob: &str) -> Hive {
    clear_of_midnight();
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "live_feed", json!("1")),
        ("policy", "wellbeing_line", json!(knob)),
        ("history", "wellbeing_line", json!(knob)),
    ]);
    let at = backdated(30);
    h.row(at, "text-1", "t1", "user", &user("I feel unwell"), 1);
    h.mark(
        at + chrono::Duration::seconds(1),
        "text-1",
        "t1",
        "fact",
        &canonical(&json!({"subject": "Ada", "predicate": "has_symptom",
                           "claim": "Ada feels unwell and slept badly.",
                           "source": "person", "stance": "asserted",
                           "wellbeing": true})),
    );
    h
}

fn wellbeing_lines(seed: &Msg) -> usize {
    seed.messages()
        .iter()
        .filter_map(|m| m["text"].as_str())
        .filter(|t| t.starts_with("Wellbeing: "))
        .count()
}

/// Pin: seed with the open line, then a closed turn of the same call -- no
/// feed, the call's facts stay the seed's; a later call sees no line.
#[test]
fn the_carrying_call_is_not_fed_its_line_again() {
    if !shipped() {
        return;
    }
    let mut h = unwell_today("1");
    let on = history_params(&[("wellbeing_line", json!("1"))], &["now_utc"]);
    let seed = only(&seed_ask(&h, &on, CALL), "seed");
    assert_eq!(wellbeing_lines(&seed), 1, "{:?}", seed.messages());
    let held = h.state(&format!("seed_facts:{CALL}"));
    assert!(
        held.contains("wellbeing"),
        "the seed's facts carry it: {held}"
    );
    closed_turn(&mut h, "t5");
    assert!(
        h.routed("live_feed").is_empty(),
        "no feed for the line the call already has: {:?}",
        h.routed("live_feed")
    );
    assert_eq!(h.state(&format!("seed_facts:{CALL}")), held);
    let later = only(&seed_ask(&h, &on, "call-10"), "seed");
    assert_eq!(wellbeing_lines(&later), 0, "{:?}", later.messages());
}

/// Lock: a renewed seed of the carrying call carries the line on and books
/// no second closing mark.
#[test]
fn a_renewed_seed_of_the_call_books_once() {
    if !shipped() {
        return;
    }
    let h = unwell_today("1");
    let on = history_params(&[("wellbeing_line", json!("1"))], &["now_utc"]);
    let first = only(&seed_ask(&h, &on, CALL), "seed");
    let again = only(&seed_ask(&h, &on, CALL), "seed");
    assert_eq!(wellbeing_lines(&first), 1);
    assert_eq!(wellbeing_lines(&again), 1, "{:?}", again.messages());
    let marks: Vec<Vec<Value>> = h.rows("SELECT value FROM marks WHERE kind = 'wellbeing'");
    assert_eq!(marks.len(), 1, "{marks:?}");
}

/// Lock: off (shipped) no line, no feed.
#[test]
fn off_no_line_and_no_feed() {
    if !shipped() {
        return;
    }
    let mut h = unwell_today("0");
    let off = history_params(&[], &["now_utc"]);
    let seed = only(&seed_ask(&h, &off, CALL), "seed");
    assert_eq!(wellbeing_lines(&seed), 0);
    closed_turn(&mut h, "t5");
    assert!(h.routed("live_feed").is_empty());
}
