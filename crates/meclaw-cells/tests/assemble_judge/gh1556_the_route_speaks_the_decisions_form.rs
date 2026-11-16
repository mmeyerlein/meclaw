//! GH #1556 -- the decide state and the questions in the decisions providers' form.
//!
//! WHY this file exists: the decisions API guidance says "Write the full question
//! in the instructions. The model never sees question keys" and "Send only the
//! fields the questions need. Unrelated content lowers accuracy." The route sent
//! "On the screen right now:\n- nothing" on every judged turn although no screen
//! state reaches the curator, and asked "Does answering it need ...?" -- a
//! question without its subject. The measured provider form (text state, the
//! subject written out as `the person's latest request ("The request now")`)
//! held where the production form failed. So: no screen line without a screen
//! state, every question the route writes names its subject, and the label the
//! subject points at stands in every decide state -- while the hop keeps the raw
//! request, which the curator's comparison of a partial and the second judge read.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/history_judge.rs"]
#[allow(unused_imports)]
use crate::history_judge; // harness_fold: mod history_judge;
// harness_fold: #[path = "support/history_view.rs"]
#[allow(unused_imports)]
use crate::history_view; // harness_fold: mod history_view;

use assemble_cell::{lane, now_ms, run_cell};
use curator_hive::{Hive, said, user};
use history_judge::{at, drive, topic};
use history_view::{hints_value, part, screen_value, the_view, view_params};
use serde_json::{Value, json};

const ROUTE: &str = "templates/talky/route/config.json";
const TEXT: &str = "How is the weather tomorrow?";
const SUBJECT: &str = "the person's latest request (\"The request now\")";

fn ctx() -> Value {
    json!({"turn_id": "t2", "session_id": "s-now", "channel": "tg:1",
           "audience_set": "[\"member:e\"]"})
}

fn delta_ctx() -> Value {
    json!({"turn_id": "c-1", "channel": "voice:1", "delta": "1"})
}

fn said_now() -> Value {
    json!([{"origin": "user", "type": "text", "text": TEXT}])
}

// Asked an hour ahead: no start time of the cell's process makes it late
// (dev-rules § 11).
fn in_time() -> String {
    (now_ms() + 3_600_000).to_string()
}

fn on<'a>(out: &'a [Value], route: &str) -> &'a Value {
    out.iter()
        .find(|m| m["header"]["route"] == route)
        .unwrap_or_else(|| panic!("no {route} in {out:?}"))
}

/// The whole judged chain: the route's view ask through the shipped `./history`
/// (one earlier turn of both sides, its topic, the presenter's screen topics and
/// the affinity's hints), the view back to the route on `in_view`; the decide
/// emission the route sends.
fn judged(knobs: &[(&str, Value)]) -> Value {
    let mut h = Hive::new();
    let params = view_params();
    h.row(at(1), "s-now", "t1", "user", &user("Good morning."), 1);
    let later = |s: i64| at(1) + chrono::Duration::seconds(s);
    h.row(
        later(20),
        "s-now",
        "t1",
        "assistant",
        &said("Good morning to you."),
        1,
    );
    h.mark(
        later(40),
        "s-now",
        "t1",
        "topic",
        &topic("the trip on Sunday", "start"),
    );
    let _ = part(&h, &params, "decide", "presenter", screen_value(), "");
    let _ = part(&h, &params, "decide", "affinity", hints_value(), "member:e");

    let asked = run_cell(ROUTE, knobs, lane("in_turn", json!({}), ctx(), said_now())).0;
    let ask = on(&asked, "view_ask");
    let view = the_view(&drive(
        &h,
        &params,
        "in_view_ask",
        json!({"session_id": "s-now", "turn_id": "t2"}),
        json!({}),
        json!({"messages": [], "view_ask": ask["view_ask"].clone()}),
    ));
    let mut back = lane(
        "in_view",
        json!({"state": TEXT, "asked_at": in_time()}),
        ctx(),
        view.body["messages"].clone(),
    );
    back["system"] = view.body["system"].clone();
    let out = run_cell(ROUTE, knobs, back).0;
    on(&out, "decide").clone()
}

