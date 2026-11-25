//! GH #1585 (wave d54s, strand richter; Nr. 48 b, Nr. 22) -- the second judge of
//! `talky/route` in three points.
//!
//! WHY this file exists: measured in set 0.72 (leser-48b § 3), the second judge
//! ran where nothing it said could change the turn, and changed it where its
//! word was weak:
//! 1. on a turn with a crisis note `steer` pulls the class onto `notes_class`
//!    anyway, yet the second judge was asked (0.9-2.8 s on the turn that may
//!    not wait);
//! 2. a second `simple` at p 0.5/0.6 replaced a first `talk` at 0.9 -- any
//!    class but the asking one stood without a bar; now it replaces the class
//!    only at `sure.second_change` (shipped 0.9), below it the first verdict
//!    stands;
//! 3. a failing judge (error, its own timeout, no JSON) closed the turn on
//!    `close_unsure`, which threw away the first verdict's needs and families;
//!    now the first verdict stands with them.
//!
//! The asking class (`clarify`, from `sure.second`) and the crisis depth stay
//! as they were; a pin each holds that. Runs the shipped script of the cell
//! (`support/assemble_cell.rs`); the decider and the second judge are
//! fixtures, no model is called, no assertion reads a clock.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::{config_of, lane, now_ms, run_cell, talky_view_of};
use serde_json::{Value, json};

const ROUTE: &str = "templates/talky/route/config.json";
const JUDGE: &str = "judge_two";
const ASKING: &str = "clarify";
const TEXT: &str = "Please put the drive to my mother in the calendar for two o'clock.";

type Knob = (&'static str, Value);

fn ctx() -> Value {
    json!({"turn_id": "c-1", "channel": "tg:1", "audience_set": "[\"m-1\"]"})
}

// Asked an hour ahead: the duration clamps to 0 (dev-rules § 11).
fn in_time() -> String {
    (now_ms() + 3_600_000).to_string()
}

fn risk_choice() -> Value {
    json!({"risk": {
        "question": "How is the person doing, as far as their safety goes?",
        "options": {"none": "no sign of distress", "acute": "in danger now"},
        "fallback": "unknown",
        "threshold": 0,
        "notes": {"acute": "CRISIS: give the emergency number now."}
    }})
}

/// The composition of 0.72 in short: the path bar 0.9841, the asking class at
/// 0.9, the class sharpened by a second judge; `more` on top.
fn knobs(more: &[Knob]) -> Vec<Knob> {
    let mut all = vec![
        ("sure", json!({"path": 0.9841, "move": 0.95, "second": 0.9})),
        ("sharpen", json!(["path"])),
        ("second_judge", json!(JUDGE)),
        ("second_wait_ms", json!(5000)),
        (ASKING, json!("1")),
    ];
    all.extend(more.iter().cloned());
    all
}

/// The first verdict: `path` with `p`, a need and a family it said yes to.
fn first(path: &str, p: f64) -> Value {
    json!({"path": {"choice": path, "p": p},
           "intent": {"choice": "task", "p": 0.86},
           "needs.tool": {"yes": 0.8},
           "families.history": {"yes": 0.69}})
}

fn in_crisis(mut answers: Value) -> Value {
    answers["choices.risk"] = json!({"choice": "acute", "p": 0.97});
    answers
}

fn decision(answers: Value) -> Value {
    let mut doc = lane(
        "in_decision",
        json!({"finish_reason": "stop", "asked_at": in_time(), "state": TEXT}),
        ctx(),
        json!([]),
    );
    doc["decision"] = json!({"answers": answers, "missing": [], "model": "d"});
    doc
}

fn routes(out: &[Value]) -> Vec<String> {
    out.iter()
        .map(|m| m["header"]["route"].as_str().unwrap_or("").to_string())
        .collect()
}

fn on<'a>(out: &'a [Value], route: &str) -> &'a Value {
    out.iter()
        .find(|m| m["header"]["route"] == route)
        .unwrap_or_else(|| panic!("no {route} in {out:?}"))
}

