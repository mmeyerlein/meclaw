//! D58 judge (GH #1480): the curator's VIEW and the CONTRIBUTIONS to a
//! target's section, driven on `./history` alone (`support/history_judge.rs`
//! runs the shipped script against the hive's ledger).
//!
//! The forms the pins of the strand hold (plan part `u-judge.md`, contract
//! points 1 to 4):
//!
//! * a view is asked on the lane `in_view_ask` with the body
//!   `{messages: [], view_ask: {target?, turns, budget_tokens}}` and answered
//!   by exactly one `view`, `{messages, system}`;
//! * a contribution arrives on the lane `in_part` as
//!   `{messages: [], target, source, value, subject?, until?}`, or as an entry
//!   of the list `parts` in the body of a curate, and answers nothing;
//! * in the `system` tree of a view the contribution is ONE leaf under the
//!   name of its source in the family of its target,
//!   `system[<target>][<source>] = {"text": ...}`: a text value as it came, a
//!   structured value as its JSON text (the curator never reads it).
#![allow(dead_code)]

use crate::curator_hive::*;
use crate::history_judge::*;
use meclaw_core::serde_json::{self as sj, Map, Value, json};

/// The history cell as shipped with the clock fixed and a place and a zone,
/// so a view's time and place are facts the test can name.
pub fn view_params() -> (String, Map<String, Value>) {
    history_params(
        &[
            ("place", json!("Berlin")),
            ("timezone", json!("Europe/Berlin")),
        ],
        &[],
    )
}

/// The body of a view ask: `target` "" asks a view without a target.
pub fn view_ask_body(target: &str, turns: u64, budget: u64) -> Value {
    let mut ask = json!({"turns": turns, "budget_tokens": budget});
    if !target.is_empty() {
        ask["target"] = json!(target);
    }
    json!({"messages": [], "view_ask": ask})
}

/// One view ask of `session`'s running `turn` for `target`.
pub fn view_ask(
    h: &Hive,
    params: &(String, Map<String, Value>),
    target: &str,
    session: &str,
    turn: &str,
) -> Run {
    drive(
        h,
        params,
        "in_view_ask",
        json!({"session_id": session, "turn_id": turn}),
        json!({}),
        view_ask_body(target, 4, 1500),
    )
}

/// One contribution on `in_part`; `subject` "" leaves it out.
pub fn part(
    h: &Hive,
    params: &(String, Map<String, Value>),
    target: &str,
    source: &str,
    value: Value,
    subject: &str,
) -> Run {
    let mut body = json!({"messages": [], "target": target, "source": source, "value": value});
    if !subject.is_empty() {
        body["subject"] = json!(subject);
    }
    drive(
        h,
        params,
        "in_part",
        json!({"session_id": "s-now"}),
        json!({}),
        body,
    )
}

/// The one `view` a run answered with (fails the test when there is none).
pub fn the_view(run: &Run) -> Msg {
    assert_eq!(
        run.out.len(),
        1,
        "a view ask is answered by exactly one view: {:?}",
        run.out
    );
    assert_eq!(run.out[0].route(), "view", "{:?}", run.out[0].hop);
    run.out[0].clone()
}

/// The leaf `system[target][source]` of a view, or Null.
pub fn leaf(view: &Msg, target: &str, source: &str) -> Value {
    view.body
        .get("system")
        .and_then(|s| s.get(target))
        .and_then(|f| f.get(source))
        .cloned()
        .unwrap_or(Value::Null)
}

/// The value a leaf carries: its text, read back as JSON when the value was
/// structured.
pub fn leaf_value(leaf: &Value) -> Value {
    let text = leaf["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a leaf is {{\"text\": ...}}: {leaf}"));
    sj::from_str::<Value>(text).unwrap_or_else(|_| json!(text))
}

/// `(origin, text)` of a view's messages, in their order.
pub fn said_in(view: &Msg) -> Vec<(String, String)> {
    view.body["messages"]
        .as_array()
        .unwrap_or_else(|| panic!("a view carries `messages`: {:?}", view.body))
        .iter()
        .map(|m| {
            (
                m["origin"].as_str().unwrap_or("").to_string(),
                m["text"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

/// The screen topics of the presenter as one structured value.
pub fn screen_value() -> Value {
    json!([
        {"name": "weather", "purpose": "the weather today and the next days",
         "candidates": [{"key": "today", "describe": "today's weather"},
                        {"key": "week", "describe": "the week ahead"}]},
        {"name": "calendar", "purpose": "the person's appointments"}
    ])
}

/// The person's earlier how-questions as the affinity brief brings them.
pub fn hints_value() -> Value {
    json!([{"question": "Its concerts or its website?", "answer": "its concerts",
            "class": "simple"}])
}

/// Every table of the ledger with its row count: what a read must leave as
/// it found it.
pub fn ledger_counts(h: &Hive) -> Vec<(String, i64)> {
    let tables: Vec<String> = h
        .rows("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .into_iter()
        .filter_map(|r| r.first().and_then(|v| v.as_str()).map(str::to_string))
        .collect();
    tables
        .into_iter()
        .map(|t| {
            let n = h
                .rows(&format!("SELECT COUNT(*) FROM \"{t}\""))
                .first()
                .and_then(|r| r.first())
                .and_then(|v| v.as_i64())
                .unwrap_or(-1);
            (t, n)
        })
        .collect()
}

/// The ledger operations of a run, one entry per operation.
pub fn ledger_operations(run: &Run) -> Vec<Value> {
    run.store
        .iter()
        .flat_map(|m| m.messages())
        .filter(|m| m["type"] == "tool_call")
        .map(|c| sj::from_str::<Value>(c["text"].as_str().unwrap_or("")).unwrap_or(Value::Null))
        .collect()
}
