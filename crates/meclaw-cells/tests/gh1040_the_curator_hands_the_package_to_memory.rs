//! GH #1040 — the curator hands the answering model's package to the memory.
//!
//! The memory hive sizes its bundle and its query guard by the asker's
//! `input_soft` (`gh1040_the_recall_is_sized_by_the_model_package.rs`). The
//! recall cell never sees a hop of the model: the collector's ask of a turn
//! leaves before the model answers. So `./policy` keeps the `input_soft` the
//! llm cell stamped on the last answer (GH #1037) and `./push` hands it over on
//! every ask as `recall_input_soft` -- empty when no answer named a package.
//!
//! Runs the shipped curator hive through its real lanes (`support/curator_hive.rs`).

#[path = "support/curator_hive.rs"]
mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const ROUND: &str = r#"["member:e"]"#;

fn talky() -> Hive {
    Hive::with(&[("policy", "role", json!("talky"))])
}

/// The collector's ask of one turn; returns every `recall` that left for it.
fn ask(h: &mut Hive, turn: &str, text: &str) -> Vec<Msg> {
    h.out.clear();
    h.lane(
        "in_recall_ask",
        json!({"session_id": "s1", "channel": "test", "audience_set": ROUND}),
        json!({"phase": "recall", "turn_id": turn, "session_id": "s1", "iter": "0",
               "recall_query": text, "memory_tier": "1",
               "recall_window_from": "", "recall_window_to": ""}),
        json!({"messages": [user(text)]}),
    );
    h.routed("recall")
}

/// One round of the model, answered on the tap with `usage`.
fn answer(h: &mut Hive, turn: &str, text: &str, usage: Value) {
    h.out.clear();
    h.lane(
        "in_curate",
        json!({"session_id": "s1", "turn_id": turn, "iter": "0", "channel": "test",
               "audience_set": ROUND}),
        json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": ""}),
        json!({"messages": [user(text)], "system": mode("")}),
    );
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "{:?} {:?}", h.out, h.stderr);
    h.tap(&calls[0], "stop", usage, json!([said("noted")]));
}

fn soft_of(asks: &[Msg]) -> String {
    assert_eq!(asks.len(), 1, "every ask leaves exactly once");
    asks[0].hop["recall_input_soft"]
        .as_str()
        .unwrap_or_else(|| panic!("the ask carries the key, empty or not: {:?}", asks[0].hop))
        .to_string()
}

#[test]
fn the_package_of_the_last_answer_rides_on_the_next_ask() {
    let mut h = talky();
    assert_eq!(
        soft_of(&ask(&mut h, "t1", "Who is my son?")),
        "",
        "no answer yet"
    );
    answer(
        &mut h,
        "t1",
        "Who is my son?",
        json!({"input_soft": 250000, "input_hard": 1000000, "cost_in": 10.0,
               "cost_cached_in": 1.0}),
    );
    // GH #1085 (R-IG-1): the window is luna's catalog row, `input_soft`
    // 250 000 -- no role cuts it any more (talky's `quality_cap` made it
    // 120 000 until curator 1.11.2).
    assert_eq!(h.state("input_soft"), "250000", "{:?}", h.stderr);
    assert_eq!(
        soft_of(&ask(&mut h, "t2", "And what does he do?")),
        "250000"
    );
}

#[test]
fn the_package_is_cut_to_the_models_window() {
    // A model window smaller than the soft limit, a soft limit under the
    // model window, and luna's row whole: the window is the model's own.
    for (usage, kept) in [
        (
            json!({"input_soft": 250000, "context_window": 1050000}),
            "250000",
        ),
        (
            json!({"input_soft": 250000, "context_window": 100000}),
            "100000",
        ),
        (
            json!({"input_soft": 60000, "context_window": 1000000}),
            "60000",
        ),
    ] {
        let mut h = talky();
        ask(&mut h, "t1", "Who is my son?");
        answer(&mut h, "t1", "Who is my son?", usage.clone());
        assert_eq!(h.state("input_soft"), kept, "{usage} {:?}", h.stderr);
    }
}

#[test]
fn an_answer_without_a_package_hands_none_on() {
    let mut h = talky();
    ask(&mut h, "t1", "Who is my son?");
    answer(&mut h, "t1", "Who is my son?", json!({}));
    assert_eq!(h.state("input_soft"), "");
    assert_eq!(soft_of(&ask(&mut h, "t2", "And what does he do?")), "");
}
