//! GH #1466 (D58 recall) -- the route stamp can carry the yes-values of its needs.
//!
//! WHY this file exists: the stamp says which needs the decider answered yes
//! at `yes_threshold`, and nothing about the ones below it. For "this turn
//! surely needs no memory" that is not enough: a yes of 0.45 and a yes of 0.02
//! both leave `needs` empty, and the stamp's `p` is the confidence of the
//! path, not of a need. The curator's rule `recall_skip_sure` reads the
//! yes-value of the memory question itself. With `stamp_needs_yes` "1" a
//! JUDGED stamp carries `needs_yes`, `{<need>: yes}` for every asked need the
//! decider answered with a number. A need it did not answer is absent, never
//! 0 -- silence is not a "no". A fallback stamp (`error`, `late`, `unsure`)
//! carries no `needs_yes` at all: no judgement stands behind it. Shipped "0",
//! the stamp keeps the keys of before, byte for byte.
//!
//! Runs the shipped `talky/route` script (`support/assemble_cell.rs`).

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::*;
use serde_json::{Value, json};

const ROUTE: &str = "templates/talky/route/config.json";

/// A set's object: the three shipped entries plus `memory`.
fn four_needs() -> Value {
    json!({"tool": "a tool call",
           "reasoning": "careful step-by-step reasoning",
           "research": "a deep search over many sources",
           "memory": "something the person said earlier or before today"})
}

/// The decider's answer for turn `c-1`, asked `ago_ms` before now (negative:
/// asked ahead, so the duration clamps to 0 and the answer is never late).
fn decision(ago_ms: i64, hop: Value, answers: Value) -> Value {
    let mut h = json!({"finish_reason": "stop", "asked_at": (now_ms() - ago_ms).to_string()});
    for (k, v) in hop.as_object().expect("hop object") {
        h[k] = v.clone();
    }
    let mut doc = lane("in_decision", h, json!({"turn_id": "c-1"}), json!([]));
    doc["decision"] = json!({"answers": answers, "missing": [], "model": "d"});
    doc
}

fn in_time(answers: Value) -> Value {
    decision(-3_600_000, json!({}), answers)
}

fn stamp_of(over: &[(&str, Value)], doc: Value) -> Value {
    let out = run_cell(ROUTE, over, doc).0;
    assert_eq!(out.len(), 1, "expected one emission, got {out:?}");
    let m = out.into_iter().next().expect("one emission");
    assert_eq!(m["header"]["route"], "stamp", "{m}");
    m
}

fn keys(stamp: &Value) -> Vec<String> {
    let mut k: Vec<String> = stamp
        .as_object()
        .expect("stamp object")
        .keys()
        .cloned()
        .collect();
    k.sort_unstable();
    k
}

fn answers(path_p: f64) -> Value {
    json!({
        "path": {"choice": "simple", "p": path_p},
        "intent": {"choice": "chat", "p": 0.8},
        "needs.memory": {"yes": 0.05}, "needs.tool": {"yes": 0.9},
        "needs.reasoning": {"yes": 0.45}, "needs.research": {"yes": 0.0}
    })
}

