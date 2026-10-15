//! The thread `weave` rebuilds is not a slate -- it is a REQUEST BODY. It goes
//! straight back into `compose`, an `llm` cell, and from there onto an
//! OpenAI-compatible wire. So every turn in it has to be one the wire accepts.
//!
//! Measured on 2026-08-27 (the builder agentic-loop run series,
//! § 2, 3 of 3 runs): the refine lane worked end to end -- receipt in, parked,
//! `weave` fired at `compose` with `repairs: "1"` -- and then died with
//! `wire: HttpStatus(400)`. The last turn of the rebuilt thread was
//!
//! ```text
//! origin=tool  type=tool_result  id=""   "the submission was refused: edge_schema -- ..."
//! ```
//!
//! `crates/meclaw-cells/src/llm/translate.rs`, `map_turn`, renders that arm
//! unconditionally as `{"role":"tool","tool_call_id":"","content":…}`. On the
//! wire a `role:"tool"` message must answer a `tool_calls[]` entry of the
//! assistant message in front of it; one that answers nothing is a form error,
//! and the provider refuses the whole call. The loop loses its repair chance.
//!
//! A submitter's refusal answers NO tool_call -- it comes from the refine lane,
//! not from a tool round -- so the repair is not to invent a correlation but to
//! stop claiming one: `docs/cell-types.md` sanctions exactly this in the
//! `store` cell's § "Body format of the response" ("In direct use outside a
//! tool-loop, the `origin` may also be `user` or `system` depending on the
//! application convention; `id` is then omitted"), and `("user","text")`
//! renders as `{"role":"user","content":…}` with
//! no id on the wire at all.
//!
//! The second case here is the ORDER of that turn. A receipt is parked under
//! whatever `context.iter` the foreign chain carried -- which is 0, always,
//! because the submitter's answer begins a fresh trace. A build that took two
//! tool rounds would therefore see its refusal sorted BEFORE round 1 instead of
//! at the end of the thread. Single-round builds hid it; the lane fix above is
//! what makes multi-round repairs reachable at all.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_all, shipped_script};

const WEAVE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/weave/config.json"
);

fn run_weave(hop: Value, ctx: Value, body: Value) -> Value {
    let mut flat = json!({"header": {"hop": hop, "context": ctx}, "params": {}});
    if let Value::Object(slots) = body {
        for (slot, v) in slots {
            flat[slot] = v;
        }
    }
    Value::Array(emit_all(&shipped_script(WEAVE), &flat))
}

fn row(iter: i64, role: &str, turn: &str, fired: i64, at: &str) -> Value {
    json!({"build_id": "b7", "iter": iter, "role": role, "turn": turn,
           "fired": fired, "recorded_at": at})
}

fn slate(rows: Vec<Value>) -> Value {
    json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "w-round-read",
                         "text": meclaw_core::serde_json::to_string(&rows).expect("rows")}]})
}

/// The receipt row as the ear ACTUALLY parks it -- run, not spelled. A hand-
/// written fixture here would only ever prove itself: the two lanes have to
/// agree, and the one that writes the row is the authority on its shape.
fn parked_receipt_row(iter: i64, code: &str, fired: i64, at: &str) -> Value {
    let out = run_weave(
        json!({"route": "in_receipt", "error_code": code}),
        json!({"build_id": "b7", "iter": "0", "repairs": "0"}),
        json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "",
                             "text": "because"}]}),
    );
    let bundle = out
        .as_array()
        .expect("multi-send")
        .iter()
        .find(|m| m["header"]["route"] == "cstore")
        .cloned()
        .expect("the ear parks the refusal");
    let op = bundle["messages"]
        .as_array()
        .expect("bundle legs")
        .iter()
        .find(|o| o["id"] == "w-round-row")
        .cloned()
        .expect("the insert leg");
    let insert: Value =
        meclaw_core::serde_json::from_str(op["text"].as_str().unwrap_or("{}")).expect("op json");
    row(
        iter,
        "receipt",
        insert["row"]["turn"].as_str().expect("the parked turn"),
        fired,
        at,
    )
}

/// The wire rule, spelled the way `translate.rs::map_turn` and the provider
/// spell it together. Kept here rather than imported because `map_turn` is
/// `pub(crate)`; the table is copied verbatim from it and the correlation rule
/// is the provider's.
fn assert_wire_legal(thread: &[Value], what: &str) {
    let mut open: Vec<String> = Vec::new();
    for (i, t) in thread.iter().enumerate() {
        let origin = t["origin"].as_str().unwrap_or("");
        let type_ = t["type"].as_str().unwrap_or("");
        let id = t["id"].as_str().unwrap_or("");
        match (origin, type_) {
            ("user", "text") | ("assistant", "text") | ("system", "text") => {}
            ("assistant", "tool_call") => {
                assert!(
                    !id.is_empty(),
                    "{what}: turn {i} is a tool_call without an id -- \
                     `tool_calls[].id` is required on the wire"
                );
                open.push(id.to_string());
            }
            ("tool", "tool_result") => {
                assert!(
                    !id.is_empty(),
                    "{what}: turn {i} renders as {{\"role\":\"tool\",\
                     \"tool_call_id\":\"\"}} -- a tool message that answers no \
                     call is a wire form error and the provider refuses the \
                     whole request (measured: HttpStatus(400), 3 of 3 runs)"
                );
                assert!(
                    open.contains(&id.to_string()),
                    "{what}: turn {i} answers call {id:?}, which no assistant \
                     turn in front of it opened"
                );
            }
            _ => panic!(
                "{what}: turn {i} is origin={origin:?} type={type_:?}, which \
                 `map_turn` has no wire arm for -- it errors as TypeUnsupported"
            ),
        }
    }
}

