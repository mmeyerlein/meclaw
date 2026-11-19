//! GH #1567 -- a spoken turn without the person's words reaches the wall,
//! and the seed of a later call reads a closed session's open end as ended.
//!
//! WHY this file exists: a duplex voice turn with an empty `user` half (the
//! model speaking a delegation's answer on its own, a goodbye) was held by the
//! collector for the next round of its session; the caller hung up first, so
//! the line never reached the curator. A delegation answered for the voice
//! carried its words only in the `fact` section of its block, and every reader
//! of the wall reads an answer without its block -- so nothing of it showed.
//! The next call's seed ended with the old question and read it as open.
//! Now the collector sends such a line at once (`line_only`), the curator
//! files it without a model call, a fact-only answer is filed with the fact's
//! words, and a closed session's open end is said to be over in the seed.
//!
//! The hive in one process (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_live.rs"]
#[allow(unused_imports)]
use crate::curator_live; // harness_fold: mod curator_live;

use curator_hive::*;
use curator_live::*;
use meclaw_core::serde_json::{Value, json};

const LINE: &str = "Your appointment is confirmed for nine tomorrow. Goodbye!";
const FACT: &str = "The pharmacy on Main Street is open until eight tonight.";
const ENDED: &str = "[the conversation above ended there;";

fn text(m: &Value) -> &str {
    m["text"].as_str().unwrap_or("")
}

#[test]
fn gh1567_a_line_without_the_persons_words_is_filed_without_a_model_call() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.name_main_target();
    h.out.clear();
    h.lane(
        "in_curate",
        json!({"session_id": "s1", "turn_id": "s1#0", "iter": "0", "channel": "test",
               "audience_set": ROUND_E}),
        json!({"session_id": "s1", "turn_id": "s1#0", "iter": "0", "phase": "",
               "line_only": "1"}),
        json!({"messages": [said(LINE)]}),
    );
    assert!(
        h.routed("brain").is_empty(),
        "no model is called for the agent's own line: {:?}",
        h.out
    );
    assert_eq!(
        h.rows("SELECT kind, final FROM wall WHERE turn_id = 's1#0'"),
        vec![vec![json!("assistant"), json!(1)]],
        "the line is the said answer of its turn: {:?}",
        h.stderr
    );
    assert_eq!(
        h.rows("SELECT COUNT(*) FROM state WHERE key LIKE 'pending:%' AND key NOT LIKE '%:e'"),
        vec![vec![json!(0)]],
        "nothing stays parked: {:?}",
        h.stderr
    );
    let next = h.curate("s1", "s1#1", 0, json!([user("Thanks.")]), mode("Be brief."));
    let msgs = next.messages();
    let at = |needle: &str| msgs.iter().position(|m| text(m).contains(needle));
    let line = at(LINE).unwrap_or_else(|| panic!("the line stands in the next window: {msgs:#?}"));
    assert_eq!(msgs[line]["origin"], "assistant", "{msgs:#?}");
    assert!(
        line < at("Thanks.").expect("the person's words"),
        "{msgs:#?}"
    );
}

#[test]
fn gh1567_a_fact_only_answer_is_filed_with_the_facts_words() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.name_main_target();
    let only_fact = format!("```sidecar\n{{\"fact\": \"{FACT}\"}}\n```");
    turn(
        &mut h,
        "s1",
        "t1",
        "Is the pharmacy open?",
        &only_fact,
        json!({}),
    );
    let next = h.curate(
        "s1",
        "t2",
        0,
        json!([user("Thank you.")]),
        mode("Be brief."),
    );
    let msgs = next.messages();
    let answer = msgs
        .iter()
        .find(|m| m["origin"] == "assistant" && text(m).contains(FACT))
        .unwrap_or_else(|| panic!("the fact stands as the answer of its turn: {msgs:#?}"));
    assert!(
        text(answer).starts_with(FACT),
        "the fact's words open the answer: {:?}",
        text(answer)
    );
}