fn stamp_of(out: &[Value]) -> &Value {
    &on(out, "stamp")["stamp"]
}

fn carried(msg: &Value) -> Value {
    let mut hop = json!({});
    for (k, v) in msg["header"].as_object().expect("header") {
        if k != "route" {
            hop[k.as_str()] = v.clone();
        }
    }
    hop["asked_at"] = json!(in_time());
    hop
}

fn wall() -> Value {
    json!({"turns": [], "topic": "", "screen": {"topics": []},
           "now": {"iso": "2026-10-30T09:00:00+01:00", "weekday": "Friday",
                   "timezone": "Europe/Berlin"},
           "place": "Berlin"})
}

/// The first verdict `answers` under `over` up to the call to the second
/// judge, then its answer: `hop` on the lane (`finish_reason`, an error) and
/// `text` as its one message.
fn judged(over: &[Knob], answers: Value, hop: Value, text: &str) -> Vec<Value> {
    let asked = run_cell(ROUTE, over, decision(answers)).0;
    let ask = on(&asked, "view_ask").clone();
    let (messages, system) = talky_view_of(&wall());
    let mut view = lane("in_second_view", carried(&ask), ctx(), messages);
    view["system"] = system;
    let called = run_cell(ROUTE, over, view).0;
    let mut back = carried(on(&called, JUDGE));
    for (k, v) in hop.as_object().expect("hop") {
        back[k.as_str()] = v.clone();
    }
    let answer = lane(
        "in_second",
        back,
        ctx(),
        json!([{"origin": "assistant", "type": "text", "text": text}]),
    );
    run_cell(ROUTE, over, answer).0
}

fn second_says(over: &[Knob], answers: Value, path: &str, p: f64) -> Vec<Value> {
    let word = json!({"path": {"choice": path, "p": p}}).to_string();
    judged(over, answers, json!({"finish_reason": "stop"}), &word)
}

fn keeps_the_first(stamp: &Value, path: &str) {
    assert_eq!(stamp["path"], json!(path), "{stamp}");
    assert_eq!(stamp["needs"], json!(["tool"]), "the needs stay: {stamp}");
    assert_eq!(
        stamp["families"],
        json!(["history"]),
        "the families stay: {stamp}"
    );
}

/// Contract point 1: a turn whose crisis note `steer` pulls onto talk anyway
/// asks no second judge -- sure or unsure, and however the asking class
/// stands. The note and the risk ride the stamp (the crisis depth of before).
#[test]
fn gh1585_a_crisis_note_asks_no_second_judge() {
    let over = knobs(&[("choices", risk_choice())]);
    for start in [
        first("simple", 0.7),
        first(ASKING, 0.97),
        first("complex", 0.9),
    ] {
        let out = run_cell(ROUTE, &over, decision(in_crisis(start.clone()))).0;
        assert_eq!(routes(&out), vec!["stamp".to_string()], "{start}: {out:?}");
        let stamp = stamp_of(&out);
        assert_eq!(
            stamp["path"],
            json!("talk"),
            "steer pulls onto talk: {stamp}"
        );
        assert_eq!(stamp["choices"]["risk"]["value"], json!("acute"), "{stamp}");
        let notes = stamp["turn_notes"].as_array().cloned().unwrap_or_default();
        assert!(notes.iter().any(|n| n["id"] == "route.risk"), "{stamp}");
        assert_eq!(stamp["trail"]["sharpened"], json!(false), "{stamp}");
    }
    // Without a note the same unsure verdict is sharpened as before.
    let out = run_cell(ROUTE, &over, decision(first("simple", 0.7))).0;
    assert_eq!(routes(&out), vec!["view_ask".to_string()], "{out:?}");
}