/// The round the refusal refuses, long since fired.
fn fired_round(iter: i64, call: &str, at_call: &str, at_result: &str) -> Vec<Value> {
    vec![
        row(
            iter,
            "assistant",
            &format!("[{{\"origin\":\"assistant\",\"type\":\"tool_call\",\"id\":\"{call}\"}}]"),
            1,
            at_call,
        ),
        row(
            iter,
            "tool",
            &format!("{{\"origin\":\"tool\",\"type\":\"tool_result\",\"id\":\"{call}\"}}"),
            0,
            at_result,
        ),
    ]
}

#[test]
fn a_repaired_thread_is_a_body_the_wire_accepts() {
    let mut rows = fired_round(
        0,
        "c-1",
        "2999-01-01T10:00:00.000000Z",
        "2999-01-01T10:00:01.000000Z",
    );
    rows.push(parked_receipt_row(
        0,
        "edge_schema",
        0,
        "2999-01-01T10:00:09.000000Z",
    ));

    let out = run_weave(
        json!({"operation": "bundle", "route": "cstore"}),
        json!({"build_id": "b7", "iter": "0", "repairs": "0", "store_origin": "weave"}),
        slate(rows),
    );
    let repair = out
        .as_array()
        .expect("multi-send")
        .iter()
        .find(|m| m["header"]["route"] == "repair")
        .cloned()
        .expect("below the budget the refusal goes back to the composer");
    let thread = repair["messages"]
        .as_array()
        .expect("rebuilt thread")
        .clone();

    assert_wire_legal(&thread, "the repair thread");

    let last = thread.last().expect("a non-empty thread");
    assert!(
        last["text"]
            .as_str()
            .unwrap_or("")
            .contains("the submission was refused: edge_schema"),
        "and the refusal is still NAMED in it -- a refusal the model cannot \
         name is one it cannot repair: {last}"
    );
}

#[test]
fn the_ear_parks_a_refusal_in_a_shape_the_wire_can_carry() {
    // The seam itself: whatever the ear writes into the row is verbatim what
    // `rebuild` hands to the composer later, so the row has to be wire-legal
    // where it is minted, not repaired on the way out.
    let out = run_weave(
        json!({"route": "in_receipt", "error_code": "edge_schema"}),
        json!({"build_id": "b7", "iter": "0", "repairs": "0"}),
        json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "",
                             "text": "from='.' unknown"}]}),
    );
    let bundle = out
        .as_array()
        .expect("multi-send")
        .iter()
        .find(|m| m["header"]["route"] == "cstore")
        .cloned()
        .expect("the ear parks the refusal");
    let op = bundle["messages"]
        .as_array()
        .expect("bundle legs")
        .iter()
        .find(|o| o["id"] == "w-round-row")
        .cloned()
        .expect("the insert leg");
    let insert: Value =
        meclaw_core::serde_json::from_str(op["text"].as_str().unwrap_or("{}")).expect("op json");
    let turn: Value =
        meclaw_core::serde_json::from_str(insert["row"]["turn"].as_str().unwrap_or("{}"))
            .expect("turn json");

    assert_wire_legal(std::slice::from_ref(&turn), "the parked receipt turn");
    assert!(
        turn["text"]
            .as_str()
            .unwrap_or("")
            .contains("the submission was refused: edge_schema"),
        "named where it is written: {turn}"
    );
}

#[test]
fn a_refusal_is_the_newest_turn_of_the_thread_whatever_round_it_was_filed_under() {
    // A receipt arrives on a foreign chain with no context, so the ear files it
    // under iteration 0 no matter how many rounds the build took. The composer
    // must still read it LAST -- a refusal buried between two tool rounds is a
    // refusal of something the thread has not shown yet.
    let mut rows = fired_round(
        0,
        "c-1",
        "2999-01-01T10:00:00.000000Z",
        "2999-01-01T10:00:01.000000Z",
    );
    rows.extend(fired_round(
        1,
        "c-2",
        "2999-01-01T10:00:02.000000Z",
        "2999-01-01T10:00:03.000000Z",
    ));
    rows.push(parked_receipt_row(
        0,
        "edge_schema",
        0,
        "2999-01-01T10:00:09.000000Z",
    ));

    let out = run_weave(
        json!({"operation": "bundle", "route": "cstore"}),
        json!({"build_id": "b7", "iter": "1", "repairs": "0", "store_origin": "weave"}),
        slate(rows),
    );
    let repair = out
        .as_array()
        .expect("multi-send")
        .iter()
        .find(|m| m["header"]["route"] == "repair")
        .cloned()
        .expect("the refusal goes back to the composer");
    let thread = repair["messages"]
        .as_array()
        .expect("rebuilt thread")
        .clone();

    assert_wire_legal(&thread, "the two-round repair thread");
    assert!(
        thread
            .last()
            .and_then(|t| t["text"].as_str())
            .unwrap_or("")
            .contains("the submission was refused"),
        "the refusal is the last turn, not the third of five: {thread:?}"
    );
}
