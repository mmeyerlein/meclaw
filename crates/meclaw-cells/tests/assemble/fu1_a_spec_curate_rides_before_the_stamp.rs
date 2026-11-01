//! GH #1275: the speculative curate of a turn (`quick_speculative`).
//!
//! Speculation is the SAME curate of the same turn, earlier: while the turn
//! waits for its route stamp (`route_wait_ms` above 0), the collector sends one
//! extra `curate` as soon as every expected leg but the stamp and the memory
//! stands -- `hop.spec` "1", no path. The curator may start the quick call on
//! it and keep or drop the answer when the stamp's seam arrives. Fire-once is a
//! store guard: the cell inserts a `spec-fired` row `if_absent` and reads the
//! round back in ONE bundle, and only the reply that inserted it sends the
//! curate. The normal seam after the stamp leaves exactly as before.
//! "0" (shipped) changes nothing.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::*;
use serde_json::{Value, json};

const TURN: &str = "spec-channel#0001";
/// A window leg recorded "now" for any `route_wait_ms`: no clock in the test.
const FRESH: &str = "2999-01-01T00:00:00.000000Z";

fn knob(spec: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("memory_tier", json!("1")),
        ("route_wait_ms", json!(150)),
        ("quick_speculative", json!(spec)),
    ]
}

/// The same knobs without the speculation param: what a collector ran before.
fn today() -> Vec<(&'static str, Value)> {
    vec![("memory_tier", json!("1")), ("route_wait_ms", json!(150))]
}

fn leg(role: &str, payload: Value) -> Value {
    json!({"turn_id": TURN, "iter": 0, "role": role, "turn": payload.to_string(),
           "fired": 0, "recorded_at": FRESH})
}

fn window_of(role: &str) -> Value {
    leg(
        "leg-window",
        json!({"turns": [{"role": role, "text": "how is the weather tomorrow",
                          "consult_id": ""}],
               "deferred": 0, "deferred_turns": []}),
    )
}

fn window() -> Value {
    window_of("user")
}

fn memory() -> Value {
    leg("leg-memory", json!({"system": {}, "messages": []}))
}

fn route(path: &str) -> Value {
    leg(
        "leg-route",
        json!({"path": path, "families": ["web"], "needs": []}),
    )
}

fn spec_mark() -> Value {
    let mut r = leg("spec-fired", json!({}));
    r["fired"] = json!(1);
    r
}

fn collect(over: &[(&str, Value)], rows: Value) -> Vec<Value> {
    assemble(
        over,
        bundle_reply("collect", TURN, &[("c-collect-read", rows)]),
    )
}

/// The reply of the `spec-fire` bundle: the guarded insert, then the read-back.
/// `won` is what the guard did -- 1 inserted the mark, 0 found one standing.
fn spec_reply(rows: Value, won: i64) -> Value {
    let mut doc = bundle_reply(
        "spec-fire",
        TURN,
        &[("c-spec-mark", Value::Null), ("c-spec-read", rows)],
    );
    doc["results"][0]["rows_affected"] = json!(won);
    doc
}

fn seams(out: &[Value]) -> Vec<&Value> {
    out.iter()
        .filter(|m| {
            let r = m["header"]["route"].as_str();
            r == Some("curate") || r == Some("answer")
        })
        .collect()
}

fn spec_fire(out: &[Value]) -> Option<&Value> {
    out.iter().find(|m| {
        m["header"]["route"].as_str() == Some("cstore")
            && m["header"]["phase"].as_str() == Some("spec-fire")
    })
}

