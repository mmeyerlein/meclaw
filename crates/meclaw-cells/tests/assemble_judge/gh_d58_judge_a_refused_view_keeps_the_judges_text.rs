//! D58 judge (GH #1480, contract point 7, review I1) -- a refused or
//! degraded view is no judge context, and the text the judges send stays
//! the bytes of before.
//!
//! WHY this file exists: review 1 of the strand measured that `./history`
//! answered a view with a target, refused at its door (`no_round`) or
//! refused / degraded after the first ledger round (`no_round`,
//! `degraded: store`), with `system: {}`, and that `talky/route` took every
//! object as a judge context. The former turns ask answered the door's
//! refusal without a package (the route sent the turn alone) and the later
//! ones with a package of now and place without turns, topic or screen (the
//! route sent "On the screen right now: - nothing", now and place). The
//! normal-way pin (`..._the_state_route_sends_to_decide_is_byte_identical`)
//! never drove those ways. Measured with the reviewer's probe: the door's
//! refusal sent `On the screen right now:\n- nothing\n\nThe request now:...`
//! instead of the turn alone, a degraded read lost the lines `Now:` and
//! `Place:`.
//!
//! The chain: the shipped `./history` answers the view (the door's refusal
//! on the lane `in_view_ask` without a round; the later ones on a ledger
//! answer of the phase `view-rows`, as the store hands it back with the
//! phase promoted into the context), and its answer goes on `in_view` to
//! `talky/route` (`judge_context` "1") and `talky/screen_judge`
//! (`enabled` "1"). `TODAY_*` are what the route and the screen judge of
//! master before the strand (abbbd811) sent with the answer the history of
//! master gave in the same place (computed by running both scripts of that
//! commit; the questions are the route's without a screen question).

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
use curator_hive::{Hive, Msg};
use history_judge::drive;
use history_view::{the_view, view_params};
use serde_json::{Value, json};

const ROUTE: &str = "templates/talky/route/config.json";
const SCREEN_JUDGE: &str = "templates/talky/screen_judge/config.json";
const TEXT: &str = "How is the weather tomorrow?";

/// The door's refusal: the former turns ask gave no package, the turn went alone.
const TODAY_DOOR: &str = "How is the weather tomorrow?";

/// A refusal or a degraded read after the first ledger round: the package of
/// now and place alone.
const TODAY_LATER: &str = "On the screen right now:\n- nothing\n\nNow: Friday, 2026-10-30T02:00:00+01:00 (Europe/Berlin)\n\nPlace: Berlin\n\nThe request now:\nHow is the weather tomorrow?";

const TODAY_QUESTIONS: &str = r#"{"families.history": {"instructions": "Would answering it use earlier conversations with this person?", "kind": "yes_no"}, "families.web": {"instructions": "Would answering it use searching or reading the web?", "kind": "yes_no"}, "intent": {"instructions": "What does the person want with this request?", "kind": "choice", "options": {"chat": "to talk, share or be heard", "other": "none of these", "question": "to know something", "task": "to get something done"}}, "needs.reasoning": {"instructions": "Does answering it need careful step-by-step reasoning?", "kind": "yes_no"}, "needs.research": {"instructions": "Does answering it need a deep search over many sources?", "kind": "yes_no"}, "needs.tool": {"instructions": "Does answering it need a tool call?", "kind": "yes_no"}, "path": {"instructions": "Which way should this request be answered?", "kind": "choice", "options": {"complex": "needs careful reasoning, planning or several steps before it can be answered", "research": "needs a deep search over many sources before it can be answered", "simple": "one quick answer: a fact, a short lookup or one tool call, no deliberation", "talk": "small talk, a greeting, a feeling or a reply that needs no fact and no tool"}}}"#;

fn ctx() -> Value {
    json!({"turn_id": "t2", "session_id": "s-now", "channel": "tg:1",
           "audience_set": "[\"member:e\"]"})
}

// Asked an hour ahead: no start time of the cell's process makes it late
// (dev-rules § 11).
fn in_time() -> String {
    (now_ms() + 3_600_000).to_string()
}

/// The arguments the view's first ledger round carries in `cur_reason`.
fn reason() -> String {
    json!({"target": "decide", "n": 2, "budget": 1500, "limit": 8}).to_string()
}

