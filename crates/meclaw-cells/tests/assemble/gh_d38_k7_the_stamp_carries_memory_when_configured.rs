//! GH #1435 (D38 K7) -- the route stamp can name `memory` in `needs`.
//!
//! WHY this file exists: a question about the person's own life (their
//! people, their past, something said before today) was stamped `simple` and
//! left on the fast path, which carries no memory rows; the fast model filled
//! the gap by guessing (window bench: invented names). The route cell's param
//! `needs` is an object of yes/no questions; a set adds a fourth entry
//! `memory` and the stamp then names `memory` like `tool`. The shipped object
//! stays the three entries of before, byte for byte, and the cell declares the
//! param at the door so a set's object passes (d53-3 K4 (1)).
//!
//! Runs the shipped `talky/route` script (`support/assemble_cell.rs`).

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::*;
use serde_json::{Value, json};

const ROUTE: &str = "templates/talky/route/config.json";
const MEMORY_Q: &str =
    "something the person said earlier or before today -- their people, their life, their past";

/// The set's object: the three shipped entries plus `memory`.
fn four_needs() -> Value {
    json!({"tool": "a tool call",
           "reasoning": "careful step-by-step reasoning",
           "research": "a deep search over many sources",
           "memory": MEMORY_Q})
}

fn turn_in() -> Value {
    lane(
        "in_turn",
        json!({}),
        json!({"turn_id": "c-1"}),
        json!([{"origin": "user", "type": "text", "text": "How is my sister doing?"}]),
    )
}

fn decision(answers: Value) -> Value {
    // Asked an hour ahead: the duration clamps to 0, never late (dev-rules § 11).
    let mut doc = lane(
        "in_decision",
        json!({"finish_reason": "stop", "asked_at": (now_ms() + 3_600_000).to_string()}),
        json!({"turn_id": "c-1"}),
        json!([]),
    );
    doc["decision"] = json!({"answers": answers, "missing": [], "model": "d"});
    doc
}

fn one(out: Vec<Value>, route: &str) -> Value {
    assert_eq!(
        out.len(),
        1,
        "expected one emission on {route}, got {out:?}"
    );
    let m = out.into_iter().next().expect("one emission");
    assert_eq!(m["header"]["route"], route, "{m}");
    m
}

/// The shipped object is the three entries of before, byte for byte, and the
/// door knows the param: `contract.settings.needs` carries the same default.
#[test]
fn the_shipped_needs_object_is_byte_identical_and_declared_at_the_door() {
    let cfg = config_of(ROUTE);
    let shipped = json!({"tool": "a tool call",
                         "reasoning": "careful step-by-step reasoning",
                         "research": "a deep search over many sources"});
    assert_eq!(
        cfg["params"]["needs"], shipped,
        "the shipped needs: {}",
        cfg["params"]["needs"]
    );
    let setting = &cfg["contract"]["settings"]["needs"];
    assert!(
        setting.is_object(),
        "templates/talky/route/config.json declares needs under contract.settings: {setting}"
    );
    assert_eq!(
        setting["default"], shipped,
        "the setting's default is the shipped object"
    );
    assert!(
        setting["description"]
            .as_str()
            .unwrap_or("")
            .contains("memory"),
        "the setting names the optional fourth entry: {setting}"
    );
}

/// With the set's object the decider is asked the memory question, and a yes
/// puts `memory` into the stamp's `needs`, sorted like every need.
#[test]
fn a_set_object_with_four_entries_asks_the_memory_question_and_the_stamp_names_memory() {
    let over = [("needs", four_needs())];
    let call = one(run_cell(ROUTE, &over, turn_in()).0, "decide");
    let qs = &call["decide"]["questions"];
    assert_eq!(qs["needs.memory"]["kind"], "yes_no", "{qs}");
    assert_eq!(
        qs["needs.memory"]["instructions"],
        json!(format!("Does answering it need {MEMORY_Q}?")),
        "{qs}"
    );
    for k in ["needs.tool", "needs.reasoning", "needs.research"] {
        assert_eq!(qs[k]["kind"], "yes_no", "{k} is still asked: {qs}");
    }

    let yes_memory = json!({
        "path": {"choice": "simple", "p": 0.91},
        "intent": {"choice": "question", "p": 0.8},
        "needs.memory": {"yes": 0.9}, "needs.tool": {"yes": 0.1},
        "needs.reasoning": {"yes": 0.1}, "needs.research": {"yes": 0.1},
        "families.web": {"yes": 0.05}, "families.history": {"yes": 0.9}
    });
    let m = one(run_cell(ROUTE, &over, decision(yes_memory)).0, "stamp");
    assert_eq!(m["stamp"]["path"], "simple", "{m}");
    assert_eq!(m["stamp"]["needs"], json!(["memory"]), "{m}");
    assert_eq!(m["stamp"]["families"], json!(["history"]), "{m}");

    let both = json!({
        "path": {"choice": "simple", "p": 0.91},
        "intent": {"choice": "question", "p": 0.8},
        "needs.memory": {"yes": 0.9}, "needs.tool": {"yes": 0.9},
        "needs.reasoning": {"yes": 0.1}, "needs.research": {"yes": 0.1}
    });
    let m = one(run_cell(ROUTE, &over, decision(both)).0, "stamp");
    assert_eq!(
        m["stamp"]["needs"],
        json!(["memory", "tool"]),
        "sorted: {m}"
    );

    // Without the set's entry nothing asks for memory and nothing stamps it.
    let call = one(run_cell(ROUTE, &[], turn_in()).0, "decide");
    assert!(
        call["decide"]["questions"].get("needs.memory").is_none(),
        "{call}"
    );
    let m = one(
        run_cell(
            ROUTE,
            &[],
            decision(json!({
                "path": {"choice": "simple", "p": 0.91},
                "intent": {"choice": "question", "p": 0.8},
                "needs.memory": {"yes": 0.9}, "needs.tool": {"yes": 0.1},
                "needs.reasoning": {"yes": 0.1}, "needs.research": {"yes": 0.1}
            })),
        )
        .0,
        "stamp",
    );
    assert_eq!(
        m["stamp"]["needs"],
        json!([]),
        "an unasked need is never stamped: {m}"
    );
}