#[test]
fn gh_d58_recall_the_stamp_carries_the_yes_values() {
    let on = [("needs", four_needs()), ("stamp_needs_yes", json!("1"))];
    let seven = [
        "duration", "families", "intent", "needs", "p", "path", "turn_id",
    ];

    // Declared at the door, shipped off.
    let cfg = config_of(ROUTE);
    assert_eq!(cfg["params"]["stamp_needs_yes"], json!("0"), "ships off");
    let setting = &cfg["contract"]["settings"]["stamp_needs_yes"];
    assert_eq!(setting["default"], json!("0"), "{setting}");
    assert!(
        setting["description"]
            .as_str()
            .unwrap_or("")
            .contains("needs_yes"),
        "the setting names the stamp's field: {setting}"
    );

    // On: a judged stamp carries the yes-value of every asked need -- the
    // ones named in `needs` and the ones below the threshold alike.
    let m = stamp_of(&on, in_time(answers(0.91)));
    assert!(m["header"].get("fallback").is_none(), "judged: {m}");
    assert_eq!(m["stamp"]["needs"], json!(["tool"]), "{m}");
    assert_eq!(
        m["stamp"]["needs_yes"],
        json!({"memory": 0.05, "reasoning": 0.45, "research": 0.0, "tool": 0.9}),
        "{m}"
    );
    let mut with_field: Vec<String> = seven.iter().map(|k| k.to_string()).collect();
    with_field.push("needs_yes".to_string());
    with_field.sort_unstable();
    assert_eq!(keys(&m["stamp"]), with_field, "one key more, no other: {m}");

    // A need the decider did not answer with a number is absent, never 0.
    let mut partial = answers(0.91);
    let a = partial.as_object_mut().expect("answers");
    a.remove("needs.memory");
    a.insert("needs.research".to_string(), json!({"yes": true}));
    a.insert("needs.reasoning".to_string(), json!({"yes": "0.45"}));
    let m = stamp_of(&on, in_time(partial));
    assert_eq!(
        m["stamp"]["needs_yes"],
        json!({"tool": 0.9}),
        "an unanswered need is absent, so nobody reads silence as a sure no: {m}"
    );

    // Only asked needs: with the three shipped needs there is no `memory`.
    let m = stamp_of(&[("stamp_needs_yes", json!("1"))], in_time(answers(0.91)));
    assert_eq!(
        m["stamp"]["needs_yes"],
        json!({"reasoning": 0.45, "research": 0.0, "tool": 0.9}),
        "a need that was not asked is not carried: {m}"
    );

    // A fallback stamp carries no yes-values: unsure, late, error.
    let unsure = stamp_of(&on, in_time(answers(0.3)));
    assert_eq!(unsure["header"]["fallback"], "unsure", "{unsure}");
    let late = stamp_of(&on, decision(3_600_000, json!({}), answers(0.91)));
    assert_eq!(late["header"]["fallback"], "late", "{late}");
    let error = stamp_of(
        &on,
        decision(
            -3_600_000,
            json!({"finish_reason": "error", "error_code": "decide_failed"}),
            json!({}),
        ),
    );
    assert_eq!(error["header"]["fallback"], "error", "{error}");
    for m in [&unsure, &late, &error] {
        assert_eq!(
            keys(&m["stamp"]),
            seven,
            "a fallback stamp keeps the seven keys: {m}"
        );
    }

    // Off (shipped) and "0": the stamp of before, byte for byte.
    let plain = stamp_of(&[("needs", four_needs())], in_time(answers(0.91)));
    let off = stamp_of(
        &[("needs", four_needs()), ("stamp_needs_yes", json!("0"))],
        in_time(answers(0.91)),
    );
    assert_eq!(keys(&plain["stamp"]), seven, "{plain}");
    assert_eq!(off["stamp"], plain["stamp"], "\"0\" is the shipped stamp");
    assert_eq!(off["header"], plain["header"], "and the shipped hop");
    // The field is the only difference to the stamp with the knob on.
    let mut stripped = stamp_of(&on, in_time(answers(0.91)))["stamp"].clone();
    stripped
        .as_object_mut()
        .expect("stamp object")
        .remove("needs_yes");
    assert_eq!(stripped, plain["stamp"], "additive: nothing else moved");
}

// ---------------------------------------------------------------------------
// The way of the field from the stamp to the curator: the collector's seam.
//
// WHY this stands in the same file: the pin below is the second half of the
// same promise (the stamp carries the yes-values AND they reach the curator).
// The collector parked only the keys of `ROUTE_KEYS` with the route leg and
// laid only `path`, `needs`, `families`, `turn_notes` (and `fallback`) on the
// seam, so `needs_yes` stopped at the collector and the curator's rule had
// nothing to read in a running colony. It runs the shipped
// `collector/assemble` script. What it pins:
//
// 1. the lane `in_route` parks `needs_yes` with the leg, as the stamp sent it;
// 2. a seam of a turn whose stamp carries the field carries `needs_yes` on
//    the hop, the JSON text of the object (hop values are text, like
//    `turn_notes`);
// 3. a stamp without the field (the shipped route cell, an older one, a
//    fallback stamp) leaves a seam WITHOUT the key: byte for byte the seam of
//    before. The same holds for a field that is no object or an empty one --
//    nothing to read is not a "no".
// ---------------------------------------------------------------------------

const SEAM_TURN: &str = "route-channel#0001";
const SEAM_FRESH: &str = "2999-01-01T00:00:00.000000Z";

fn seam_knob() -> Vec<(&'static str, Value)> {
    vec![("memory_tier", json!("1")), ("route_wait_ms", json!(150))]
}

fn seam_leg(role: &str, payload: Value) -> Value {
    json!({"turn_id": SEAM_TURN, "iter": 0, "role": role, "turn": payload.to_string(),
           "fired": 0, "recorded_at": SEAM_FRESH})
}

/// The seam of a turn whose parked route leg is `stamp`.
fn seam_of(stamp: Value) -> Value {
    let window = seam_leg(
        "leg-window",
        json!({"turns": [{"role": "user", "text": "good morning, how are you",
                          "consult_id": ""}],
               "deferred": 0, "deferred_turns": []}),
    );
    // A `talk` turn waits for its memory leg; it is in, so the fan-in fires.
    let memory = seam_leg("leg-memory", json!({"system": {}, "messages": []}));
    let out = assemble(
        &seam_knob(),
        bundle_reply(
            "collect",
            SEAM_TURN,
            &[(
                "c-collect-read",
                json!([window, memory, seam_leg("leg-route", stamp)]),
            )],
        ),
    );
    out.iter()
        .find(|m| m["header"]["route"] == "curate")
        .cloned()
        .unwrap_or_else(|| panic!("no seam: {out:?}"))
}

