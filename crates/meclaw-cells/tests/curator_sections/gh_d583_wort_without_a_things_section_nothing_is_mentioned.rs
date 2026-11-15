//! d58-3 wort (GH #1523) -- without a section `things`, nothing is mentioned.
//!
//! WHY this file exists: the push no longer guesses a name from the form of a
//! word (a capital, a list per language). What the model did not name as a
//! thing does not stand in `mentioned:` -- not a capital word, not a date,
//! not a number of the person's turn. A round whose answer brought no
//! section `things` asks with the topic and the person's words alone, as it
//! came; the rest of the question is unchanged.
//!
//! The hive runs in one process (`support/curator_hive.rs`), with the
//! sections a talky's curator registers as shipped. Guarded like every
//! template-reading test (GH #49).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn push_shipped() -> bool {
    repo("templates/curator/push/config.json").is_file()
        && repo("templates/talky/curator/config.json").is_file()
}

/// The sections a talky's curator registers, as shipped.
fn talky() -> Hive {
    let comp = read_json(&repo("templates/talky/curator/config.json"));
    let sections = comp["override_params"]["intake"]["sections"].clone();
    assert!(
        sections["things"].is_object(),
        "a talky registers `things`: {sections}"
    );
    Hive::with(&[("intake", "sections", sections)]).reporting_composite("talky")
}

fn ask(h: &mut Hive, session: &str, turn: &str, text: &str) -> Vec<Msg> {
    h.out.clear();
    h.lane(
        "in_recall_ask",
        json!({"session_id": session, "channel": "test", "audience_set": ROUND_E}),
        json!({"phase": "recall", "turn_id": turn, "session_id": session, "iter": "0",
               "recall_query": text, "memory_tier": "1",
               "recall_window_from": "", "recall_window_to": ""}),
        json!({"messages": [user(text)]}),
    );
    h.routed("recall")
}

/// One round without a section `things`: the ask, the person's words and the
/// model's answer; `memory`, if given, is the round's section `memory`.
fn round(h: &mut Hive, turn: &str, text: &str, reply: &str, memory: Option<Value>) {
    assert_eq!(ask(h, "s1", turn, text).len(), 1, "{:?}", h.stderr);
    let call = h.curate("s1", turn, 0, json!([user(text)]), mode(""));
    h.tap(&call, "stop", json!({}), json!([said(reply)]));
    if let Some(memory) = memory {
        h.section(&call, "memory", memory);
    }
}

#[test]
fn no_section_no_names() {
    if !push_shipped() {
        return;
    }
    let mut h = talky();
    round(
        &mut h,
        "t1",
        "Yesterday my daughter Anna came with me to the doctor.",
        "How nice that your daughter came along.",
        None,
    );
    round(
        &mut h,
        "t2",
        "The new appointment is on 12.03. at 9:30 with Mr Becker.",
        "Then I will note the appointment.",
        Some(json!({"topic": {"movement": "start", "name": "doctor visits"}, "facts": []})),
    );
    let asks = ask(&mut h, "s1", "t3", "What do you think?");
    assert_eq!(asks.len(), 1, "{:?}", h.stderr);
    let q = asks[0].hop["recall_query"]
        .as_str()
        .unwrap_or("")
        .to_string();
    // The positive anchor first: the push built this question from its reads
    // (the topic the round named stands in it) and built it whole. A question
    // sent as it came -- `recall_push` off, an ask not enriched, a broken
    // `compose` -- carries no `mentioned:` either, so without the anchor the
    // absence below would pass for the wrong reason.
    assert!(
        q.starts_with('[') && q.contains("topic: doctor visits"),
        "the push built the question with the round's topic: {q:?}"
    );
    assert!(
        asks[0].hop.get("recall_degraded").is_none(),
        "the question was built whole: {:?}",
        asks[0].hop
    );
    assert!(
        !q.contains("mentioned:"),
        "no section `things`, no `mentioned:` -- nothing guessed from the form of a word: {q:?}"
    );
    assert!(
        q.ends_with("] What do you think?"),
        "the person's words stay as they came, behind the push's part: {q:?}"
    );
}