/// The door's refusal: a view ask for `decide` in a context without a round.
fn refused_at_the_door(h: &Hive) -> Msg {
    let params = view_params();
    the_view(&drive(
        h,
        &params,
        "in_view_ask",
        json!({"session_id": "s-now", "turn_id": "t2", "audience_set": ""}),
        json!({}),
        json!({"messages": [],
               "view_ask": {"target": "decide", "turns": 2, "budget_tokens": 1500}}),
    ))
}

/// The ledger's answer of the round `view-rows` coming back to `./history`:
/// without a round (`audience_set` ""), or with the store's error code.
fn later(h: &Hive, audience: &str, error_code: &str) -> Msg {
    let params = view_params();
    let mut hop = json!({"operation": "select"});
    if !error_code.is_empty() {
        hop["error_code"] = json!(error_code);
    }
    the_view(&drive(
        h,
        &params,
        "",
        json!({"session_id": "s-now", "turn_id": "t2", "audience_set": audience,
               "cur_phase": "view-rows", "cur_reason": reason(),
               "cur_origin": "history", "cur_call": ""}),
        hop,
        json!({"messages": []}),
    ))
}

/// The view on `in_view` to a judge, with the curator's flags on the hop.
fn back(view: &Msg) -> Value {
    let mut hop = json!({"state": TEXT, "asked_at": in_time()});
    for flag in ["refused", "degraded"] {
        if let Some(v) = view.hop.get(flag) {
            hop[flag] = v.clone();
        }
    }
    let mut msg = lane(
        "in_view",
        hop,
        ctx(),
        view.body.get("messages").cloned().unwrap_or(json!([])),
    );
    if let Some(system) = view.body.get("system") {
        msg["system"] = system.clone();
    }
    msg
}

fn on<'a>(out: &'a [Value], route: &str) -> &'a Value {
    out.iter()
        .find(|m| m["header"]["route"] == route)
        .unwrap_or_else(|| panic!("no {route} in {out:?}"))
}

/// What the route sends to `decide` and the screen judge to `screen_model`.
fn judged(view: &Msg) -> (String, Value, String) {
    let knobs = [("judge_context", json!("1")), ("context_turns", json!(2))];
    let out = run_cell(ROUTE, &knobs, back(view)).0;
    let decide = on(&out, "decide")["decide"].clone();
    let sj = run_cell(SCREEN_JUDGE, &[("enabled", json!("1"))], back(view)).0;
    let screen = on(&sj, "screen_model")["messages"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string();
    (
        decide["state"].as_str().unwrap_or("").to_string(),
        decide["questions"].clone(),
        screen,
    )
}

#[test]
fn gh_d58_judge_a_refused_view_keeps_the_judges_text() {
    let h = Hive::new();
    let questions: Value = serde_json::from_str(TODAY_QUESTIONS).expect("json");

    let door = refused_at_the_door(&h);
    assert_eq!(
        door.hop.get("refused"),
        Some(&json!("no_round")),
        "{door:?}"
    );
    let (state, qs, screen) = judged(&door);
    assert_eq!(
        state, TODAY_DOOR,
        "a view refused at the door is no judge context: the turn alone, as before"
    );
    assert_eq!(qs, questions, "and the questions without a screen question");
    assert_eq!(
        screen, TODAY_DOOR,
        "the screen judge's text is the turn alone too"
    );

    for (name, view, flag, word) in [
        ("no round", later(&h, "", ""), "refused", "no_round"),
        (
            "degraded",
            later(&h, "[\"member:e\"]", "store_degraded"),
            "degraded",
            "store",
        ),
    ] {
        assert_eq!(view.hop.get(flag), Some(&json!(word)), "{name}: {view:?}");
        let (state, qs, screen) = judged(&view);
        assert_eq!(
            state, TODAY_LATER,
            "{name}: a refused or degraded view after the first round gives the \
             moment alone, the bytes of the package of now and place"
        );
        assert_eq!(
            qs, questions,
            "{name}: the questions without a screen question"
        );
        assert_eq!(
            screen, TODAY_LATER,
            "{name}: the screen judge's text likewise"
        );
    }
}