/// Contract point 2: the second word replaces the class only at
/// `sure.second_change` (shipped 0.9); below it the first verdict stands.
#[test]
fn gh1585_a_weak_second_word_leaves_the_first_class() {
    let over = knobs(&[]);
    let out = second_says(&over, first("talk", 0.9), "simple", 0.6);
    assert_eq!(routes(&out), vec!["stamp".to_string()], "{out:?}");
    keeps_the_first(stamp_of(&out), "talk");
    assert_eq!(stamp_of(&out)["p"], json!(0.9), "{out:?}");
    let out = second_says(&over, first("talk", 0.9), "complex", 0.89);
    keeps_the_first(stamp_of(&out), "talk");
    // At the bar the second word stands.
    let out = second_says(&over, first("talk", 0.9), "simple", 0.9);
    assert_eq!(stamp_of(&out)["path"], json!("simple"), "{out:?}");
    // A first asking class can not stand by itself: under the bar the turn
    // falls back, keeping what the first verdict said beside the class.
    let out = second_says(&over, first(ASKING, 0.97), "simple", 0.6);
    keeps_the_first(stamp_of(&out), "talk");
}

/// Contract point 2, the set's bar: `sure` is a declared param (the door
/// checks a set's override by its top-level name), and the set's
/// `second_change` is the bar that is read.
#[test]
fn gh1585_the_sets_second_change_is_read() {
    let route = config_of(ROUTE);
    assert!(
        route["params"].get("sure").is_some(),
        "declared: {}",
        route["params"]
    );
    let said = route["contract"]["settings"]["sure"]["description"]
        .as_str()
        .unwrap_or("");
    assert!(said.contains("second_change"), "{said}");
    let low = knobs(&[(
        "sure",
        json!({"path": 0.9841, "move": 0.95, "second": 0.9, "second_change": 0.5}),
    )]);
    let out = second_says(&low, first("talk", 0.9), "simple", 0.6);
    assert_eq!(stamp_of(&out)["path"], json!("simple"), "{out:?}");
}

/// Contract point 3: a failing second judge (an error of its cell, its own
/// timeout, an answer without JSON) leaves the first verdict WITH its needs
/// and families.
#[test]
fn gh1585_a_failed_second_judge_keeps_the_first_verdict() {
    let over = knobs(&[]);
    let error = json!({"finish_reason": "error", "error_code": "timeout"});
    let stop = json!({"finish_reason": "stop"});
    for (hop, text) in [(error.clone(), ""), (stop, "no json here")] {
        let out = judged(&over, first("talk", 0.9), hop.clone(), text);
        assert_eq!(routes(&out), vec!["stamp".to_string()], "{out:?}");
        keeps_the_first(stamp_of(&out), "talk");
        assert_eq!(stamp_of(&out)["p"], json!(0.9), "{hop}: {out:?}");
    }
    // A first asking class falls back, needs and families kept.
    let out = judged(&over, first(ASKING, 0.97), error, "");
    keeps_the_first(stamp_of(&out), "talk");
}

/// Contract point 4: the asking class stands only on the second judge's word
/// at `sure.second` (0.9), as before -- `second_change` does not touch it.
#[test]
fn gh1585_the_asking_class_keeps_its_bar() {
    let over = knobs(&[]);
    let out = second_says(&over, first("simple", 0.9), ASKING, 0.95);
    assert_eq!(stamp_of(&out)["path"], json!(ASKING), "{out:?}");
    let out = second_says(&over, first("simple", 0.9), ASKING, 0.6);
    assert_ne!(stamp_of(&out)["path"], json!(ASKING), "{out:?}");
    assert_eq!(
        on(&out, "stamp")["header"]["fallback"],
        json!("unsure"),
        "{out:?}"
    );
    let low = knobs(&[(
        "sure",
        json!({"path": 0.9841, "second": 0.9, "second_change": 0.1}),
    )]);
    let out = second_says(&low, first("simple", 0.9), ASKING, 0.6);
    assert_ne!(stamp_of(&out)["path"], json!(ASKING), "{out:?}");
}
