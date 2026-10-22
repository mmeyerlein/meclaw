//! GH #1292 (S2) -- the menu row of a turn answerer never folds, and its empty
//! answer is written.
//!
//! WHY this file exists: the menu merge is answerer-blind. With the fold on,
//! two client tools `look_at` and `look_around` became a family `look` the
//! caller never declared (and whose calls the dispatcher would unfold), and a
//! turn that brought no client tools left the last turn's ones standing: an
//! empty menu answer is parked, never recorded (GH #529). Client tools are
//! valid for one turn (OR-SA-2), so the collector's `turn_answerers` (default
//! `[]`, the simple agent `["client"]`) names the answerers whose rows stand
//! outside the fold and whose empty answer empties their row.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;

const TURN: &str = "chan#0001";

fn decl(name: &str) -> Value {
    json!({"name": name, "description": format!("{name} does it."),
           "parameters": {"type": "object", "properties": {}}})
}

fn rows(client: &[&str]) -> Value {
    json!([
        {"answerer": "client", "tools": client.iter().map(|n| decl(n)).collect::<Vec<_>>()},
        {"answerer": "tools", "tools": [decl("web_search"), decl("web_fetch")]}
    ])
}

fn turn_on() -> Vec<(&'static str, Value)> {
    vec![
        ("fold_tools", json!("1")),
        ("turn_answerers", json!(["client"])),
    ]
}

fn names(msg: &Value) -> BTreeSet<String> {
    msg["system"]["tools"]
        .as_object()
        .unwrap_or_else(|| panic!("no tools subtree: {msg}"))
        .keys()
        .filter(|k| *k != "$replace")
        .cloned()
        .collect()
}

fn want(v: &[&str]) -> BTreeSet<String> {
    v.iter().map(|s| (*s).to_string()).collect()
}

fn merge(over: &[(&str, Value)], rows: Value) -> Vec<Value> {
    assemble(
        over,
        bundle_reply("menu-merge", "", &[("c-menu-all", rows)]),
    )
}

fn empty_answer(over: &[(&str, Value)]) -> Vec<Value> {
    let mut doc = lane(
        "in_menu",
        json!({}),
        json!({"tool_answerer": "client"}),
        json!([]),
    );
    doc["schemas"] = json!([]);
    assemble(over, doc)
}

#[test]
fn a_turn_answerers_rows_never_fold() {
    let out = merge(&turn_on(), rows(&["look_at", "look_around"]));
    let menu = out.first().unwrap_or_else(|| panic!("nothing written"));
    assert_eq!(
        names(menu),
        want(&["look_around", "look_at", "web"]),
        "the client tools stay single, the own family folds: {menu}"
    );
    assert_eq!(hop_str(menu, "menu_folded"), "web", "{menu}");

    // Shipped default: every row folds as before.
    let out = merge(
        &[("fold_tools", json!("1"))],
        rows(&["look_at", "look_around"]),
    );
    assert_eq!(names(&out[0]), want(&["look", "web"]));
}

#[test]
fn a_turn_answerers_empty_answer_writes_its_empty_row() {
    let out = empty_answer(&turn_on());
    let put = call(in_phase(&out, "menu-merge"), "c-menu-put");
    assert_eq!(put["row"]["answerer"], "client", "{put}");
    assert_eq!(put["row"]["tools"], json!([]), "{put}");

    // Shipped default: an empty answer is parked (GH #529).
    assert!(empty_answer(&[]).is_empty());
}

#[test]
fn an_emptied_turn_menu_revokes_even_when_it_was_the_only_one() {
    let out = merge(&turn_on(), json!([{"answerer": "client", "tools": []}]));
    let menu = out.first().unwrap_or_else(|| panic!("nothing written"));
    assert_eq!(menu["system"]["tools"], json!({"$replace": true}), "{menu}");

    // Shipped default: no rows with a declaration write nothing.
    assert!(merge(&[], json!([{"answerer": "client", "tools": []}])).is_empty());
}

#[test]
fn the_seam_names_no_family_of_a_turn_answerer() {
    let opened = assemble(
        &turn_on(),
        lane(
            "in_turn",
            json!({"turn_id": TURN}),
            json!({"turn_id": TURN, "channel": "room-1"}),
            json!([{"origin": "user", "type": "text", "text": "hello"}]),
        ),
    );
    assert_eq!(
        call(in_phase(&opened, "turn-open"), "c-open-menu")["table"],
        "menu"
    );
    let mut reply = bundle_reply(
        "turn-open",
        TURN,
        &[
            ("c-open-turn", Value::Null),
            ("c-open-round", json!([])),
            (
                "c-open-win",
                json!([{"id": "0001", "session_id": SESSION, "turn_id": TURN,
                        "role": "user", "content": "hello", "deferred": 0,
                        "consult_id": ""}]),
            ),
            ("c-open-scope", json!([])),
            ("c-open-roster", json!([])),
            ("c-open-menu", rows(&["look_at", "look_around"])),
        ],
    );
    reply["header"]["context"]["channel"] = json!("room-1");
    let (parked, stderr) = run_cell(ASSEMBLE, &turn_on(), reply);
    let mut w = calls_of(in_phase(&parked, "collect"))
        .into_iter()
        .filter(|(_, a)| a["operation"] == "insert")
        .map(|(_, a)| a["row"].clone())
        .find(|r| r["role"] == "leg-window")
        .unwrap_or_else(|| panic!("no leg-window parked: {parked:?} {stderr}"));
    w["fired"] = json!(0);
    let out = assemble(
        &turn_on(),
        bundle_reply("collect", TURN, &[("c-collect-read", json!([w]))]),
    );
    let c = on_route(&out, "curate");
    assert_eq!(
        hop_str(c, "tools_folded"),
        "web",
        "a client tool pair is no family the dispatcher unfolds: {c}"
    );
}
