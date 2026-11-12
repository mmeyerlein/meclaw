//! GH #1436 (fix round 1, review C2) -- a seed that aged the topic out is not
//! undone by the live feed: after the seed of a call said no topic, a closed
//! turn of that session sends no `live_feed` and leaves `seed_facts:<live>` as
//! the seed wrote it.
//!
//! WHY this file exists: `./history` writes the facts of the seed it sent
//! under `seed_facts:<live>`, `./policy` compares the facts of a closed live
//! turn against that row and feeds what moved (#1279, "one source"). With the
//! aging in `./history` alone the policy read the newest named topic of the
//! round, ungated by age: the digests parted on every closed turn, the feed
//! said `topic: crisis` into the call, and with section `renew` the voice
//! renewed its session, asked for a seed that said no topic again -- a feed
//! and a cold renewal on every turn. Both cells age by one function
//! (`topic_aged`) behind one switch (`topic_aging`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;
// harness_fold: #[path = "support/history_judge.rs"]
#[allow(unused_imports)]
use crate::history_judge; // harness_fold: mod history_judge;

use curator_hive::*;
use history_judge::{aging_live, fact_line, history_params, only, seed_ask};
use meclaw_core::serde_json::{Value, json};

const CALL: &str = "call-9";

fn seam(h: &mut Hive, turn: &str, extra: Value) {
    h.out.clear();
    let mut hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                         "path": "talk", "needs": "", "families": ""});
    for (k, v) in obj(extra) {
        hop[k] = v;
    }
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "talk"});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("and then?")]}),
    );
}

/// One closed turn of the running session (its handle `live`): its curate and
/// the model's answer.
fn closed_turn(h: &mut Hive, turn: &str) {
    seam(h, turn, json!({"live": CALL}));
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "{:?} {:?}", h.out, h.stderr);
    h.out.clear();
    h.tap(&calls[0], "stop", json!({}), json!([said("fine")]));
}

/// A crisis named in a text session, then four quiet turns of the call's
/// session `s1` (each with its nameless `continue` mark), all behind the
/// clock.
fn crisis_then_call(h: &mut Hive) {
    clear_of_midnight();
    let at = backdated(55);
    h.row(
        at,
        "text-1",
        "t1",
        "user",
        &user("I do not want to go on"),
        1,
    );
    h.mark(
        at + chrono::Duration::seconds(1),
        "text-1",
        "t1",
        "topic",
        &json!({"movement": "start", "name": "crisis"}).to_string(),
    );
    for i in 1..=4i64 {
        let at = backdated(50 - 2 * i);
        let turn = format!("q{i}");
        h.row(at, "s1", &turn, "user", &user("what time is it"), 1);
        h.mark(
            at + chrono::Duration::seconds(1),
            "s1",
            &turn,
            "topic",
            &json!({"movement": "continue", "name": ""}).to_string(),
        );
    }
}

fn hive(aging: &str) -> Hive {
    Hive::with(&[
        ("policy", "live_feed", json!("1")),
        ("policy", "topic_aging", json!(aging)),
        ("policy", "topic_age_turns", json!(4)),
        ("history", "topic_aging", json!(aging)),
        ("history", "topic_age_turns", json!(4)),
    ])
    .reporting_composite("talky")
}

#[test]
fn an_aged_seed_is_not_fed_back_by_a_closed_turn() {
    if !shipped() {
        return;
    }
    let mut h = hive("1");
    crisis_then_call(&mut h);
    let seed = only(&seed_ask(&h, &aging_live(4), CALL), "seed");
    assert!(
        !fact_line(&seed).contains("topic:"),
        "the seed aged the topic out: {}",
        fact_line(&seed)
    );
    let held = h.state(&format!("seed_facts:{CALL}"));
    assert!(!held.is_empty(), "the seed keeps its facts");
    closed_turn(&mut h, "t5");
    assert!(
        h.routed("live_feed").is_empty(),
        "no feed brings the aged topic back: {:?}",
        h.routed("live_feed")
    );
    assert_eq!(
        h.state(&format!("seed_facts:{CALL}")),
        held,
        "the call's facts stay the seed's"
    );
}

#[test]
fn with_the_switch_off_the_feed_reads_as_before() {
    if !shipped() {
        return;
    }
    let mut h = hive("0");
    crisis_then_call(&mut h);
    let mut p = history_params(&[], &["now_utc"]);
    p.1.insert("topic_aging".into(), json!("0"));
    let seed = only(&seed_ask(&h, &p, CALL), "seed");
    assert!(
        fact_line(&seed).contains("topic: crisis"),
        "off, the topic carries as before: {}",
        fact_line(&seed)
    );
    let held = h.state(&format!("seed_facts:{CALL}"));
    closed_turn(&mut h, "t5");
    assert!(
        h.routed("live_feed").is_empty(),
        "seed and feed agree as before: {:?}",
        h.routed("live_feed")
    );
    assert_eq!(h.state(&format!("seed_facts:{CALL}")), held);
}
