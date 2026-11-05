//! GH #1423 (M7-C) -- the rails of a counter (M7 section 4.2).
//!
//! `fact_id` must be a record the round was shown, `episode_id` a turn of the
//! COMPANION in the same round. A record outside the window, a turn of the
//! member or of a peer (the source boundary: a person's fact is never answered
//! out of their own or another person's mouth), a turn of no round, a reply to
//! the agent's own record -- each is dropped alone, counted under
//! `unseen_refs`, and the rest of the verdict goes on. Bare script runs, no
//! colony, no model.

// harness_fold: #[path = "support/memory_hive_subject.rs"]
#[allow(unused_imports)]
use crate::support; // harness_fold: mod support;

use meclaw_core::serde_json::{Value, json};
use support::*;

fn counter(fact_id: &str, episode_id: &str) -> Value {
    json!({"fact_id": fact_id, "episode_id": episode_id,
           "claim": "worth showing to a doctor", "why": ""})
}

fn assert_dropped(turns: &[Value], facts: &[Value], item: Value) {
    let out = m7c_close(turns, facts, json!({"counter": [item.clone()]}));
    assert!(m7c_blocks(&out).is_empty(), "{item}: {out:?}");
    let report = m7c_report(&out);
    assert_eq!(report["countered"], 0, "{item}: {report}");
    assert_eq!(report["unseen_refs"], 1, "{item}: {report}");
}

#[test]
fn a_record_the_round_was_not_shown_is_answered_by_nobody() {
    if !shipped() {
        return;
    }
    let mut elsewhere = m7c_fact("f-other-room", M7C_SAID, "benign", "");
    elsewhere["audience_set"] = json!(r#"["agent:b","member:e"]"#);
    let facts = [m7c_fact("f-said", M7C_SAID, "harmless", ""), elsewhere];
    // Invented, and of another round.
    assert_dropped(&m7c_turns(), &facts, counter("f-invented", M7C_REPLY));
    assert_dropped(&m7c_turns(), &facts, counter("f-other-room", M7C_REPLY));
}

#[test]
fn only_a_turn_of_the_companion_in_this_round_answers() {
    if !shipped() {
        return;
    }
    let facts = [m7c_fact("f-said", M7C_SAID, "harmless", "")];
    let mut turns = m7c_turns();
    turns.push(m7c_turn(
        "ep-again",
        "user",
        "Well, I think it is fine.",
        "2026-11-05T10:00:09Z",
    ));
    let mut peer = m7c_turn(
        "ep-peer",
        "peer",
        "I would not worry.",
        "2026-11-05T10:00:10Z",
    );
    peer["speaker"] = json!("a1b2");
    turns.push(peer);
    // The member's own turns, a peer's turn, a turn of no round.
    assert_dropped(&turns, &facts, counter("f-said", M7C_SAID));
    assert_dropped(&turns, &facts, counter("f-said", "ep-again"));
    assert_dropped(&turns, &facts, counter("f-said", "ep-peer"));
    assert_dropped(&turns, &facts, counter("f-said", "ep-nowhere"));
}

#[test]
fn the_agent_does_not_answer_itself() {
    if !shipped() {
        return;
    }
    let facts = [m7c_fact(
        "f-own",
        M7C_REPLY,
        "will remind about the mole",
        "agent:self",
    )];
    assert_dropped(&m7c_turns(), &facts, counter("f-own", M7C_REPLY));
}

#[test]
fn a_dropped_counter_leaves_the_rest_of_the_verdict_standing() {
    if !shipped() {
        return;
    }
    let out = m7c_close(
        &m7c_turns(),
        &[m7c_fact("f-said", M7C_SAID, "harmless", "")],
        json!({"counter": [counter("f-invented", M7C_REPLY), counter("f-said", M7C_REPLY)]}),
    );
    assert_eq!(m7c_blocks(&out).len(), 1, "{out:?}");
    let report = m7c_report(&out);
    assert_eq!(report["countered"], 1, "{report}");
    assert_eq!(report["unseen_refs"], 1, "{report}");
}
