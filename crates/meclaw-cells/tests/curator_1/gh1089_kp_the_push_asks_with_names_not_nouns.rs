//! GH #1089 on the curator (GH #1225 B4) -- the push asks with names, not nouns.
//!
//! WHY this file exists: `curator/push` took every capital that opens no
//! sentence for a name -- an English rule. In German every noun carries one:
//! `mentioned:` filled its eight places with Tochter, Arzt, Termin and the
//! polite "Ihrer", and the name the memory needed fell off the end. Now the
//! words of a language that are never a name are data (`entity_stop_lang`, one
//! list per language, the German one the fixture `entity_stop_de.jsonl`
//! that the memory hive reads too), a capital behind a determiner is a noun
//! (`entity_det_lang`), and a spelling the round already knows as a name is taken
//! wherever it stands. Nothing guesses a name.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn push_shipped() -> bool {
    repo("templates/curator/push/config.json").is_file()
}

fn talky() -> Hive {
    Hive::with(&[("policy", "role", json!("talky"))])
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
    round(
        &mut h,
        "s1",
        "t2",
        "Der neue Termin ist am 12.03. bei Herrn Becker.",
        "Dann notiere ich mir den Termin bei Herrn Becker.",
    );
    let q = query(&mut h, "t3", "Was meinst du dazu?");
    let got = mentioned(&q);
    for name in ["Anna", "Becker", "12.03."] {
        assert!(got.iter().any(|g| g == name), "{name} in {got:?}: {q}");
    }
    for noun in [
        "Tochter", "Arzt", "Termin", "Herrn", "Doktor", "Ihre", "Sie",
    ] {
        assert!(!got.iter().any(|g| g == noun), "{noun} is no name: {got:?}");
    }
}

/// E4 (one round over the levers): a capital is a noun by its form alone --
/// a word of `entity_stop_lang` with an inflection ending ("Tanten"), or an
/// `entity_suffix_lang` ending behind four letters or more ("Entlastung",
/// "Gesellschaft"); a shorter stem keeps the name ("Dennis").
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
    let q = query(&mut h, "t2", "Was meinst du dazu?");
    let got = mentioned(&q);
    assert!(got.iter().any(|g| g == "Dennis"), "Dennis in {got:?}: {q}");
    for noun in ["Entlastung", "Gesellschaft", "Tanten"] {
        assert!(!got.iter().any(|g| g == noun), "{noun} is no name: {got:?}");
    }
}

#[test]
fn a_spelling_the_round_knows_is_a_name_wherever_it_stands() {
    if !push_shipped() {
        return;
    }
    let mut h = talky();
    round(&mut h, "s1", "t1", "hallo", "Hallo!");
    topic(&mut h, "s1", "t1", "Lotte");
    round(&mut h, "s1", "t2", "Lotte kommt morgen vorbei.", "Schön.");
    topic(&mut h, "s1", "t2", "der Garten");
    let q = query(&mut h, "t3", "und was noch?");
    assert!(q.contains("topic: der Garten"), "{q}");
    assert!(mentioned(&q).iter().any(|g| g == "Lotte"), "{q}");
}

#[test]
fn the_german_list_of_the_push_is_the_fixture() {
    if !push_shipped() {
        return;
    }
    let cfg = read_json(&repo("templates/curator/push/config.json"));
    let raw = std::fs::read_to_string(repo(
        "crates/meclaw-cells/tests/fixtures/entity_stop_de.jsonl",
    ))
    .expect("the fixture");
    let words: Vec<Value> = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| meclaw_core::serde_json::from_str::<Value>(l).expect("a line")["word"].clone())
        .collect();
    assert!(words.len() > 100, "{}", words.len());
    assert_eq!(cfg["params"]["entity_stop_lang"]["de"], json!(words));
    assert_eq!(
        cfg["contract"]["settings"]["entity_stop_lang"]["default"],
        cfg["params"]["entity_stop_lang"]
    );
    assert_eq!(
        cfg["contract"]["settings"]["entity_det_lang"]["default"],
        cfg["params"]["entity_det_lang"]
    );
    // OR-KF-50: the word lists are language data, so each key ends in `_lang`
    // (the export audit cuts only `params.<name>_lang`); the language picker is
    // a plain string and does not.
    for (old, new) in [
        ("entity_stop", "entity_stop_lang"),
        ("entity_det", "entity_det_lang"),
        ("entity_suffix", "entity_suffix_lang"),
        ("entity_date", "entity_date_lang"),
        ("entity_lang", "entity_language"),
    ] {
        for at in [&cfg["params"], &cfg["contract"]["settings"]] {
            assert!(at.get(old).is_none(), "{old} is still declared");
            assert!(at.get(new).is_some(), "{new} is not declared");
        }
    }
    assert!(cfg["params"]["entity_language"].is_string());
}
