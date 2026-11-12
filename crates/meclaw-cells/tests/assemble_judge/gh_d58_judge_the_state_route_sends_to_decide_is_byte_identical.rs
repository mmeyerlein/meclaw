//! D58 judge (GH #1480, contract point 7) -- the state the route sends to
//! decide is byte identical.
//!
//! WHY this file exists: `./decide` takes no window (its provider
//! `decisions` takes `decide.state` and `decide.questions` only,
//! `translate_decisions.rs`), so the view goes back to the orderer, and
//! `talky/route` folds it into `decide.state` as it folded the package
//! `judge`. The text it sends must stay the bytes of before -- the earlier
//! turns of both sides, the topic, the person's how-questions, the line
//! "On the screen right now: - nothing", now and place -- and so must the
//! screen questions built from the presenter's topics.
//!
//! The chain as the composition runs it: the route's ask (`judge_context`
//! "1", `context_turns` 2) goes to the shipped `./history` on `in_view_ask`,
//! the view comes back to the route on `in_view` with the turn's text on
//! `hop.state`. `TODAY_STATE` and `TODAY_QUESTIONS` are what the route of
//! master before the strand sent for the same wall with the package `judge`
//! (computed from `templates/talky/route/config.json` at abbbd811 with the
//! judge package `./history` gave for this wall).

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

const TODAY_STATE: &str = "Earlier in this conversation (oldest first):\n- person: Good morning.\n- companion: Good morning to you.\n\nThe conversation is about: the trip on Sunday\n\nHow this person meant earlier requests when asked (newest first):\n- asked \"Its concerts or its website?\", the person answered \"its concerts\" (class: simple)\n\nOn the screen right now:\n- nothing\n\nNow: Friday, 2026-10-30T02:00:00+01:00 (Europe/Berlin)\n\nPlace: Berlin\n\nThe request now:\nHow is the weather tomorrow?";

const TODAY_QUESTIONS: &str = r#"{"families.history": {"instructions": "Would answering it use earlier conversations with this person?", "kind": "yes_no"}, "families.web": {"instructions": "Would answering it use searching or reading the web?", "kind": "yes_no"}, "intent": {"instructions": "What does the person want with this request?", "kind": "choice", "options": {"chat": "to talk, share or be heard", "other": "none of these", "question": "to know something", "task": "to get something done"}}, "needs.reasoning": {"instructions": "Does answering it need careful step-by-step reasoning?", "kind": "yes_no"}, "needs.research": {"instructions": "Does answering it need a deep search over many sources?", "kind": "yes_no"}, "needs.tool": {"instructions": "Does answering it need a tool call?", "kind": "yes_no"}, "path": {"instructions": "Which way should this request be answered?", "kind": "choice", "options": {"complex": "needs careful reasoning, planning or several steps before it can be answered", "research": "needs a deep search over many sources before it can be answered", "simple": "one quick answer: a fact, a short lookup or one tool call, no deliberation", "talk": "small talk, a greeting, a feeling or a reply that needs no fact and no tool"}}, "screen.further": {"instructions": "Which second block of that same topic helps beside the first one, if any?", "kind": "choice", "options": {"none": "no second block", "weather/today": "weather: today's weather", "weather/week": "weather: the week ahead"}}, "screen.lead": {"instructions": "Which block of the topic that would help shows best what the request asks about?", "kind": "choice", "options": {"weather/today": "weather: today's weather", "weather/week": "weather: the week ahead"}}, "screen.topic": {"instructions": "Which of these would help on a screen beside the answer to this request?", "kind": "choice", "options": {"calendar": "the person's appointments", "none": "nothing on the screen helps with this request", "weather": "the weather today and the next days"}}}"#;

fn ctx() -> Value {
    json!({"turn_id": "t2", "session_id": "s-now", "channel": "tg:1",
           "audience_set": "[\"member:e\"]"})
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

#[test]
fn gh_d58_judge_the_state_route_sends_to_decide_is_byte_identical() {
    // The wall: one earlier turn of both sides and its topic.
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
    // The domains' contributions for the route's model.
    let _ = part(&h, &params, "decide", "presenter", screen_value(), "");
    let _ = part(&h, &params, "decide", "affinity", hints_value(), "member:e");

    let knobs = [("judge_context", json!("1")), ("context_turns", json!(2))];
    let turn = lane(
        "in_turn",
        json!({}),
        ctx(),
        json!([{"origin": "user", "type": "text", "text": TEXT}]),
    );
    let asked = run_cell(ROUTE, &knobs, turn).0;
    let ask = on(&asked, "view_ask");
    assert!(
        asked.iter().all(|m| m["header"]["route"] != "turns_ask"),
        "no turns_ask any more: {asked:?}"
    );

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
    let out = run_cell(ROUTE, &knobs, back).0;
    let decide = &on(&out, "decide")["decide"];
    assert_eq!(
        decide["state"].as_str().unwrap_or(""),
        TODAY_STATE,
        "the text the route sends to decide is today's, byte for byte"
    );
    let want: Value = serde_json::from_str(TODAY_QUESTIONS).expect("json");
    assert_eq!(decide["questions"], want, "and so are the questions");
}