#[test]
fn spec_curate_fires_once_before_the_stamp() {
    // The window stands, the stamp and the memory do not: the turn does not
    // fire, the speculation asks the store for its one mark.
    let out = collect(&knob("1"), json!([window()]));
    assert!(
        seams(&out).is_empty(),
        "the turn itself still waits: {out:?}"
    );
    let fire = spec_fire(&out).unwrap_or_else(|| panic!("no spec-fire bundle: {out:?}"));
    let mark = call(fire, "c-spec-mark");
    assert_eq!(mark["operation"], "insert", "{mark}");
    assert_eq!(mark["row"]["role"], "spec-fired", "{mark}");
    assert_eq!(mark["row"]["turn_id"], TURN, "{mark}");
    assert_eq!(
        mark["if_absent"],
        json!({"turn_id": TURN, "role": "spec-fired"}),
        "the store elects the one speculation: {mark}"
    );
    let read = call(fire, "c-spec-read");
    assert_eq!(read["where"], json!({"turn_id": TURN}), "{read}");

    // The reply that inserted the mark sends the spec curate: the window leg,
    // `spec` "1", no path -- and nothing else (no bookkeeping, no fired mark).
    let out = assemble(&knob("1"), spec_reply(json!([window(), spec_mark()]), 1));
    assert_eq!(out.len(), 1, "the spec curate alone: {out:?}");
    let s = &out[0];
    assert_eq!(s["header"]["route"], "curate", "{s}");
    assert_eq!(hop_str(s, "spec"), "1");
    assert_eq!(hop_str(s, "path"), "", "no stamp, no path");
    assert_eq!(hop_str(s, "iter"), "0");
    assert_eq!(hop_str(s, "turn_id"), TURN);
    assert!(
        s["messages"]
            .to_string()
            .contains("how is the weather tomorrow"),
        "the window leg rides the spec curate: {s}"
    );

    // The guard found a mark standing: no second spec curate.
    let out = assemble(&knob("1"), spec_reply(json!([window(), spec_mark()]), 0));
    assert!(out.is_empty(), "the lost election sends nothing: {out:?}");
    // The stamp came first: the read sees it, the normal seam is the curate.
    let out = assemble(
        &knob("1"),
        spec_reply(json!([window(), spec_mark(), route("simple")]), 1),
    );
    assert!(
        out.is_empty(),
        "after the stamp nothing speculates: {out:?}"
    );

    // The memory leg lands after the spec curate, before the stamp: no second
    // spec-fire, and the turn still waits for its stamp.
    let out = collect(&knob("1"), json!([window(), spec_mark(), memory()]));
    assert!(out.is_empty(), "one speculation per turn: {out:?}");

    // The stamp lands: the normal seam leaves with its path, byte for byte the
    // seam of a collector without the param (the mark row is invisible to it).
    let on = collect(
        &knob("1"),
        json!([window(), spec_mark(), memory(), route("simple")]),
    );
    let before = collect(&today(), json!([window(), memory(), route("simple")]));
    assert_eq!(seams(&on).len(), 1, "{on:?}");
    assert!(spec_fire(&on).is_none(), "{on:?}");
    assert_eq!(hop_str(seams(&on)[0], "path"), "simple");
    assert!(
        seams(&on)[0]["header"].get("spec").is_none(),
        "the normal seam carries no spec key"
    );
    assert_eq!(on, before, "the normal seam is today's");
    let on = collect(&knob("1"), json!([window(), spec_mark(), route("simple")]));
    let before = collect(&today(), json!([window(), route("simple")]));
    assert_eq!(on, before, "simple waits for no memory, as today");
}

#[test]
fn spec_never_fires_for_an_event_round() {
    // A round `in_advice` or `in_delegation` opened waits for no stamp and
    // fires on its own legs; it never speculates.
    for role in ["advice", "delegation"] {
        let out = collect(&knob("1"), json!([window_of(role)]));
        assert!(
            spec_fire(&out).is_none() && seams(&out).is_empty(),
            "a {role} round does not speculate: {out:?}"
        );
        let out = assemble(&knob("1"), spec_reply(json!([window_of(role)]), 1));
        assert!(
            out.is_empty(),
            "a {role} round sends no spec curate: {out:?}"
        );
    }
    // Nor without a route wait: no stamp to run ahead of.
    let mut off = knob("1");
    off[1] = ("route_wait_ms", json!(0));
    let out = collect(&off, json!([window()]));
    assert!(spec_fire(&out).is_none(), "{out:?}");
}

#[test]
fn spec_off_is_byte_identical() {
    // Default "0": the outputs of a turn's fan-in are today's bytes, with the
    // param named "0" and with the param absent.
    let docs = [
        bundle_reply("collect", TURN, &[("c-collect-read", json!([window()]))]),
        bundle_reply(
            "collect",
            TURN,
            &[("c-collect-read", json!([window(), memory()]))],
        ),
        bundle_reply(
            "collect",
            TURN,
            &[("c-collect-read", json!([window(), route("simple")]))],
        ),
        bundle_reply(
            "collect",
            TURN,
            &[(
                "c-collect-read",
                json!([window(), memory(), route("complex")]),
            )],
        ),
        spec_reply(json!([window()]), 1),
    ];
    for doc in docs {
        let off = run_cell_stdout(ASSEMBLE, &knob("0"), doc.clone()).0;
        let base = run_cell_stdout(ASSEMBLE, &today(), doc.clone()).0;
        assert_eq!(off, base, "off is today's bytes for {doc}");
    }
    // The window alone waits, as today: nothing leaves.
    let out = collect(&knob("0"), json!([window()]));
    assert!(out.is_empty(), "off speculates nothing: {out:?}");

    // The knob ships off and says so in all three places.
    let cfg = config_of(ASSEMBLE);
    assert_eq!(cfg["params"]["quick_speculative"], json!("0"));
    assert_eq!(
        cfg["contract"]["settings"]["quick_speculative"]["default"],
        json!("0")
    );
    let src = cfg["params"]["script_inline"].as_str().expect("script");
    assert!(
        src.contains("_str(\"quick_speculative\", \"0\")"),
        "the script's fallback"
    );
}

/// live-2 #1370: `quick_speculative` set as JSON `true` (the set's form) is on
/// here as in the curator's policy, which reads the same switch.
#[test]
fn quick_speculative_reads_json_true() {
    let on = vec![
        ("memory_tier", json!("1")),
        ("route_wait_ms", json!(150)),
        ("quick_speculative", json!(true)),
    ];
    let out = collect(&on, json!([window()]));
    assert!(spec_fire(&out).is_some(), "true is on: {out:?}");
}
