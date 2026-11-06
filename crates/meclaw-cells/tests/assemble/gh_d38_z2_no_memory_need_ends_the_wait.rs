//! GH #1441 (D38 Z2) -- a stamp that needs no memory ends the wait for the recall leg.
//!
//! WHY this file exists: RK-38-7 (adaptive retrieval) -- the memory is looked
//! up when the turn needs it, not on every turn. `route_no_recall` (shipped
//! `["simple"]`) frees only a PATH from the recall leg; a `talk` or `complex`
//! turn the route judged to need nothing of the past still waits for the
//! memory leg. The new knob `needs_no_recall` ("0" shipped) keeps the recall
//! wait point in this one cell: with "1" a stamp whose `needs` do not name
//! `memory` ends the wait for `leg-memory`, whatever its path. A stamp that
//! names `memory` waits as before, and a fallback stamp (the stamp's
//! `fallback`, e.g. `unsure`: no judgement of the needs) waits too -- the
//! curator folds the recall of such a `talk` turn (A11), so the recall has to
//! be there. The ask itself is not touched: it leaves when the turn opens.
//! The knob off is byte for byte the fan-in of before.
//!
//! Runs the shipped `collector/assemble` script (`support/assemble_cell.rs`).

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::*;
use serde_json::{Value, json};

const TURN: &str = "z2-channel#0001";
/// A window leg recorded "now" for any `route_wait_ms`: no clock in the test.
const FRESH: &str = "2999-01-01T00:00:00.000000Z";

fn knobs(no_recall: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("memory_tier", json!("1")),
        ("route_wait_ms", json!(150)),
        ("needs_no_recall", json!(no_recall)),
    ]
}

fn leg(role: &str, payload: Value) -> Value {
    json!({"turn_id": TURN, "iter": 0, "role": role, "turn": payload.to_string(),
           "fired": 0, "recorded_at": FRESH})
}