/// A line sent at once (`line_only`) through `in_curate` and the next
/// window of its session, as the wall gives it back.
fn line_then_next(line: &str) -> Vec<Value> {
    let mut h = Hive::new();
    h.name_main_target();
    h.out.clear();
    h.lane(
        "in_curate",
        json!({"session_id": "s1", "turn_id": "s1#0", "iter": "0", "channel": "test",
               "audience_set": ROUND_E}),
        json!({"session_id": "s1", "turn_id": "s1#0", "iter": "0", "phase": "",
               "line_only": "1"}),
        json!({"messages": [said(line)]}),
    );
    assert!(h.routed("brain").is_empty(), "no model call: {:?}", h.out);
    let next = h.curate("s1", "s1#1", 0, json!([user("Thanks.")]), mode("Be brief."));
    next.messages()
}

/// GH #1566 on the `in_curate` way: a line sent at once with a copied frame
/// of the curator in front of it is filed without the frame.
#[test]
fn gh1566_a_line_sent_at_once_is_filed_without_a_copied_frame() {
    if !shipped() {
        return;
    }
    let copied = format!(
        "[your answer right after this: your own, in another session, since 2026-11-17 18:15 UTC]\n{LINE}"
    );
    let msgs = line_then_next(&copied);
    let line = msgs
        .iter()
        .find(|m| m["origin"] == "assistant" && text(m).contains(LINE))
        .unwrap_or_else(|| panic!("the line stands in the next window: {msgs:#?}"));
    assert!(
        text(line).starts_with(LINE),
        "the line is filed without the copied frame: {:?}",
        text(line)
    );
    assert!(
        !msgs.iter().any(
            |m| m["origin"] == "assistant" && text(m).contains("[your answer right after this")
        ),
        "no copied frame as the agent's words in the next window: {msgs:#?}"
    );
}

/// GH #1567 on the `in_curate` way: a line sent at once that is only a block
/// with a `fact` section is filed with the fact's words.
#[test]
fn gh1567_a_fact_only_line_sent_at_once_is_filed_with_the_facts_words() {
    if !shipped() {
        return;
    }
    let only_fact = format!("```sidecar\n{{\"fact\": \"{FACT}\"}}\n```");
    let msgs = line_then_next(&only_fact);
    let answer = msgs
        .iter()
        .find(|m| m["origin"] == "assistant" && text(m).contains(FACT))
        .unwrap_or_else(|| panic!("the fact stands as the line of its turn: {msgs:#?}"));
    assert!(
        text(answer).starts_with(FACT),
        "the fact's words open the line: {:?}",
        text(answer)
    );
}

fn old_question_then_seed(close: bool) -> Vec<Value> {
    let mut h = live_hive(&[]);
    // A turn of an earlier session whose answer never reached the wall.
    seam(&mut h, "s-old", "s-old#0", "talk", &json!({}), json!({}));
    if close {
        h.lane(
            "in_close",
            json!({"session_id": "s-old", "audience_set": ROUND_E}),
            json!({"session_id": "s-old"}),
            json!({"messages": []}),
        );
        assert_eq!(
            h.rows(
                "SELECT COUNT(*) FROM marks WHERE kind = 'session_closed' AND session_id = 's-old'"
            ),
            vec![vec![json!(1)]],
            "the close is a mark: {:?}",
            h.stderr
        );
    }
    let seeds = ask_seed(&mut h, &live_keys(LIVE), "start");
    assert_eq!(seeds.len(), 1, "{:?} {:?}", h.out, h.stderr);
    seeds[0].messages()
}

#[test]
fn gh1567_the_seed_reads_the_open_end_of_a_closed_session_as_ended() {
    if !shipped() {
        return;
    }
    let msgs = old_question_then_seed(true);
    let asked = msgs
        .iter()
        .position(|m| m["role"] == "user" && text(m).contains("and then?"))
        .unwrap_or_else(|| panic!("the old question stands in the seed: {msgs:#?}"));
    let after = msgs.get(asked + 1).unwrap_or_else(|| panic!("{msgs:#?}"));
    assert!(
        after["role"] == "developer" && text(after).starts_with(ENDED),
        "right after the old question the seed says the session ended: {msgs:#?}"
    );
}

#[test]
fn gh1567_a_session_not_closed_keeps_its_seed_as_it_was() {
    if !shipped() {
        return;
    }
    let msgs = old_question_then_seed(false);
    assert!(
        msgs.iter().any(|m| text(m).contains("and then?")),
        "{msgs:#?}"
    );
    assert!(
        !msgs.iter().any(|m| text(m).starts_with(ENDED)),
        "no end line without a close: {msgs:#?}"
    );
}
