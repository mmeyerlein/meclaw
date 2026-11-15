//! d58-3 treue (T1, evidence A4) -- an earlier answer is not taken back as
//! unproven.
//!
//! WHY this file exists: in run A4 of the d54g battery the model saw an
//! earlier session in its window without that session's tool results, read
//! its own earlier statements there as claims of this turn nothing backed,
//! called them "unproven" and took right answers back. The window now says
//! whose words they are and when they were said: the own answer of the
//! earlier session opens with the frame of an own answer said in an earlier
//! session, not as a bare statement of the running turn.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const ASK: &str = "Is the pharmacy on Main Street open on Sunday?";
const ANSWER: &str = "Yes, the pharmacy on Main Street is open on Sunday from 10 to 14.";
const NOW: &str = "You told me about the pharmacy. Is that right?";

fn text(m: &Value) -> &str {
    m["text"].as_str().unwrap_or("")
}

#[test]
fn gh_d583_treue_an_earlier_answer_is_not_taken_back_as_unproven() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut h = Hive::new();
    let base = backdated(50);
    let at = |ms: i64| base + chrono::Duration::milliseconds(ms);
    // The earlier session as the window gets it: the question and the own
    // answer, the tool results that backed it not among its rows.
    h.row(at(0), "s1", "t1", "user", &user(ASK), 0);
    h.row(at(10), "s1", "t1", "assistant", &said(ANSWER), 1);
    let call = h.curate("s2", "t1", 0, json!([user(NOW)]), mode("Be brief."));
    let msgs = call.messages();
    assert!(
        !msgs.iter().any(|m| m["type"] == "tool_result"),
        "check of the setup: no tool result of the earlier session in the window: {msgs:#?}"
    );
    let since = base.format("%Y-%m-%d %H:%M UTC").to_string();
    let answer = msgs
        .iter()
        .find(|m| text(m).ends_with(ANSWER))
        .unwrap_or_else(|| panic!("the own earlier answer stays: {msgs:#?}"));
    assert_eq!(
        answer["origin"], "assistant",
        "the earlier answer stays the model's own: {answer:#?}"
    );
    assert!(
        text(answer).starts_with(&format!(
            "[your own answer in an earlier session, since {since} -- said then, not now]\n"
        )),
        "the earlier answer stands as said in an earlier session: {:?}",
        text(answer)
    );
}
