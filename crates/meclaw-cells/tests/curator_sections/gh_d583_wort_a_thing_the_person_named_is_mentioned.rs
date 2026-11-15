//! d58-3 wort (GH #1523) -- the names in `mentioned:` come from the model.
//!
//! WHY this file exists: `curator/push` built `mentioned:` from word lists per
//! language (`entity_stop_lang`, `entity_det_lang`, `entity_suffix_lang`,
//! `entity_date_lang`): a capital that opens no sentence and is on no list was
//! taken for a name. A list never knows a language; the owner's rule is
//! "meaning by the model, code only for structure". The model already names
//! the things of the person's world in the section `things` of every answer
//! (talky and cogny register it). Now `./intake` hands those names to the push
//! the way it hands the topic (a ledger mark the push reads in the bundle of
//! its round), and the push asks with a name the model named AND the person
//! said in the newest rounds. A name at the start of a sentence -- which the
//! lists never took -- is a name all the same.
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

/// One round: the ask, the person's words, the model's answer and the
/// section `things` of that answer.
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
fn the_model_names_it_and_the_person_said_it() {
    if !push_shipped() {
        return;
    }
    let mut h = talky();
    round(
        &mut h,
        "t1",
        "Lotte is coming over tomorrow. My walker squeaks again.",
        "How nice that Lotte is coming.",
        json!([{"name": "Lotte", "kind": "person"},
               {"name": "walker", "kind": "device"}]),
    );
    let asks = ask(&mut h, "s1", "t2", "What do you think?");
    assert_eq!(asks.len(), 1, "{:?}", h.stderr);
    let q = asks[0].hop["recall_query"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let got = mentioned(&q);
    for name in ["Lotte", "walker"] {
        assert!(
            got.iter().any(|g| g == name),
            "{name} the model named and the person said is mentioned: {got:?} in {q:?}"
        );
    }
    assert!(
        q.ends_with("] What do you think?"),
        "the person's words come last, unchanged: {q:?}"
    );
}

/// The intake books a thing's name in NFC; the person may write the same
/// name decomposed (a base letter and a combining accent). The push matches
/// both in one normal form, so the name the person said is mentioned however
/// the keyboard spelled it.
#[test]
fn a_name_the_person_writes_decomposed_is_mentioned() {
    if !push_shipped() {
        return;
    }
    let mut h = talky();
    round(
        &mut h,
        "t1",
        "Jose\u{301} called me this morning.",
        "How kind of him.",
        json!([{"name": "Jos\u{e9}", "kind": "person"}]),
    );
    let asks = ask(&mut h, "s1", "t2", "What do you think?");
    assert_eq!(asks.len(), 1, "{:?}", h.stderr);
    let q = asks[0].hop["recall_query"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let got = mentioned(&q);
    assert!(
        got.iter().any(|g| g == "Jos\u{e9}"),
        "the name the person wrote decomposed is mentioned: {got:?} in {q:?}"
    );
}
