//! #1412 -- the screen lead belongs to the chosen topic, an unsure turn keeps its
//! families on a knob, and the default route keeps today's bytes.
//!
//! WHY this file exists (live-2 r2b T5, D53 G5):
//!
//! 1. `./route` asks ONE flat `screen.lead` over the candidates of every topic
//!    (#1371). When the decider's lead belonged to another topic than the one it
//!    chose, `screen_of` made the lead `""` without a word (fu0:5: form right, lead
//!    missing). The decide answer carries the whole distribution of the lead
//!    question (`p` per option, `translate_decisions::choice_p`), so the lead is the
//!    best option OF THE CHOSEN TOPIC in that distribution. Only when no option of
//!    that topic has weight the lead stays `""` and the presenter takes its standard
//!    (`presenter/stage` `first = v["lead"] or manifest["standard"]`): the standard
//!    itself never reaches `./route` (the topic table carries `{name, purpose,
//!    candidates}` only). A `further` of another topic stays `""` -- nothing is made up.
//! 2. T5-15 ("how long does the egg timer still run?"): `path` simple 0.53 under
//!    `threshold` 0.6 stamped the fallback, and the fallback dropped every family
//!    the decider had said yes to. Families are their own yes/no questions, like
//!    the product's choices (#1304: an unsure path does not touch them), so with
//!    `unsure_families` "1" an unsure stamp keeps them. Off as shipped.
//! 3. Shipped (`judge_context` "0", `unsure_families` "0") the stamps are today's
//!    bytes (the decide call is held by the gh277 golden).

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::{lane, now_ms, run_cell, run_cell_stdout};
use serde_json::{Value, json};

const ROUTE: &str = "templates/talky/route/config.json";

fn ctx() -> Value {
    json!({"turn_id": "c-1", "channel": "tg:1"})
}

// Asked an hour ahead: the duration clamps to 0, so no start time of the
// cell's process ever makes a verdict late (dev-rules § 11).
fn in_time() -> String {
    (now_ms() + 3_600_000).to_string()
}

fn decision(answers: Value) -> Value {
    let hop = json!({"finish_reason": "stop", "asked_at": in_time()});
    let mut doc = lane("in_decision", hop, ctx(), json!([]));
    doc["decision"] = json!({"answers": answers, "missing": [], "model": "d"});
    doc
}

fn stamp_of(out: &[Value]) -> Value {
    out.iter()
        .find(|m| m["header"]["route"] == "stamp")
        .unwrap_or_else(|| panic!("no stamp in {out:?}"))["stamp"]
        .clone()
}

fn judged(answers: Value) -> Value {
    stamp_of(&run_cell(ROUTE, &[("judge_context", json!("1"))], decision(answers)).0)
}

fn weather_chosen(lead: Value, further: Value) -> Value {
    json!({
        "path": {"choice": "simple", "p": 0.9},
        "intent": {"choice": "question", "p": 0.8},
        "screen.topic": {"choice": "weather", "p": 0.8,
                         "_": "decisions wire: p per option"},
        "screen.lead": lead,
        "screen.further": further
    })
}

#[test]
fn the_lead_is_bound_to_the_chosen_topic() {
    // The decider named a calendar block as lead but chose the weather: the best
    // weather block of the lead's distribution leads.
    let s = judged(weather_chosen(
        json!({"choice": "calendar/day",
               "p": {"calendar/day": 0.5, "weather/week": 0.3, "weather/today": 0.2}}),
        json!({"choice": "weather/today",
               "p": {"weather/today": 0.6, "none": 0.4}}),
    ));
    assert_eq!(s["screen"]["topic"], "weather", "{s}");
    assert_eq!(s["screen"]["lead"], "week", "{s}");
    assert_eq!(s["screen"]["further"], "today", "{s}");

    // The bound lead is the further the decider named: the further goes, as ever.
    let s = judged(weather_chosen(
        json!({"choice": "calendar/day",
               "p": {"calendar/day": 0.6, "weather/today": 0.4}}),
        json!({"choice": "weather/today", "p": {"weather/today": 0.7, "none": 0.3}}),
    ));
    assert_eq!(s["screen"]["lead"], "today", "{s}");
    assert_eq!(s["screen"]["further"], "", "{s}");

    // Equal weight: the first by name, so the stamp never wobbles.
    let s = judged(weather_chosen(
        json!({"choice": "calendar/day",
               "p": {"calendar/day": 0.5, "weather/week": 0.25, "weather/today": 0.25}}),
        json!({"choice": "none", "p": {"none": 1.0}}),
    ));
    assert_eq!(s["screen"]["lead"], "today", "{s}");

    // No weight on any block of the chosen topic: no lead is made up, the
    // presenter's standard leads; a further of another topic stays "" as well.
    let s = judged(weather_chosen(
        json!({"choice": "calendar/day",
               "p": {"calendar/day": 0.9, "weather/week": 0.0}}),
        json!({"choice": "calendar/week",
               "p": {"calendar/week": 0.8, "weather/today": 0.2}}),
    ));
    assert_eq!(s["screen"]["lead"], "", "{s}");
    assert_eq!(s["screen"]["further"], "", "{s}");

    // A lead of the chosen topic stands as the decider named it.
    let s = judged(weather_chosen(
        json!({"choice": "weather/today",
               "p": {"weather/today": 0.4, "weather/week": 0.35, "calendar/day": 0.25}}),
        json!({"choice": "none", "p": {"none": 1.0}}),
    ));
    assert_eq!(s["screen"]["lead"], "today", "{s}");
}

