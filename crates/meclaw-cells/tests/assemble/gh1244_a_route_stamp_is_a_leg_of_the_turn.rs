//! GH #1244 (T4-05): the collector takes the route stamp on `in_route`.
//!
//! The stamp names the path a route cell chose for the turn
//! (`talk|simple|complex|research`). While `route_wait_ms` is above 0 it is a
//! leg of the turn like the memory leg, and it narrows what the fan-in waits
//! for: a path in `route_no_recall` (shipped: `simple`) does not wait for the
//! memory leg. No stamp yet and a window leg older than `route_wait_ms`: the
//! turn stops waiting for it. The path rides on every seam as `hop.path`.
//! The stamp is the body slot `stamp` (`{turn_id, path, p, intent, needs,
//! families, duration}`), `hop.turn_id`/`hop.path` beside it (the route cell).
//! 0 is off: no route leg, the fan-in of before.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::*;
use serde_json::{Value, json};

const TURN: &str = "route-channel#0001";
/// A window leg recorded "now" for any `route_wait_ms`: no clock in the test.
const FRESH: &str = "2999-01-01T00:00:00.000000Z";
/// A window leg older than any `route_wait_ms`.
const OLD: &str = "2000-01-01T00:00:00.000000Z";

fn knob(wait: i64) -> Vec<(&'static str, Value)> {
    vec![("memory_tier", json!("1")), ("route_wait_ms", json!(wait))]
}

fn leg(role: &str, payload: Value, recorded_at: &str) -> Value {
    json!({"turn_id": TURN, "iter": 0, "role": role, "turn": payload.to_string(),
           "fired": 0, "recorded_at": recorded_at})
}

fn window(recorded_at: &str) -> Value {
    leg(
        "leg-window",
        json!({"turns": [{"role": "user", "text": "how is the weather tomorrow",
                          "consult_id": ""}],
               "deferred": 0, "deferred_turns": []}),
        recorded_at,
    )
}

fn memory() -> Value {
    leg("leg-memory", json!({"system": {}, "messages": []}), FRESH)
}

fn route(path: &str) -> Value {
    leg("leg-route", json!({"path": path}), FRESH)
}

fn collect(over: &[(&str, Value)], rows: Value) -> Vec<Value> {
    assemble(
        over,
        bundle_reply("collect", TURN, &[("c-collect-read", rows)]),
    )
}

fn seam(out: &[Value]) -> Option<&Value> {
    out.iter().find(|m| {
        let r = m["header"]["route"].as_str();
        r == Some("curate") || r == Some("answer")
    })
}

/// What a route cell sends on `in_route`: ONE body slot `stamp` with the seven
/// keys, `hop.turn_id` and `hop.path` beside it, no turn id in context.
fn stamped(stamp: Value, hop_path: &str) -> Value {
    let mut doc = lane(
        "in_route",
        json!({"turn_id": TURN, "path": hop_path}),
        json!({"path": "talk"}),
        json!([]),
    );
    doc["stamp"] = stamp;
    doc
}

fn parked_stamp(out: &[Value]) -> (Value, Value) {
    let row = calls_of(in_phase(out, "collect"))
        .into_iter()
        .filter(|(_, a)| a["operation"] == "insert")
        .map(|(_, a)| a["row"].clone())
        .next()
        .unwrap_or_else(|| panic!("in_route parked no row: {out:?}"));
    let stamp: Value = serde_json::from_str(row["turn"].as_str().expect("turn")).expect("json");
    (row, stamp)
}

