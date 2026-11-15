//! GH #1089 on the curator (GH #1225 B4, GH #1523) -- the push asks with
//! names, not nouns.
//!
//! WHY this file exists: `curator/push` took every capital that opens no
//! sentence for a name -- an English rule. In German every noun carries one:
//! `mentioned:` filled its eight places with Tochter, Arzt, Termin and the
//! polite "Ihrer", and the name the memory needed fell off the end. GH #1225
//! answered with word lists per language; GH #1523 took them out (the owner's
//! rule: meaning by the model, code only for structure). Now a name is one the
//! model named in its section `things` (mark `thing`) and the person said; a
//! noun nobody named stays out, whatever its form, and so does a topic's name
//! the model did not name as a thing. Nothing guesses a name.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn push_shipped() -> bool {
    repo("templates/curator/push/config.json").is_file()
}

fn talky() -> Hive {
    // GH #1473: a talky's curator registers its `memory` section, and
    // (GH #1523) its `things` with the mark `thing`.
    Hive::with(&[(
        "intake",
        "sections",
        json!({"memory": {"lane": "sidecar", "mark": "topic"},
               "things": {"lane": "thing_seen", "cap_share": 0.1, "mark": "thing"}}),
    )])
    .reporting_composite("talky")
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

fn round(h: &mut Hive, session: &str, turn: &str, text: &str, reply: &str) {
    assert_eq!(ask(h, session, turn, text).len(), 1, "{:?}", h.stderr);
    let call = h.curate(session, turn, 0, json!([user(text)]), mode(""));
    h.tap(&call, "stop", json!({}), json!([said(reply)]));
}

fn topic(h: &mut Hive, session: &str, turn: &str, name: &str) {
    h.lane(
        "in_section",
        json!({"session_id": session, "turn_id": turn, "iter": "0", "curator_call": "c-1",
               "channel": "test", "audience_set": ROUND_E}),
        json!({"section": "memory"}),
        json!({"messages": [], "section": "memory",
               "payload": {"topic": {"movement": "start", "name": name}}}),
    );
}

/// The model's section `things` of a turn: the names it saw (GH #1523).
fn things(h: &mut Hive, session: &str, turn: &str, names: &[&str]) {
    let items: Vec<Value> = names
        .iter()
        .map(|n| json!({"name": n, "kind": "person"}))
        .collect();
    h.lane(
        "in_section",
        json!({"session_id": session, "turn_id": turn, "iter": "0", "curator_call": "c-1",
               "channel": "test", "audience_set": ROUND_E}),
        json!({"section": "things"}),
        json!({"messages": [], "section": "things", "payload": {"items": items}}),
    );
}

fn mentioned(q: &str) -> Vec<String> {
    q.split("mentioned: ")
        .nth(1)
        .and_then(|r| r.split(';').next())
        .and_then(|r| r.split("] ").next())
        .map(|r| r.split(", ").map(str::to_string).collect())
        .unwrap_or_default()
}

fn query(h: &mut Hive, turn: &str, text: &str) -> String {
    let asks = ask(h, "s1", turn, text);
    assert_eq!(asks.len(), 1);
    asks[0].hop["recall_query"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

#[test]
fn a_german_round_names_its_people_not_its_nouns() {
    if !push_shipped() {
        return;
    }
    let mut h = talky();
    round(
        &mut h,
        "s1",
        "t1",
        "Gestern war meine Tochter Anna mit mir beim Arzt.",
        "Wie schön, dass Ihre Tochter Sie begleitet hat. Was hat der Doktor gesagt?",
    );
    things(&mut h, "s1", "t1", &["Anna"]);
    round(
        &mut h,
        "s1",
        "t2",
        "Der neue Termin ist am 12.03. bei Herrn Becker.",
        "Dann notiere ich mir den Termin bei Herrn Becker.",
    );
    things(&mut h, "s1", "t2", &["Becker"]);
    let q = query(&mut h, "t3", "Was meinst du dazu?");
    let got = mentioned(&q);
    for name in ["Anna", "Becker"] {
        assert!(got.iter().any(|g| g == name), "{name} in {got:?}: {q}");
    }
    // GH #1523: a date is no thing the model named -- it stays out.
    assert!(
        !got.iter().any(|g| g == "12.03."),
        "a date is no name: {got:?}"
    );
    for noun in [
        "Tochter", "Arzt", "Termin", "Herrn", "Doktor", "Ihre", "Sie",
    ] {
        assert!(!got.iter().any(|g| g == noun), "{noun} is no name: {got:?}");
    }
}

/// E4 (one round over the levers), since GH #1523 without a list: a noun the
/// model does not name as a thing is no name, whatever its form ("Tanten",
/// "Entlastung", "Gesellschaft"); the name it names is one ("Dennis").
#[test]
fn a_noun_by_its_form_is_no_name() {
    if !push_shipped() {
        return;
    }
    let mut h = talky();
    round(
        &mut h,
        "s1",
        "t1",
        "Ich brauche Entlastung, und gestern hat Dennis mir Gesellschaft geleistet. Haben Tanten auch Zeit?",
        "Wie schön.",
    );
    things(&mut h, "s1", "t1", &["Dennis"]);
    let q = query(&mut h, "t2", "Was meinst du dazu?");
    let got = mentioned(&q);
    assert!(got.iter().any(|g| g == "Dennis"), "Dennis in {got:?}: {q}");
    for noun in ["Entlastung", "Gesellschaft", "Tanten"] {
        assert!(!got.iter().any(|g| g == noun), "{noun} is no name: {got:?}");
    }
}

/// GH #1523: a name the model named is a name wherever the person says it,
/// at the start of a sentence too; a topic's name alone is none -- only what
/// the model names as a thing enters `mentioned:` (the topic rides as
/// `topic:`).
#[test]
fn a_spelling_the_round_knows_is_a_name_wherever_it_stands() {
    if !push_shipped() {
        return;
    }
    let mut h = talky();
    round(&mut h, "s1", "t1", "hallo", "Hallo!");
    topic(&mut h, "s1", "t1", "Lotte");
    round(
        &mut h,
        "s1",
        "t2",
        "Lotte is coming over tomorrow. And Paul too.",
        "Nice.",
    );
    things(&mut h, "s1", "t2", &["Lotte"]);
    topic(&mut h, "s1", "t2", "Paul");
    topic(&mut h, "s1", "t2", "der Garten");
    let q = query(&mut h, "t3", "und was noch?");
    assert!(q.contains("topic: der Garten"), "{q}");
    let got = mentioned(&q);
    assert!(got.iter().any(|g| g == "Lotte"), "{q}");
    assert!(
        !got.iter().any(|g| g == "Paul"),
        "a topic's name the model named as no thing is no name: {q}"
    );
}