fn timer_families() -> Value {
    json!({"web": "searching or reading the web",
           "set_timer": "setting a timer or a reminder"})
}

fn timer_unsure() -> Value {
    json!({
        "path": {"choice": "simple", "p": 0.53},
        "intent": {"choice": "question", "p": 0.8},
        "needs.tool": {"yes": 0.9},
        "families.set_timer": {"yes": 0.8},
        "families.web": {"yes": 0.1}
    })
}

#[test]
fn a_timer_without_family_is_stamped_timer() {
    let on = [
        ("families", timer_families()),
        ("unsure_families", json!("1")),
    ];
    let s = stamp_of(&run_cell(ROUTE, &on, decision(timer_unsure())).0);
    assert_eq!(s["path"], "talk", "{s}");
    assert_eq!(s["p"], 0.0, "{s}");
    assert_eq!(s["families"], json!(["set_timer"]), "{s}");
    // The needs ride with the path: the fallback way carries no tool duty.
    assert_eq!(s["needs"], json!([]), "{s}");

    // Off (shipped): the unsure stamp names no family, as before.
    let s = stamp_of(
        &run_cell(
            ROUTE,
            &[("families", timer_families())],
            decision(timer_unsure()),
        )
        .0,
    );
    assert_eq!(s["families"], json!([]), "{s}");

    // Late: the round is on its way, the knob changes nothing.
    let mut late = decision(timer_unsure());
    late["header"]["hop"]["asked_at"] = json!((now_ms() - 600_000).to_string());
    let s = stamp_of(&run_cell(ROUTE, &on, late).0);
    assert_eq!(s["families"], json!([]), "late: {s}");
}

const TODAY_UNSURE: &str = r#"{"header": {"route": "stamp", "turn_id": "c-1", "path": "talk", "fallback": "unsure"}, "messages": [], "stamp": {"turn_id": "c-1", "path": "talk", "p": 0.0, "intent": "question", "needs": [], "families": [], "duration": 0}}"#;
const TODAY_SURE: &str = r#"{"header": {"route": "stamp", "turn_id": "c-1", "path": "simple"}, "messages": [], "stamp": {"turn_id": "c-1", "path": "simple", "p": 0.9, "intent": "question", "needs": ["tool"], "families": ["web"], "duration": 0}}"#;

#[test]
fn the_default_route_bytes_do_not_move() {
    let p = assemble_cell::config_of(ROUTE)["params"].clone();
    assert_eq!(p["judge_context"], "0", "{p}");
    assert_eq!(p["unsure_families"], "0", "{p}");

    // An unsure verdict that said yes to a family: today's fallback bytes.
    let unsure = json!({
        "path": {"choice": "complex", "p": 0.3},
        "intent": {"choice": "question", "p": 0.8},
        "needs.tool": {"yes": 0.9},
        "families.web": {"yes": 0.95}
    });
    let out = run_cell_stdout(ROUTE, &[], decision(unsure)).0;
    assert_eq!(
        out.trim_end(),
        TODAY_UNSURE,
        "the unsure stamp is today's bytes"
    );

    // A sure verdict with screen answers but no judge context: no screen.
    let sure = json!({
        "path": {"choice": "simple", "p": 0.9},
        "intent": {"choice": "question", "p": 0.8},
        "needs.tool": {"yes": 0.9},
        "families.web": {"yes": 0.95},
        "screen.topic": {"choice": "weather", "p": {"weather": 0.9}},
        "screen.lead": {"choice": "calendar/day",
                        "p": {"calendar/day": 0.6, "weather/week": 0.4}}
    });
    let out = run_cell_stdout(ROUTE, &[], decision(sure)).0;
    assert_eq!(
        out.trim_end(),
        TODAY_SURE,
        "the sure stamp is today's bytes"
    );
}
