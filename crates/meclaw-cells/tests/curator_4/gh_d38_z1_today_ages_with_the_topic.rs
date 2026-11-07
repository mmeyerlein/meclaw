//! GH #1439 (D38 Z1, fix round 1, review I1) -- `today` ages with the topic.
//!
//! WHY this file exists: the state line read `today` as the newest named
//! `topic` mark of the round, unaged. A sensitive topic named in another
//! session, or turns ago, stood as `today: <topic>` right before the newest
//! words of the person on every turn -- the carry GH #1436 removed from the
//! route stamp, the judge, the seed and the live feed (R037 phone probe red,
//! ledger OR-38-34/OR-38-48). With `topic_aging` "1" the state line reads the
//! topic through the one aging function of `./policy` (`topic_aged`, the twin
//! of `./history`'s): no `today` in a session that did not name it, none after
//! `topic_age_turns` turns without a renewed mention; a mention keeps it. With
//! `topic_aging` "0" `today` stays as before.
//!
//! Runs the shipped policy in the test hive (`support/curator_hive.rs`); no
//! model, no colony.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

const TOPIC: &str = "the garden";

fn hive(aging: &str) -> Hive {
    Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "state_block", json!("1")),
        ("policy", "topic_aging", json!(aging)),
    ])
}

fn named(h: &mut Hive, session: &str, turn_id: &str) {
    h.mark(
        backdated(0),
        session,
        turn_id,
        "topic",
        &canonical(&json!({"movement": "start", "name": TOPIC})),
    );
}

/// The state line of the call (or "" when the window carries none).
fn state_line(c: &Msg) -> String {
    texts(c)
        .last()
        .cloned()
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with("- [state -- "))
        .unwrap_or("")
        .to_string()
}

/// The topic named in `s1`/`t1`, then `quiet` turns of `s1` that never name
/// it; the call of the next turn.
fn after_quiet(aging: &str, quiet: u32) -> String {
    let mut h = hive(aging);
    turn(
        &mut h,
        "s1",
        "t1",
        "We planted tomatoes.",
        "Lovely.",
        json!({}),
    );
    named(&mut h, "s1", "t1");
    for i in 2..=quiet + 1 {
        turn(
            &mut h,
            "s1",
            &format!("t{i}"),
            "What time is it?",
            "Noon.",
            json!({}),
        );
    }
    let next = format!("t{}", quiet + 2);
    let c = h.curate("s1", &next, 0, json!([user("And now?")]), mode("Be brief."));
    state_line(&c)
}

#[test]
fn quiet_turns_age_today() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    // Three quiet turns: under the bound of `topic_age_turns` (4), today holds.
    assert_eq!(
        after_quiet("1", 3),
        "- [state -- today: the garden]",
        "under the bound the topic still frames the turn"
    );
    // The fourth reaches it: no `today`, and nothing else to say -- no line.
    assert_eq!(after_quiet("1", 4), "", "the aged topic leaves the line");
    // Off: the line of before, the topic unaged.
    assert_eq!(after_quiet("0", 4), "- [state -- today: the garden]");
}

#[test]
fn a_renewed_mention_keeps_today() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut h = hive("1");
    turn(
        &mut h,
        "s1",
        "t1",
        "We planted tomatoes.",
        "Lovely.",
        json!({}),
    );
    named(&mut h, "s1", "t1");
    for i in 2..=4 {
        turn(
            &mut h,
            "s1",
            &format!("t{i}"),
            "What time is it?",
            "Noon.",
            json!({}),
        );
    }
    turn(&mut h, "s1", "t5", "The tomatoes again.", "Yes.", json!({}));
    named(&mut h, "s1", "t5");
    for i in 6..=7 {
        turn(
            &mut h,
            "s1",
            &format!("t{i}"),
            "What time is it?",
            "Noon.",
            json!({}),
        );
    }
    let c = h.curate("s1", "t8", 0, json!([user("And now?")]), mode("Be brief."));
    assert_eq!(
        state_line(&c),
        "- [state -- today: the garden]",
        "two quiet turns since the renewed mention: the topic holds"
    );
}

/// The session rule at the function the window calls: `state_of_rows` hands
/// `today` to `topic_aged` with the asking session. A first call of a new
/// session carries the handover and leaves `today` out anyway (GH #896); the
/// leaf falls with the session's first rebuild, and from then on only the
/// aging keeps the other session's topic out.
#[test]
fn another_session_carries_no_today() {
    if !shipped() {
        return;
    }
    let args = json!({
        "round": ROUND_E,
        "marks": [{"seq": 100, "session_id": "s1", "turn_id": "t1", "kind": "topic",
                   "value": canonical(&json!({"movement": "start", "name": TOPIC})),
                   "at": "", "audience_set": ROUND_E}],
        "wall": [{"seq": 200, "session_id": "s2", "turn_id": "n1", "audience_set": ROUND_E}]
    });
    let probe = "[state_of_rows(ARGS['marks'], [], {}, {}, ARGS['round'], s, 0, '', \
                   (ARGS['wall'], 0, False))['today'] for s in ('s2', 's1')] \
                 + [state_of_rows(ARGS['marks'], [], {}, {}, ARGS['round'], 's2', 0, '')['today']]";
    let (on, err) = policy_scope(
        json!({"role": "talky", "topic_aging": "1"}),
        probe,
        args.clone(),
    );
    assert_eq!(
        on,
        json!(["", TOPIC, TOPIC]),
        "another session: no today; its own session one turn on: today; no age: as read: {err}"
    );
    let (off, err) = policy_scope(json!({"role": "talky", "topic_aging": "0"}), probe, args);
    assert_eq!(off, json!([TOPIC, TOPIC, TOPIC]), "off: as before: {err}");
}