/// What the lane `in_route` parks for `stamp`: the payload of the leg.
fn parked(stamp: Value) -> Value {
    let mut doc = lane(
        "in_route",
        json!({"turn_id": SEAM_TURN, "path": "talk"}),
        json!({}),
        json!([]),
    );
    doc["stamp"] = stamp;
    let out = assemble(&seam_knob(), doc);
    let row = calls_of(in_phase(&out, "collect"))
        .into_iter()
        .filter(|(_, a)| a["operation"] == "insert")
        .map(|(_, a)| a["row"].clone())
        .next()
        .unwrap_or_else(|| panic!("in_route parked no row: {out:?}"));
    assert_eq!(row["role"], "leg-route", "{row}");
    serde_json::from_str(row["turn"].as_str().expect("turn")).expect("json")
}

#[test]
fn gh_d58_recall_the_yes_values_cross_the_seam() {
    let yes = json!({"memory": 0.05, "tool": 0.1});
    let judged = json!({"turn_id": SEAM_TURN, "path": "talk", "p": 0.93, "intent": "chat",
                        "needs": [], "families": [], "duration": 120});
    let mut with_yes = judged.clone();
    with_yes["needs_yes"] = yes.clone();

    // 1. The lane parks the field with the leg, as sent; without it the
    //    parked leg holds no such key, and the field is the one difference.
    let leg_with = parked(with_yes);
    assert_eq!(
        leg_with["needs_yes"], yes,
        "the leg keeps the yes-values: {leg_with}"
    );
    let leg_without = parked(judged.clone());
    assert!(
        leg_without.get("needs_yes").is_none(),
        "no field on the stamp, no key on the leg: {leg_without}"
    );
    let mut stripped = leg_with.clone();
    stripped.as_object_mut().expect("leg").remove("needs_yes");
    assert_eq!(
        stripped, leg_without,
        "the field is the one difference of the parked leg"
    );

    // 2. The seam carries the JSON text of the object.
    let seam_with = seam_of(leg_with);
    let text = hop_str(&seam_with, "needs_yes");
    let back: Value = serde_json::from_str(&text).expect("needs_yes is JSON text");
    assert_eq!(back, yes, "the seam carries the yes-values: {text}");

    // 3. Without the field the seam has no such key, and the key is the one
    //    difference between the two seams.
    let seam_without = seam_of(leg_without);
    assert!(
        seam_without["header"].get("needs_yes").is_none(),
        "no field, no key: {}",
        seam_without["header"]
    );
    let mut cut = seam_with.clone();
    cut["header"]
        .as_object_mut()
        .expect("header")
        .remove("needs_yes");
    assert_eq!(
        cut, seam_without,
        "the key is the one difference of the seam"
    );

    // The whole way, from the shipped route cell: with `stamp_needs_yes` on
    // the yes-values of its judged stamp stand on the seam; shipped off, the
    // stamp carries none and the seam is the seam of before -- the curator's
    // rule then reads nothing and keeps every block.
    let asked = [("needs", four_needs()), ("stamp_needs_yes", json!("1"))];
    let sent_on = stamp_of(&asked, in_time(answers(0.91)))["stamp"].clone();
    let s = seam_of(parked(sent_on));
    let back: Value =
        serde_json::from_str(&hop_str(&s, "needs_yes")).expect("needs_yes is JSON text");
    assert_eq!(
        back,
        json!({"memory": 0.05, "reasoning": 0.45, "research": 0.0, "tool": 0.9}),
        "the route's yes-values reach the seam: {}",
        s["header"]
    );
    let sent_off = stamp_of(&[("needs", four_needs())], in_time(answers(0.91)))["stamp"].clone();
    let s = seam_of(parked(sent_off));
    assert!(
        s["header"].get("needs_yes").is_none(),
        "`stamp_needs_yes` off: no yes-values on the seam: {}",
        s["header"]
    );

    // Nothing to read is no key: an empty object, a value that is no object.
    for odd in [
        json!({}),
        json!(null),
        json!("memory"),
        json!(0.05),
        json!([0.05]),
        json!(true),
    ] {
        let mut stamp = judged.clone();
        stamp["needs_yes"] = odd.clone();
        let s = seam_of(stamp);
        assert!(
            s["header"].get("needs_yes").is_none(),
            "`needs_yes` {odd} puts no key on the seam: {}",
            s["header"]
        );
        assert_eq!(s, seam_without, "and the seam is the seam of before: {odd}");
    }
}