fn judge_knobs() -> Vec<(&'static str, Value)> {
    vec![
        ("judge_context", json!("1")),
        ("context_turns", json!(2)),
        (
            "moves",
            json!({"gently": "the person sounds low and needs a soft answer"}),
        ),
    ]
}

#[test]
fn gh1556_the_decide_state_carries_no_screen_line_without_a_screen_state() {
    let decide = judged(&judge_knobs());
    let state = decide["decide"]["state"].as_str().unwrap_or("");
    assert!(
        !state.contains("On the screen"),
        "no screen state reached the curator, so no screen line: {state}"
    );
    assert!(
        state.contains("The conversation is about: the trip on Sunday"),
        "the rest of the state stays: {state}"
    );
    assert!(
        state.ends_with(&format!("The request now:\n{TEXT}")),
        "{state}"
    );
    // The screen questions (the presenter's topics) stay.
    let qs = &decide["decide"]["questions"];
    for key in ["screen.topic", "screen.lead", "screen.further"] {
        assert!(qs.get(key).is_some(), "{key} stays asked: {qs}");
    }
}

#[test]
fn gh1556_every_decide_question_names_its_subject() {
    let decide = judged(&judge_knobs());
    let qs = decide["decide"]["questions"]
        .as_object()
        .cloned()
        .expect("questions");
    for key in [
        "path",
        "intent",
        "needs.tool",
        "families.web",
        "move",
        "screen.topic",
        "screen.lead",
    ] {
        assert!(qs.contains_key(key), "a full package asks {key}: {qs:?}");
    }
    for (key, q) in &qs {
        if key.starts_with("choices.") {
            // Their question comes from the composition's param.
            continue;
        }
        let text = q["instructions"].as_str().unwrap_or("");
        for bad in [" it ", " it?", "this request"] {
            assert!(
                !text.contains(bad),
                "{key} asks without its subject ({bad:?}): {text}"
            );
        }
        if key != "screen.further" {
            assert!(text.contains(SUBJECT), "{key} names its subject: {text}");
        }
    }
    let ask = |k: &str| qs[k]["instructions"].as_str().unwrap_or("").to_string();
    assert_eq!(
        ask("path"),
        format!("Which way should {SUBJECT} be answered?")
    );
    assert_eq!(
        ask("intent"),
        format!("What does the person want with {SUBJECT}?")
    );
    assert_eq!(
        ask("needs.tool"),
        format!("Does answering {SUBJECT} need a tool call?")
    );
    assert_eq!(
        ask("families.web"),
        format!("Would answering {SUBJECT} use searching or reading the web?")
    );
    assert_eq!(
        ask("move"),
        format!("In which manner should {SUBJECT} be answered, if any of these fits?")
    );
    assert_eq!(
        ask("screen.topic"),
        format!("Which of these would help on a screen beside the answer to {SUBJECT}?")
    );
    assert_eq!(
        ask("screen.lead"),
        format!("Which block of the topic that would help shows best what {SUBJECT} asks about?")
    );

    // The turn without a view asks the same questions.
    let out = run_cell(ROUTE, &[], lane("in_turn", json!({}), ctx(), said_now())).0;
    let plain = &on(&out, "decide")["decide"]["questions"];
    assert_eq!(plain["path"]["instructions"], json!(ask("path")));
    assert_eq!(
        plain["needs.tool"]["instructions"],
        json!(ask("needs.tool"))
    );
}