#[test]
fn a_route_stamp_is_a_leg_and_a_simple_turn_does_not_wait_for_the_memory() {
    // The lane parks the body slot `stamp` under the hop's turn id; a
    // `context.path` riding along is not the stamp.
    let (row, stamp) = parked_stamp(&assemble(
        &knob(150),
        stamped(
            json!({"turn_id": TURN, "path": "simple", "p": 0.91, "intent": "ask",
                   "needs": [], "families": ["web"], "duration": 120}),
            "simple",
        ),
    ));
    assert_eq!(row["role"], "leg-route", "{row}");
    assert_eq!(
        row["turn_id"], TURN,
        "the hop's turn id keys the leg: {row}"
    );
    assert_eq!(stamp["path"], "simple", "the stamp's path: {stamp}");
    assert_eq!(stamp["families"], json!(["web"]), "{stamp}");
    assert_eq!(stamp["p"], json!(0.91), "{stamp}");
    assert_eq!(stamp["duration"], json!(120), "{stamp}");
    // `hop.path` stands in for a stamp without a path.
    let (_, stamp) = parked_stamp(&assemble(
        &knob(150),
        stamped(json!({"turn_id": TURN, "p": 0.7}), "complex"),
    ));
    assert_eq!(
        stamp["path"], "complex",
        "hop.path is the same value: {stamp}"
    );

    // simple: window and stamp fire the turn, the memory leg is not waited for.
    let stamp = leg(
        "leg-route",
        json!({"path": "simple", "families": ["web", "history"]}),
        FRESH,
    );
    let out = collect(&knob(150), json!([window(FRESH), stamp]));
    let s = seam(&out).unwrap_or_else(|| panic!("simple waits for nothing more: {out:?}"));
    assert_eq!(hop_str(s, "path"), "simple");
    // orch-8 (K-ROUTE OR .8): the stamp's families ride the seam beside the
    // path, a comma list in stamp order -- the curator's quick tool_scope.
    assert_eq!(hop_str(s, "families"), "web,history");

    // complex waits for the memory leg, then carries its path.
    let out = collect(&knob(150), json!([window(FRESH), route("complex")]));
    assert!(
        seam(&out).is_none(),
        "complex waits for the memory: {out:?}"
    );
    let out = collect(
        &knob(150),
        json!([window(FRESH), route("complex"), memory()]),
    );
    let s = seam(&out).unwrap_or_else(|| panic!("all legs stand: {out:?}"));
    assert_eq!(hop_str(s, "path"), "complex");

    // No stamp: a fresh turn waits for it, one older than route_wait_ms does not.
    let out = collect(&knob(150), json!([window(FRESH), memory()]));
    assert!(
        seam(&out).is_none(),
        "the turn waits for its stamp: {out:?}"
    );
    let out = collect(&knob(150), json!([window(OLD), memory()]));
    let s = seam(&out).unwrap_or_else(|| panic!("the wait is over: {out:?}"));
    assert_eq!(hop_str(s, "path"), "", "no stamp, no path");

    // 0 is off: no route leg, and a stamp is not parked.
    let out = collect(&knob(0), json!([window(FRESH), memory()]));
    assert!(seam(&out).is_some(), "off fans in as before: {out:?}");
    let off = assemble(
        &knob(0),
        stamped(json!({"turn_id": TURN, "path": "simple"}), "simple"),
    );
    assert!(off.is_empty(), "off parks no stamp: {off:?}");

    // The knobs ship off and say so in all three places.
    let cfg = config_of(ASSEMBLE);
    assert_eq!(cfg["params"]["route_wait_ms"], json!(0));
    assert_eq!(
        cfg["contract"]["settings"]["route_wait_ms"]["default"],
        json!(0)
    );
    assert_eq!(cfg["params"]["route_no_recall"], json!(["simple"]));
    assert_eq!(
        cfg["contract"]["settings"]["route_no_recall"]["default"],
        json!(["simple"])
    );
    let src = cfg["params"]["script_inline"].as_str().expect("script");
    assert!(
        src.contains("_int(\"route_wait_ms\", 0)"),
        "the script's fallback"
    );
    assert!(
        src.contains("_list(\"route_no_recall\", [\"simple\"])"),
        "the script's fallback"
    );
}

/// Re-check I1: a round an EVENT opened (`in_advice`, `in_delegation`) is keyed
/// by the collector itself and no route cell stamps it, so with `route_wait_ms`
/// above 0 its fan-in never waits for the stamp -- the timer-less escape needs a
/// later leg, and none comes.
#[test]
fn an_event_round_never_waits_for_a_route_stamp() {
    for role in ["advice", "delegation"] {
        let win = leg(
            "leg-window",
            json!({"turns": [{"role": role, "text": "the advisor is back",
                              "consult_id": "c-1"}],
                   "deferred": 0, "deferred_turns": []}),
            FRESH,
        );
        let out = collect(&knob(150), json!([win, memory()]));
        let s = seam(&out).unwrap_or_else(|| panic!("a {role} round waits for no stamp: {out:?}"));
        assert_eq!(hop_str(s, "path"), "", "no stamp, no path");
    }
}
