//! D58 judge (GH #1480, contract point 4) -- a start window line keeps its
//! place without a kind.
//!
//! WHY this file exists: `in_seed_line` stays, but the curator no longer
//! knows the kinds `wind_down`, `hint` and `identity` of the set's senders.
//! The sender names the place itself: `position` (`head` or `tail`), `order`
//! (a number, lower first) and `lasts` (`session`: once, for the running
//! session's next seed; `always`: in every seed until a newer line replaces
//! it). A newer line in the same place -- position and order, and for a
//! `session` line the same call -- replaces the older one. Today's senders in that form give today's start
//! window byte for byte (history.py `SEED_LINE_ORDER`, `SEED_HEAD_ORDER` and
//! the persona anchor's place right before the wind-down line):
//!
//!   identity  = head, always;
//!   hint      = tail, order 1, session;
//!   wind_down = tail, order 2, session.
//!
//! Runs the shipped `./history` against the hive's ledger.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/history_judge.rs"]
#[allow(unused_imports)]
use crate::history_judge; // harness_fold: mod history_judge;
// harness_fold: #[path = "support/history_view.rs"]
#[allow(unused_imports)]
use crate::history_view; // harness_fold: mod history_view;

use curator_hive::*;
use history_judge::*;
use meclaw_core::serde_json::{Map, Value, json};

const ANCHOR: &str = "Stay who you are: warm, brief, curious.";

fn params() -> (String, Map<String, Value>) {
    history_params(&[("seed_anchor", json!(ANCHOR))], &[])
}

/// One start window line without a kind; `call` "" sends none.
fn line(h: &Hive, p: &(String, Map<String, Value>), call: &str, at: Value, text: &str) {
    let mut body = json!({"messages": [], "text": text});
    for (k, v) in at.as_object().expect("place") {
        body[k] = v.clone();
    }
    if !call.is_empty() {
        // The running session's handle: `session_id` is read before and
        // after strand `live` (which adds `live` beside it).
        body["session_id"] = json!(call);
    }
    let run = drive(h, p, "in_seed_line", json!({}), json!({}), body);
    assert!(run.out.is_empty(), "a line answers nothing: {:?}", run.out);
}

fn identity() -> Value {
    json!({"position": "head", "lasts": "always"})
}
fn hint() -> Value {
    json!({"position": "tail", "order": 1, "lasts": "session"})
}
fn wind_down() -> Value {
    json!({"position": "tail", "order": 2, "lasts": "session"})
}

/// The texts of a seed after its fact line.
fn texts(seed: &Msg) -> Vec<String> {
    seed.body["messages"]
        .as_array()
        .expect("a seed's messages")
        .iter()
        .skip(1)
        .map(|m| m["text"].as_str().unwrap_or("").to_string())
        .collect()
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

#[test]
fn gh_d58_judge_a_start_window_line_keeps_its_place_without_a_kind() {
    let p = params();
    let mut h = Hive::new();
    said_turn(&mut h, 1, "s-now", "t1", "the choir", "the choir");

    // Sent in another order than they stand: the place is the sender's word.
    line(&h, &p, "call-1", wind_down(), "The call ends in a minute.");
    line(&h, &p, "call-1", hint(), "Remind them of the concert.");
    line(&h, &p, "", identity(), "You are Egon.");
    let seed = only(&seed_ask(&h, &p, "call-1"), "seed");
    assert_eq!(
        texts(&seed),
        s(&[
            "You are Egon.",
            "the choir",
            "about the choir",
            "Remind them of the concert.",
            ANCHOR,
            "The call ends in a minute.",
        ]),
        "head after the facts, the tail by order, the anchor right before the last line"
    );

    // `session`: used once; `always`: in the next seed of every call too.
    let again = only(&seed_ask_for(&h, &p, "call-1", "renew", 1), "seed");
    assert_eq!(
        texts(&again),
        s(&["You are Egon.", "the choir", "about the choir", ANCHOR])
    );

    // Today's single lines keep today's places: a hint alone ends before the
    // anchor, a wind-down alone after it.
    line(&h, &p, "call-2", hint(), "Ask about the garden.");
    let hint_only = only(&seed_ask(&h, &p, "call-2"), "seed");
    assert_eq!(
        texts(&hint_only),
        s(&[
            "You are Egon.",
            "the choir",
            "about the choir",
            "Ask about the garden.",
            ANCHOR,
        ])
    );
    line(&h, &p, "call-3", wind_down(), "Time to say goodbye.");
    let wind_only = only(&seed_ask(&h, &p, "call-3"), "seed");
    assert_eq!(
        texts(&wind_only),
        s(&[
            "You are Egon.",
            "the choir",
            "about the choir",
            ANCHOR,
            "Time to say goodbye.",
        ])
    );
    assert!(
        h.rows("SELECT key FROM state WHERE key LIKE 'judge:%'")
            .is_empty(),
        "no state key `judge:*` any more"
    );
}