#[test]
fn gh1556_the_request_label_stands_without_earlier_words() {
    let labelled = format!("The request now:\n{TEXT}");
    // `in_turn` without a view: the decide call goes straight out.
    let out = run_cell(ROUTE, &[], lane("in_turn", json!({}), ctx(), said_now())).0;
    assert_eq!(on(&out, "decide")["decide"]["state"], json!(labelled));
    // ... also for a partial ...
    let out = run_cell(
        ROUTE,
        &[],
        lane("in_turn", json!({}), delta_ctx(), said_now()),
    )
    .0;
    assert_eq!(on(&out, "decide")["decide"]["state"], json!(labelled));
    // ... and for a turn that may be sharpened.
    let sharpen = [
        ("second_judge", json!("second")),
        ("sharpen", json!(["path"])),
    ];
    let out = run_cell(
        ROUTE,
        &sharpen,
        lane("in_turn", json!({}), ctx(), said_now()),
    )
    .0;
    assert_eq!(on(&out, "decide")["decide"]["state"], json!(labelled));

    // `in_view` without earlier words: the view of the person's words ...
    let back = lane(
        "in_view",
        json!({"state": TEXT, "asked_at": in_time()}),
        ctx(),
        json!([]),
    );
    let out = run_cell(ROUTE, &[("context_turns", json!(2))], back.clone()).0;
    assert_eq!(on(&out, "decide")["decide"]["state"], json!(labelled));
    // ... and a judged view without any part.
    let mut bare = back;
    bare["system"] = json!({});
    let out = run_cell(ROUTE, &[("judge_context", json!("1"))], bare).0;
    assert_eq!(on(&out, "decide")["decide"]["state"], json!(labelled));
}

#[test]
fn gh1556_the_hop_keeps_the_raw_request() {
    let raw = "  I do NOT want to go on.  ";
    let words = json!([{"origin": "user", "type": "text", "text": raw}]);
    // A partial: the raw text rides out on the hop, the label only in the state.
    let out = run_cell(ROUTE, &[], lane("in_turn", json!({}), delta_ctx(), words)).0;
    let decide = on(&out, "decide");
    let hop_state = decide["header"]["state"].as_str().unwrap_or("").to_string();
    assert!(
        !hop_state.contains("The request now"),
        "the hop carries the raw request: {hop_state:?}"
    );
    assert!(
        decide["decide"]["state"]
            .as_str()
            .unwrap_or("")
            .starts_with("The request now:\n"),
        "{decide}"
    );
    // The decider's answer comes back with that hop; the curator's comparison
    // reads the normalised raw text.
    let mut doc = lane(
        "in_decision",
        json!({"finish_reason": "stop", "asked_at": in_time(), "state": hop_state}),
        delta_ctx(),
        json!([]),
    );
    doc["decision"] = json!({"answers": {
        "path": {"choice": "talk", "p": 0.9},
        "intent": {"choice": "chat", "p": 0.8}
    }, "missing": [], "model": "d"});
    let out = run_cell(ROUTE, &[], doc).0;
    assert_eq!(
        on(&out, "delta_stamp")["stamp"]["text_norm"],
        json!("i do not want to go on"),
        "{out:?}"
    );

    // A turn that may be sharpened: the second judge reads the raw request.
    let sharpen = [
        ("second_judge", json!("second")),
        ("sharpen", json!(["path"])),
    ];
    let out = run_cell(
        ROUTE,
        &sharpen,
        lane("in_turn", json!({}), ctx(), said_now()),
    )
    .0;
    assert_eq!(on(&out, "decide")["header"]["state"], json!(TEXT));
    let back = lane(
        "in_view",
        json!({"state": TEXT, "asked_at": in_time()}),
        ctx(),
        json!([]),
    );
    let mut over = sharpen.to_vec();
    over.push(("context_turns", json!(2)));
    let out = run_cell(ROUTE, &over, back).0;
    let decide = on(&out, "decide");
    assert_eq!(decide["header"]["state"], json!(TEXT));
    assert_eq!(
        decide["decide"]["state"],
        json!(format!("The request now:\n{TEXT}"))
    );
}
