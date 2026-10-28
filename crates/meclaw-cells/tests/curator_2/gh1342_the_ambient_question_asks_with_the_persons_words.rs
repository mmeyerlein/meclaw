//! GH #1342 -- the ambient memory question is built from the person's own
//! words of the running session, never from the model's answers of an
//! earlier one.
//!
//! WHY this file exists: in a canary run (0.76.2) a person went through a
//! crisis in one session; the model's answer there named the emergency
//! number, a helpline and the person's own crisis sentence. In the next,
//! fresh session the first turn ("I am so sad today, my son cancelled
//! again") went to the memory as
//! `[mentioned: 112, ..., helpline, 0800] <the person's words>` -- the
//! anchors came from the assistant's text of the session before. The recall
//! carried the crisis into the new conversation, and the model raised it
//! unasked. What came before is background and comes up only when asked;
//! anything delicate reaches a turn only as a note addressed to it. So the
//! anchors of the ambient question are the person's words of the session
//! that runs, and the model's own answers never feed them (structure: the
//! rows' kind and session, never a reading of the words).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

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

fn query(h: &mut Hive, session: &str, turn: &str, text: &str) -> String {
    let asks = ask(h, session, turn, text);
    assert_eq!(asks.len(), 1, "{:?}", h.stderr);
    asks[0].hop["recall_query"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

/// The trace of the canary, in its form: a crisis in session `w2`, the
/// model's answer naming the emergency number and a helpline; the first
/// turn of the fresh session `s1` asks the memory with the person's words
/// alone.
#[test]
fn a_fresh_session_asks_without_the_answers_of_the_last_one() {
    if !push_shipped() {
        return;
    }
    let mut h = talky();
    round(
        &mut h,
        "w2",
        "w2#1",
        "Mein Enkel hat mir ein Video von seinem Hund geschickt, ich musste so lachen.",
        "Das freut mich. Was macht der Hund denn in dem Video?",
    );
    round(
        &mut h,
        "w2",
        "w2#2",
        "Ich habe Tabletten hier und will mir das Leben nehmen.",
        "Bitte ruf jetzt die 112 an, die Telefonseelsorge hat 0800 111 0 111. Bleib in Reichweite.",
    );
    let text = "Ich bin heute so traurig, mein Sohn hat schon wieder abgesagt.";
    let q = query(&mut h, "s1", "s1#1", text);
    for word in [
        "112",
        "Telefonseelsorge",
        "0800",
        "Tabletten",
        "Reichweite",
        "Video",
    ] {
        assert!(!q.contains(word), "{word} from an earlier session in: {q}");
    }
    assert_eq!(
        q, text,
        "nothing of an earlier session anchors the question"
    );
}

/// Inside the running session the person's words anchor the question; the
/// model's own answers never do.
#[test]
fn the_anchors_are_the_persons_words_never_the_models() {
    if !push_shipped() {
        return;
    }
    let mut h = talky();
    round(
        &mut h,
        "s1",
        "t1",
        "Gestern war ich mit Herrn Hartung im Rosengarten.",
        "Und hat Frau Bader schon angerufen? Die Telefonseelsorge ist da.",
    );
    let q = query(&mut h, "s1", "t2", "Was meinst du dazu?");
    for name in ["Hartung", "Rosengarten"] {
        assert!(q.contains(name), "{name} from the person in: {q}");
    }
    for word in ["Bader", "Telefonseelsorge"] {
        assert!(!q.contains(word), "{word} from the model's answer in: {q}");
    }
}
