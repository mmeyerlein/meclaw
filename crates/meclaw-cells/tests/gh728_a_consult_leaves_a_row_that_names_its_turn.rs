//! GH #728, lock 1 — a consult that leaves the round leaves a row that names its turn.
//!
//! The ruling of 2026-09-22: "a late answer never invents an identity … the id of the
//! member turn that started them as a correlation (`consult_id` → `turn_id` looked up
//! on the round row the collector wrote at departure)". Measured before the build: no
//! such row existed. `in_calls` filed the handed-over `assistant` row as fired and the
//! correlation stood only on the RETURN's `turns` row, so the return had nothing to look
//! the member's turn up in.
//!
//! The row is a `round` row with role `depart`: the round key it left from, the
//! correlation the advisor will answer under, the deadline that later tells a leg from
//! a straggler, and `fired = 1`, which keeps it out of every open-round question.
//!
//! The correlation is pinned against the DISPATCHER, not restated: the same brain
//! response goes through the shipped dispatcher and then, as its `calls` emission,
//! through the shipped assembler, and the depart row has to carry exactly the
//! `consult_id` the dispatcher put on the errand (`async_keys`: the call id for a fresh
//! consult, `arguments.consult_id` for a reply to a question the advisor asked back).

#[path = "support/assemble_cell.rs"]
mod assemble_cell;

use assemble_cell::*;
use serde_json::{Value, json};

const T: &str = "chat#0f1e2d3c4b5a6978";

/// One brain response: a sentence, a fresh consult and a reply to an open one.
fn brain_response() -> Value {
    let fresh = json!({"name": "consult_cogny",
                       "arguments": json!({"question": "weather in berlin"}).to_string()});
    let reply = json!({"name": "consult_cogny",
                       "arguments": json!({"consult_id": "k-7", "answer": "Berlin"}).to_string()});
    json!([
        {"origin": "assistant", "type": "text", "text": "one moment, I am asking"},
        {"origin": "assistant", "type": "tool_call", "id": "call-1", "text": fresh.to_string()},
        {"origin": "assistant", "type": "tool_call", "id": "call-2", "text": reply.to_string()}
    ])
}

/// The dispatcher's two emissions this lock reads: the `calls` bundle and the errands.
fn dispatched() -> Vec<Value> {
    let doc = json!({
        "header": {"context": {"session_id": SESSION, "turn_id": T, "iter": "0"},
                   "hop": {"finish_reason": "tool_calls"}},
        "messages": brain_response()
    });
    run_cell(
        DISPATCHER,
        &[("handoff_tools", json!(["consult_cogny"]))],
        doc,
    )
    .0
}

fn departs_of(bundle: &Value) -> Vec<Value> {
    calls_of(bundle)
        .into_iter()
        .filter(|(_, a)| a["operation"] == "insert" && a["row"]["role"] == "depart")
        .map(|(_, a)| a["row"].clone())
        .collect()
}

fn fan_in() -> Vec<Value> {
    let d = dispatched();
    let calls = on_route(&d, "calls");
    assemble(
        &[],
        lane(
            "in_calls",
            json!({"async_calls": calls["header"]["async_calls"],
                   "handoff_calls": calls["header"]["handoff_calls"]}),
            json!({"turn_id": T, "iter": "0"}),
            calls["messages"].clone(),
        ),
    )
}

#[test]
fn a_handed_consult_leaves_a_depart_row_under_the_round_it_left() {
    let before = now_ms();
    let out = fan_in();
    let bundle = &out[0];
    let departs = departs_of(bundle);
    assert_eq!(departs.len(), 2, "one depart row per handed call: {bundle}");
    for row in &departs {
        assert_eq!(
            row["turn_id"], T,
            "the row names the turn it left from: {row}"
        );
        assert_eq!(row["session_id"], SESSION);
        assert_eq!(
            row["fired"], 1,
            "a departure is never an open round: no guard, no sweep, no deferral"
        );
        let deadline = row["deadline_ms"]
            .as_i64()
            .expect("deadline_ms is a number");
        assert!(
            deadline >= before + 30_000 && deadline <= now_ms() + 30_000,
            "the deadline is the shipped late_after_ms from now: {deadline} vs {before}"
        );
    }
    // The assistant row still comes FIRST in the bundle and still closes the round:
    // the member's turn ended with its interim answer, as before (GH #372).
    let (first, asst) = &calls_of(bundle)[0];
    assert!(first.ends_with("-row"), "{first}");
    assert_eq!(asst["row"]["role"], "assistant");
    assert_eq!(asst["row"]["fired"], 1);
    // And the read-back is still the last op, so it sees every insert in front of it.
    let (last, read) = calls_of(bundle).last().cloned().expect("ops");
    assert!(last.ends_with("-read"), "{last}");
    assert_eq!(read["operation"], "select");
}

#[test]
fn the_correlation_on_the_row_is_the_one_the_dispatcher_put_on_the_errand() {
    let d = dispatched();
    let mut errands: Vec<(String, String)> = d
        .iter()
        .filter(|m| m["header"]["route"] == "tool")
        .map(|m| {
            (
                m["header"]["tool_call_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                m["header"]["consult_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    errands.sort();
    assert_eq!(
        errands,
        vec![
            ("call-1".to_string(), "call-1".to_string()),
            ("call-2".to_string(), "k-7".to_string())
        ],
        "the dispatcher's own rule (async_keys) as this lock reads it"
    );

    let out = fan_in();
    let mut rows: Vec<(String, String)> = departs_of(&out[0])
        .iter()
        .map(|r| {
            let turn: Value =
                serde_json::from_str(r["turn"].as_str().expect("turn json")).expect("turn");
            (
                turn["id"].as_str().unwrap_or_default().to_string(),
                r["correlation"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    rows.sort();
    assert_eq!(
        rows, errands,
        "the depart row and the errand disagree about the correlation — the return \
         would look its turn up under an id nobody answers with"
    );
}

#[test]
fn a_call_that_is_not_handed_over_leaves_no_departure() {
    // An async call without handoff (a fire-and-forget write) never comes back as an
    // event, so there is nothing to correlate and nothing to write.
    let out = assemble(
        &[],
        lane(
            "in_calls",
            json!({"async_calls": "c1", "handoff_calls": ""}),
            json!({"turn_id": T}),
            json!([{"origin": "assistant", "type": "tool_call", "id": "c1",
                    "text": "{\"name\":\"remember\",\"arguments\":\"{}\"}"}]),
        ),
    );
    assert!(departs_of(&out[0]).is_empty(), "{out:?}");
}

#[test]
fn the_round_table_declares_the_two_columns_the_row_writes() {
    let schema = &config_of("templates/collector/window/config.json")["params"]["schema"]["round"];
    assert_eq!(schema["correlation"], "text", "{schema}");
    assert_eq!(schema["deadline_ms"], "int", "{schema}");
}
