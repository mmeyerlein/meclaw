//! GH #1436 (a) -- a topic ages: it does not carry into a new session it was
//! not named in, and it stops framing turns after `topic_age_turns` turns
//! without a renewed mention.
//!
//! WHY this file exists: `./history` read the newest named `topic` mark of the
//! round, and a quiet turn's nameless `continue` left it standing. A sensitive
//! topic raised in a text turn was still the current topic five turns later in
//! a new voice session: the route stamped a harmless question as an indirect
//! reference to it and the curator interrupted with a safety question. With
//! `topic_aging` "1" the read that hands the topic to the route stamp and the
//! judge (and the seed of a call) ages it.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/history_judge.rs"]
#[allow(unused_imports)]
use crate::history_judge; // harness_fold: mod history_judge;

use curator_hive::*;
use history_judge::*;

/// A crisis raised in a text session, then `quiet` turns of a voice session
/// that never name it.
fn crisis_then_voice(quiet: u32) -> Hive {
    let mut h = Hive::new();
    said_turn(
        &mut h,
        1,
        "text-1",
        "t1",
        "I do not want to go on",
        "crisis",
    );
    for i in 1..=quiet {
        said_turn(
            &mut h,
            10 + i,
            "voice-1",
            &format!("v{i}"),
            "what time is it",
            "",
        );
    }
    h
}

#[test]
fn a_sensitive_topic_does_not_carry_into_a_new_voice_session() {
    let p = aging(4);
    // Over the bound of quiet turns, and in another session: no topic.
    let h = crisis_then_voice(4);
    let a = only(&delta_judge_ask(&h, &p, "call-1", "v5"), "turns");
    assert_eq!(
        judged_topic(&a),
        "",
        "the route stamp and the judge see no topic"
    );
    let seed = only(&seed_ask(&h, &p, "call-1"), "seed");
    let facts = fact_line(&seed);
    assert!(
        !facts.contains("topic:"),
        "the seed names no topic: {facts}"
    );
    assert_eq!(seed.body["judge"]["topic"], "");
}

#[test]
fn the_new_session_alone_ends_the_frame() {
    // One quiet turn of the voice session, far under the bound: the topic
    // was not named in this session, so it does not carry into it.
    let h = crisis_then_voice(1);
    let a = only(&delta_judge_ask(&h, &aging(4), "call-1", "v2"), "turns");
    assert_eq!(judged_topic(&a), "");
}

#[test]
fn quiet_turns_end_the_frame_in_its_own_session() {
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
    for i in 2..=4 {
        said_turn(&mut h, i, "text-1", &format!("t{i}"), "what time is it", "");
    }
    // Three quiet turns: under the bound, the topic still frames.
    let a = only(&judge_ask(&h, &p, "text-1", "t5"), "turns");
    assert_eq!(judged_topic(&a), "crisis");
    // The fourth: the bound reached, no topic.
    said_turn(&mut h, 5, "text-1", "t5", "and the weather", "");
    let a = only(&judge_ask(&h, &p, "text-1", "t6"), "turns");
    assert_eq!(judged_topic(&a), "");
    // The bound is the param's: with 5 the same wall still frames.
    let a = only(&judge_ask(&h, &aging(5), "text-1", "t6"), "turns");
    assert_eq!(judged_topic(&a), "crisis");
}

/// Fix round 1 (review C1): the session rule takes the session that ASKS, not
/// the session of the newest mark. A call opens with its seed (`reason`
/// `start`) before any of its turns stands on the wall, and its first delta
/// is stamped before the turn's own words are written -- a new session holds
/// no mark yet, and neither may carry the topic of the text session in.
#[test]
fn a_call_with_no_voice_turn_yet_carries_no_unnamed_topic() {
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
    // The call opens: zero voice turns on the wall.
    let seed = only(&seed_ask(&h, &p, "call-1"), "seed");
    let facts = fact_line(&seed);
    assert!(
        !facts.contains("topic:"),
        "the opening seed names no topic: {facts}"
    );
    assert_eq!(seed.body["judge"]["topic"], "");
    // The first voice turn's delta, its words not yet on the wall.
    let a = only(&delta_judge_ask(&h, &p, "call-1", "v1"), "turns");
    assert_eq!(
        judged_topic(&a),
        "",
        "the first stamp of the call sees no topic"
    );
}

/// Review C1: a turn of the new session asks with its own session in the
/// context -- the topic of another session does not frame it, though no mark
/// of the new session stands yet.
#[test]
fn the_asking_session_decides_not_the_newest_mark() {
    let mut h = Hive::new();
    said_turn(
        &mut h,
        1,
        "text-1",
        "t1",
        "I do not want to go on",
        "crisis",
    );
    markless_turn(&mut h, 11, "voice-1", "v1", "what time is it");
    let a = only(&judge_ask(&h, &aging(4), "voice-1", "v2"), "turns");
    assert_eq!(judged_topic(&a), "");
}

/// Review I1: the bound counts TURNS (the person's rows on the wall), not
/// `topic` marks -- a tool round, an error answer or an answer without its
/// `memory` block leaves no mark and is a turn all the same.
#[test]
fn turns_without_a_mark_age_the_topic() {
    let mut h = Hive::new();
    said_turn(
        &mut h,
        1,
        "text-1",
        "t1",
        "I do not want to go on",
        "crisis",
    );
    for i in 2..=4 {
        markless_turn(&mut h, i, "text-1", &format!("t{i}"), "look it up");
    }
    // Three markless turns: under the bound.
    let a = only(&judge_ask(&h, &aging(4), "text-1", "t5"), "turns");
    assert_eq!(judged_topic(&a), "crisis");
    // The fourth reaches it.
    markless_turn(&mut h, 5, "text-1", "t5", "and again");
    let a = only(&judge_ask(&h, &aging(4), "text-1", "t6"), "turns");
    assert_eq!(judged_topic(&a), "");
}
