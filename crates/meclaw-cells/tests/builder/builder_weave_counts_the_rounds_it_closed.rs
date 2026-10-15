//! `hop.iter` is a COORDINATE, not a count, and reading it as one cost the
//! acceptance run a whole line of its quota.
//!
//! Task F1 measured "the loop really looked" as `hop.iter` of `./weave` > 0. In
//! all four runs the number was 0, and in all four runs the loop had looked --
//! the composer called tools, `lib` and `eyes` answered, `weave` closed the
//! round and fired over the re-entry edge
//! (the builder agentic-loop run series, the
//! acceptance table, line 4). The number is off by one because it is not the number it was read as:
//!
//!   * `weave` stamps the iteration it FINDS (`ctx.get("iter")`) -- the round it
//!     just closed, counted from zero;
//!   * the increment happens on the re-entry edge `./weave -> ./compose`
//!     (`set_context: {"iter": "int(context.iter) + 1"}`).
//!
//! So `hop.iter > 0` at `weave` says "the loop looked TWICE", and one tool round
//! followed by a written manifest is indistinguishable from never having looked.
//!
//! The ruling (orchestrator, 2026-08-27): `iter` KEEPS its meaning. It is
//! load-bearing in three places -- the slate is keyed on `(build_id, iter)`, the
//! repair edge restores `context.iter` from `hop.iter`, and the shipped scenario
//! `A1-an-eye-fixes-the-island.json` asserts `hop.iter == "1"` on it. Renaming
//! what it counts would move all three. Instead the round-closing routes stamp a
//! SECOND key that is a count and reads like one, and the evaluation reads that:
//! `hop.rounds_done` -- how many tool rounds this build has closed, this one
//! included. It exists on `fire` and `draft` and nowhere else, because those are
//! the only two emissions that close a round: `repair` hands a refusal back
//! without a round having happened, and `give_up` and `cstore` close nothing.

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

/// One complete round at iteration `it`, nothing claimed yet.
fn closed_round(it: i64) -> Vec<Value> {
    vec![
        row(
            it,
            "assistant",
            "[{\"origin\":\"assistant\",\"type\":\"tool_call\",\"id\":\"c-1\"}]",
            0,
            "2999-01-01T10:00:00.000000Z",
        ),
        row(
            it,
            "tool",
            "{\"origin\":\"tool\",\"type\":\"tool_result\",\"id\":\"c-1\"}",
            0,
            "2999-01-01T10:00:01.000000Z",
        ),
    ]
}

fn closing(it: i64) -> Value {
    run_weave(
        json!({"operation": "bundle", "route": "cstore"}),
        json!({"build_id": "b7", "iter": it.to_string(), "store_origin": "weave"}),
        slate(closed_round(it)),
    )
    .as_array()
    .expect("multi-send")
    .iter()
    .find(|m| m["header"]["route"] == "fire" || m["header"]["route"] == "draft")
    .cloned()
    .expect("a complete round leaves on fire, or on draft at the cap")
}

#[test]
fn the_first_closed_round_says_one_while_the_coordinate_still_says_zero() {
    let out = closing(0);
    assert_eq!(
        out["header"]["rounds_done"], "1",
        "one round closed is one, and it is readable as one -- this is the \
         number F1 wanted and `iter` could not give it"
    );
    assert_eq!(
        out["header"]["iter"], "0",
        "and the coordinate is UNTOUCHED: the slate is keyed on it, the repair \
         edge restores context.iter from it, and A1-an-eye-fixes-the-island \
         asserts it"
    );
}

#[test]
fn the_count_follows_the_rounds_and_not_the_other_way_round() {
    assert_eq!(closing(1)["header"]["rounds_done"], "2");
    assert_eq!(closing(1)["header"]["iter"], "1");
}

#[test]
fn the_capped_round_is_counted_too() {
    // At the cap the thread leaves for `normalise` on `draft` instead of asking
    // the composer again. It is still a round that was closed, and a metric
    // that stopped counting there would report a capped build as having looked
    // one time less than it did.
    let out = closing(6);
    assert_eq!(out["header"]["route"], "draft");
    assert_eq!(out["header"]["round_capped"], "1");
    assert_eq!(
        out["header"]["rounds_done"], "7",
        "iterations 0..6 inclusive -- the honest number, cap semantics and all"
    );
}

#[test]
fn a_hand_back_is_not_a_round_and_does_not_claim_one() {
    // A refusal can reach the composer without a single tool round: the model
    // answers straight away, `normalise` mints a digest, the submitter refuses.
    // If `repair` carried a round count it would count a round that never ran.
    let mut rows = vec![
        row(
            0,
            "assistant",
            "[{\"origin\":\"assistant\",\"type\":\"tool_call\",\"id\":\"c-1\"}]",
            1,
            "2999-01-01T10:00:00.000000Z",
        ),
        row(
            0,
            "tool",
            "{\"origin\":\"tool\",\"type\":\"tool_result\",\"id\":\"c-1\"}",
            0,
            "2999-01-01T10:00:01.000000Z",
        ),
    ];
    let turn = json!({"origin": "user", "type": "text",
                      "text": "the submission was refused: edge_schema -- because"});
    rows.push(row(
        0,
        "receipt",
        &meclaw_core::serde_json::to_string(&turn).expect("turn"),
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
        .expect("the refusal goes back to the composer");
    assert!(
        repair["header"].get("rounds_done").is_none(),
        "only the two round-closing routes carry the count: {}",
        repair["header"]
    );
}

#[test]
fn the_count_is_declared_where_a_reader_of_the_contract_would_look() {
    let cfg: Value =
        meclaw_core::serde_json::from_str(&std::fs::read_to_string(WEAVE).expect("weave config"))
            .expect("parses");
    let hop = &cfg["contract"]["emits"]["hop"];
    assert_eq!(
        hop["rounds_done"]["type"], "string",
        "an emitted hop key that is not declared is a key no consumer may rely \
         on -- and this one is what an evaluation reads"
    );
    assert_eq!(
        hop["rounds_done"]["required"], false,
        "it rides on fire and draft only, so it cannot be required"
    );
}
