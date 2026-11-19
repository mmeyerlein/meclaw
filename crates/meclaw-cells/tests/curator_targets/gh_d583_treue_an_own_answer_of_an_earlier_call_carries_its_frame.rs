//! d58-3 treue (T1) -- an own answer of an earlier call carries its frame.
//!
//! WHY this file exists: the session boundary (GH #1335) framed only the
//! person's words of an earlier session; the model's own answers of it stood
//! bare in the window. In measurement M3 (P2, the weather, n = 6) the model
//! read the weather answer of the last call as said in this one and left the
//! fact out of its answer (11 of 12 `content_wrong`). An own answer of
//! another session now has its own frame -- said in an earlier session, with
//! the session's start -- and keeps its role and its place. GH #1566: the
//! frame stands as the last line of the element right before the answer,
//! never in the model's own text (a frame there was copied into new answers).
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const ASK: &str = "How is the weather in Berlin today?";
const WEATHER: &str = "In Berlin it is 12 degrees and sunny today.";
const AGAIN: &str = "And how is the weather?";

fn text(m: &Value) -> &str {
    m["text"].as_str().unwrap_or("")
}

#[test]
fn gh_d583_treue_an_own_answer_of_an_earlier_call_carries_its_frame() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut h = Hive::new();
    let base = backdated(50);
    let at = |ms: i64| base + chrono::Duration::milliseconds(ms);
    h.row(at(0), "s1", "t1", "user", &user(ASK), 0);
    h.row(at(10), "s1", "t1", "assistant", &said(WEATHER), 1);
    let call = h.curate("s2", "t1", 0, json!([user(AGAIN)]), mode("Be brief."));
    let msgs = call.messages();
    let since = base.format("%Y-%m-%d %H:%M UTC").to_string();
    let at_answer = msgs
        .iter()
        .position(|m| text(m).ends_with(WEATHER))
        .unwrap_or_else(|| panic!("the weather answer of the last call stays: {msgs:#?}"));
    let answer = &msgs[at_answer];
    assert_eq!(
        (answer["origin"].as_str(), answer["type"].as_str()),
        (Some("assistant"), Some("text")),
        "the answer keeps its role: {answer:#?}"
    );
    assert_eq!(
        text(answer),
        WEATHER,
        "the answer keeps its own words, no frame in them"
    );
    let frame = format!(
        "\n[your answer right after this: your own, in an earlier session, since {since} -- said then, not now]"
    );
    let before = &msgs[at_answer - 1];
    assert!(
        before["origin"] != "assistant" && text(before).ends_with(&frame),
        "the element before the answer of the earlier call carries its frame: {before:#?}"
    );
    // The person's words of that call keep their boundary; the turn stays last.
    let ask = msgs
        .iter()
        .find(|m| text(m).contains(ASK))
        .unwrap_or_else(|| panic!("the earlier question stays: {msgs:#?}"));
    assert!(
        text(ask).starts_with("[earlier session, since "),
        "the earlier question keeps its boundary: {:?}",
        text(ask)
    );
    let last = msgs.last().expect("a window");
    assert!(
        last["origin"] == "user" && text(last).ends_with(AGAIN),
        "the person's turn stays last: {msgs:#?}"
    );
}
