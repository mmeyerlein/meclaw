//! GH #1435 (D38 K7) -- a turn whose stamp names `memory` waits for the recall leg.
//!
//! WHY this file exists: `route_no_recall` (shipped `["simple"]`) lets a
//! `simple` turn fire without the memory leg -- right for a weather question,
//! wrong for "how is my sister doing?": the memory the answer needs arrives
//! after the call left. The new knob `needs_wait_recall` ("0" shipped) keeps
//! the recall wait point in this one cell: with "1" a stamp that names
//! `memory` in `needs` waits for `leg-memory` even on a path listed in
//! `route_no_recall`. A stamp without `memory` fires as today; the knob off is
//! byte for byte the fan-in of before.
//!
//! Runs the shipped `collector/assemble` script (`support/assemble_cell.rs`).

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::*;
use serde_json::{Value, json};

const TURN: &str = "memory-channel#0001";
/// A window leg recorded "now" for any `route_wait_ms`: no clock in the test.
const FRESH: &str = "2999-01-01T00:00:00.000000Z";

fn knobs(wait_recall: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("memory_tier", json!("1")),
        ("route_wait_ms", json!(150)),
        ("needs_wait_recall", json!(wait_recall)),
    ]
}

fn leg(role: &str, payload: Value) -> Value {
    json!({"turn_id": TURN, "iter": 0, "role": role, "turn": payload.to_string(),
           "fired": 0, "recorded_at": FRESH})
}

fn window() -> Value {
    leg(
        "leg-window",
        json!({"turns": [{"role": "user", "text": "how is my sister doing", "consult_id": ""}],
               "deferred": 0, "deferred_turns": []}),
    )
}

fn memory() -> Value {
    leg("leg-memory", json!({"system": {}, "messages": []}))
}

fn stamp(path: &str, needs: &[&str]) -> Value {
    leg(
        "leg-route",
        json!({"path": path, "needs": needs, "families": []}),
    )
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

/// Knob on: a `simple` turn that needs memory waits for the recall leg and
/// then carries path and needs on the seam; one that needs no memory fires as
/// today, without the leg.
#[test]
fn a_simple_turn_that_needs_memory_waits_for_the_recall_leg() {
    let out = collect(&knobs("1"), json!([window(), stamp("simple", &["memory"])]));
    assert!(
        seam(&out).is_none(),
        "simple + memory waits for the recall leg: {out:?}"
    );
    let out = collect(
        &knobs("1"),
        json!([window(), stamp("simple", &["memory"]), memory()]),
    );
    let s = seam(&out).unwrap_or_else(|| panic!("all legs stand: {out:?}"));
    assert_eq!(hop_str(s, "path"), "simple");
    assert_eq!(
        hop_str(s, "needs"),
        "memory",
        "the need rides the seam: {s}"
    );

    // `memory` beside `tool`: the wait is the same.
    let out = collect(
        &knobs("1"),
        json!([window(), stamp("simple", &["memory", "tool"])]),
    );
    assert!(
        seam(&out).is_none(),
        "memory among other needs waits: {out:?}"
    );
}

#[test]
fn a_simple_turn_without_the_memory_need_fires_as_today_with_the_knob_on() {
    let out = collect(&knobs("1"), json!([window(), stamp("simple", &["tool"])]));
    let s = seam(&out)
        .unwrap_or_else(|| panic!("simple without memory waits for nothing more: {out:?}"));
    assert_eq!(hop_str(s, "path"), "simple");
    assert_eq!(hop_str(s, "needs"), "tool");
    let out = collect(&knobs("1"), json!([window(), stamp("simple", &[])]));
    assert!(seam(&out).is_some(), "no needs, no wait: {out:?}");
    // A path outside `route_no_recall` waits as it always did.
    let out = collect(&knobs("1"), json!([window(), stamp("complex", &[])]));
    assert!(
        seam(&out).is_none(),
        "complex waits for the memory: {out:?}"
    );
}

/// The knob ships "0" in params, setting and script fallback, and "0" is the
/// fan-in of before: `simple` fires without the memory leg, `memory` or not.
#[test]
fn the_knob_ships_off_and_off_is_byte_identical() {
    let cfg = config_of(ASSEMBLE);
    assert_eq!(
        cfg["params"]["needs_wait_recall"],
        json!("0"),
        "{}",
        cfg["params"]
    );
    assert_eq!(
        cfg["contract"]["settings"]["needs_wait_recall"]["default"],
        json!("0"),
        "the setting says the same"
    );
    let src = cfg["params"]["script_inline"].as_str().expect("script");
    assert!(
        src.contains("_str(\"needs_wait_recall\", \"0\")"),
        "the script's fallback"
    );
    let out = collect(&knobs("0"), json!([window(), stamp("simple", &["memory"])]));
    let s = seam(&out).unwrap_or_else(|| panic!("off: simple fires without the memory: {out:?}"));
    assert_eq!(hop_str(s, "path"), "simple");
    assert_eq!(
        hop_str(s, "needs"),
        "memory",
        "the need still rides the seam: {s}"
    );
}

/// Review M-2: a stamp may carry `needs` as a comma string; the collector
/// reads it as the policy does (`names_list`: blanks trimmed), so "tool,
/// memory" waits here as the policy sends it to the brain -- never a brain
/// call without the recall.
#[test]
fn a_needs_string_is_read_with_blanks_trimmed() {
    let out = collect(
        &knobs("1"),
        json!([
            window(),
            leg(
                "leg-route",
                json!({"path": "simple", "needs": "tool, memory",
                                                   "families": []})
            )
        ]),
    );
    assert!(
        seam(&out).is_none(),
        "\"tool, memory\" waits for the recall leg: {out:?}"
    );
}