fn window() -> Value {
    leg(
        "leg-window",
        json!({"turns": [{"role": "user", "text": "I just got back from the lake",
                          "consult_id": ""}],
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

/// A stamp the route cell sent on its fallback (`hop.fallback` of the stamp,
/// parked with the leg): an unsure path carries no needs.
fn fallback_stamp(path: &str, why: &str) -> Value {
    leg(
        "leg-route",
        json!({"path": path, "needs": [], "families": [], "fallback": why}),
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

/// Knob on: a stamped turn whose needs do not name `memory` fires without the
/// recall leg, on every path -- `talk` and `complex` included.
#[test]
fn a_stamp_without_the_memory_need_fires_without_the_recall_leg() {
    for (path, needs) in [
        ("talk", &[][..]),
        ("complex", &[][..]),
        ("talk", &["tool"][..]),
        ("complex", &["tool"][..]),
    ] {
        let out = collect(&knobs("1"), json!([window(), stamp(path, needs)]));
        let s = seam(&out).unwrap_or_else(|| {
            panic!("{path} {needs:?} needs no memory and waits for nothing more: {out:?}")
        });
        assert_eq!(hop_str(s, "path"), path, "{s}");
        assert_eq!(hop_str(s, "needs"), needs.join(","), "{s}");
    }
}

/// Knob on: a stamp that names `memory` waits for the recall leg as before,
/// on any path, and fires once the leg stands.
#[test]
fn a_stamp_that_needs_memory_still_waits_for_the_recall_leg() {
    let out = collect(&knobs("1"), json!([window(), stamp("talk", &["memory"])]));
    assert!(
        seam(&out).is_none(),
        "talk + memory waits for the recall leg: {out:?}"
    );
    let out = collect(
        &knobs("1"),
        json!([window(), stamp("complex", &["tool", "memory"])]),
    );
    assert!(
        seam(&out).is_none(),
        "memory among other needs waits: {out:?}"
    );
    let out = collect(
        &knobs("1"),
        json!([window(), stamp("talk", &["memory"]), memory()]),
    );
    let s = seam(&out).unwrap_or_else(|| panic!("all legs stand: {out:?}"));
    assert_eq!(hop_str(s, "needs"), "memory", "{s}");
}

/// Knob on: a fallback stamp is no judgement of the needs -- it waits for the
/// recall leg, and the seam says which fallback it was (`hop.fallback`), so
/// the curator can fold the recall of an unsure `talk` turn (A11). The seam
/// carries the key only beside the knob: off, the seam is byte for byte the
/// seam of before (OR-d38-2.o3.2).
#[test]
fn a_fallback_stamp_still_waits_and_the_seam_names_the_fallback() {
    let out = collect(
        &knobs("1"),
        json!([window(), fallback_stamp("talk", "unsure")]),
    );
    assert!(
        seam(&out).is_none(),
        "an unsure stamp waits for the recall leg: {out:?}"
    );
    let out = collect(
        &knobs("1"),
        json!([window(), fallback_stamp("talk", "unsure"), memory()]),
    );
    let s = seam(&out).unwrap_or_else(|| panic!("all legs stand: {out:?}"));
    assert_eq!(hop_str(s, "path"), "talk", "{s}");
    assert_eq!(
        hop_str(s, "fallback"),
        "unsure",
        "the stamp's fallback rides the seam: {s}"
    );
    // Knob off (OR-d38-2.o3.2): the seam of a fallback stamp is the seam of
    // before -- no `fallback` key; the key rides only beside `needs_no_recall`.
    let out = collect(
        &knobs("0"),
        json!([window(), fallback_stamp("talk", "unsure"), memory()]),
    );
    let s = seam(&out).unwrap_or_else(|| panic!("off: all legs stand: {out:?}"));
    assert!(
        s["header"].get("fallback").is_none(),
        "off: no fallback on the seam: {}",
        s["header"]
    );
    // A sure stamp carries no fallback key on the seam (absent unless set).
    let out = collect(&knobs("1"), json!([window(), stamp("talk", &[])]));
    let s = seam(&out).unwrap_or_else(|| panic!("a sure talk stamp fires: {out:?}"));
    assert!(
        s["header"].get("fallback").is_none(),
        "no fallback, no key: {}",
        s["header"]
    );
}

/// `in_route` parks the stamp's `hop.fallback` with the leg, so the fan-in
/// can read it; a stamp without one, or the knob off, parks the keys of before.
#[test]
fn the_route_lane_parks_the_stamps_fallback_with_the_leg() {
    fn stamped(hop: Value) -> Value {
        let mut doc = lane("in_route", hop, json!({"path": "talk"}), json!([]));
        doc["stamp"] = json!({"turn_id": TURN, "path": "talk", "p": 0.0, "intent": "",
                              "needs": [], "families": [], "duration": 80});
        doc
    }
    fn parked(out: &[Value]) -> Value {
        let row = calls_of(in_phase(out, "collect"))
            .into_iter()
            .filter(|(_, a)| a["operation"] == "insert")
            .map(|(_, a)| a["row"].clone())
            .next()
            .unwrap_or_else(|| panic!("in_route parked no row: {out:?}"));
        serde_json::from_str(row["turn"].as_str().expect("turn")).expect("json")
    }
    let p = parked(&assemble(
        &knobs("1"),
        stamped(json!({"turn_id": TURN, "path": "talk", "fallback": "unsure"})),
    ));
    assert_eq!(p["fallback"], json!("unsure"), "{p}");
    let p = parked(&assemble(
        &knobs("1"),
        stamped(json!({"turn_id": TURN, "path": "talk"})),
    ));
    assert!(p.get("fallback").is_none(), "no fallback, no key: {p}");
    // Knob off (OR-d38-2.o3.2): the parked leg is the leg of before.
    let p = parked(&assemble(
        &knobs("0"),
        stamped(json!({"turn_id": TURN, "path": "talk", "fallback": "unsure"})),
    ));
    assert!(p.get("fallback").is_none(), "off: no fallback parked: {p}");
}

/// The knob ships "0" in params, setting and script fallback, and "0" is the
/// fan-in of before: a `talk` turn without the memory need still waits.
#[test]
fn the_knob_ships_off_and_off_is_byte_identical() {
    let cfg = config_of(ASSEMBLE);
    assert_eq!(
        cfg["params"]["needs_no_recall"],
        json!("0"),
        "{}",
        cfg["params"]
    );
    assert_eq!(
        cfg["contract"]["settings"]["needs_no_recall"]["default"],
        json!("0"),
        "the setting says the same"
    );
    let src = cfg["params"]["script_inline"].as_str().expect("script");
    assert!(
        src.contains("_str(\"needs_no_recall\", \"0\")"),
        "the script's fallback"
    );
    let out = collect(&knobs("0"), json!([window(), stamp("talk", &[])]));
    assert!(
        seam(&out).is_none(),
        "off: talk waits for the memory as before: {out:?}"
    );
    let out = collect(&knobs("0"), json!([window(), stamp("simple", &[])]));
    assert!(
        seam(&out).is_some(),
        "off: route_no_recall still frees simple: {out:?}"
    );
}
