//! GH #1436 (b) -- a renewed mention keeps a topic: named again in the new
//! session, or named again before `topic_age_turns` quiet turns passed, it
//! frames the turns as before.
//!
//! WHY this file exists: the aging of `topic_aging` "1" must take only a
//! topic nobody speaks of any more out of the frame -- a topic the person
//! keeps returning to stays the route stamp's and the judge's topic.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/history_judge.rs"]
#[allow(unused_imports)]
use crate::history_judge; // harness_fold: mod history_judge;

use curator_hive::*;
use history_judge::*;
use meclaw_core::serde_json::json;

/// The order of a real call (fix round 1, review C1): the call opens with its
/// seed (`start`) before its turns stand on the wall; a turn of the call names
/// the topic again; the next delta and the seed of a renewal carry it.
#[test]
fn a_topic_named_again_in_the_new_session_carries() {
    let p = aging(4);
    let mut h = Hive::new();
    said_turn(
        &mut h,
        1,
        "text-1",
        "t1",
        "I do not want to go on",
        "crisis",
    );
    // The call opens at minute 5 -- after the text turn, before its own.
    let mut at_open = aging(4);
    at_open
        .1
        .insert("now_utc".into(), json!("2026-10-24T09:05:00Z"));
    let opening = only(&seed_ask(&h, &at_open, "call-1"), "seed");
    assert!(!fact_line(&opening).contains("topic:"));
    said_turn(&mut h, 11, "voice-1", "v1", "hello again", "");
    h.row(at(12), "voice-1", "v2", "user", &user("about yesterday"), 1);
    h.mark(
        at(12),
        "voice-1",
        "v2",
        "topic",
        &topic("crisis", "continue"),
    );
    said_turn(&mut h, 13, "voice-1", "v3", "yes", "");
    let a = only(&delta_judge_ask(&h, &p, "call-1", "v4"), "turns");
    assert_eq!(judged_topic(&a), "crisis");
    let a = only(&judge_ask(&h, &p, "voice-1", "v4"), "turns");
    assert_eq!(judged_topic(&a), "crisis");
    let seed = only(&seed_ask_for(&h, &p, "call-1", "renew", 1), "seed");
    let facts = fact_line(&seed);
    assert!(facts.contains("topic: crisis"), "{facts}");
}

#[test]
fn a_renewed_mention_starts_the_count_again() {
    let p = aging(4);
    let mut h = Hive::new();
    said_turn(
        &mut h,
        1,
        "text-1",
        "t1",
        "I do not want to go on",
        "crisis",
    );
    for i in 2..=5 {
        said_turn(&mut h, i, "text-1", &format!("t{i}"), "what time is it", "");
    }
    h.row(at(6), "text-1", "t6", "user", &user("it is back"), 1);
    h.mark(at(6), "text-1", "t6", "topic", &topic("crisis", "continue"));
    for i in 7..=9 {
        said_turn(&mut h, i, "text-1", &format!("t{i}"), "and now", "");
    }
    let a = only(&judge_ask(&h, &p, "text-1", "t10"), "turns");
    assert_eq!(judged_topic(&a), "crisis");
}
