//! d58-3 wort (GH #1523) -- a name only the model said is not mentioned.
//!
//! WHY this file exists: the section `things` is the model's, and the model
//! names what it said too. GH #1342 holds for the names that now come from it:
//! the anchors of a question are the counterpart's words of the running
//! session, never the model's own answers -- a name the model brought up and
//! the person never said (in a canary run: a crisis line the model had
//! offered) does not ride into the next recall. So the push takes a name of
//! the section only where it stands, as whole words, in the person's words of
//! the newest `entity_rounds` rounds: a comparison of strings, no meaning.
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

fn round(h: &mut Hive, turn: &str, text: &str, reply: &str, things: Value) {
    assert_eq!(ask(h, "s1", turn, text).len(), 1, "{:?}", h.stderr);
    let call = h.curate("s1", turn, 0, json!([user(text)]), mode(""));
    h.tap(&call, "stop", json!({}), json!([said(reply)]));
    h.section(&call, "things", json!({"items": things}));
}

fn mentioned(q: &str) -> Vec<String> {
    q.split("mentioned: ")
        .nth(1)
        .and_then(|r| r.split(';').next())
        .and_then(|r| r.split("] ").next())
        .map(|r| r.split(", ").map(str::to_string).collect())
        .unwrap_or_default()
}

#[test]
fn the_models_own_name_stays_out() {
    if !push_shipped() {
        return;
    }
    let mut h = talky();
    round(
        &mut h,
        "t1",
        "Lotte stayed with me for a long time yesterday.",
        "Has Doctor Brandt seen Lotte yet? The helpline answers at night too.",
        json!([{"name": "Lotte", "kind": "person"},
               {"name": "Doctor Brandt", "kind": "person"},
               {"name": "helpline", "kind": "service"}]),
    );
    let asks = ask(&mut h, "s1", "t2", "What do you think?");
    assert_eq!(asks.len(), 1, "{:?}", h.stderr);
    let q = asks[0].hop["recall_query"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let got = mentioned(&q);
    // The positive anchor first: without it, "nothing mentioned" would pass
    // the absence checks below for the wrong reason.
    assert!(
        got.iter().any(|g| g == "Lotte"),
        "Lotte the person said is mentioned: {got:?} in {q:?}"
    );
    for only_the_model in ["Doctor Brandt", "Brandt", "Doctor", "helpline"] {
        assert!(
            !got.iter().any(|g| g == only_the_model),
            "{only_the_model} only the model said is not mentioned: {got:?} in {q:?}"
        );
    }
}
