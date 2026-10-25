//! GH #1304 -- the route stamp asks a product's own choices, falls back per
//! choice, carries a choice's note into the same turn, and can leave talky.
//!
//! WHY this file exists: `talky/route` asked a fixed question set (`path`,
//! `intent`, one yes/no per need and family), and an unsure path, an error or a
//! late answer emptied everything but the fallback path. A product that needs
//! its own question answered on every turn (is this turn about X?) had no
//! place for it, no declared value when the decider failed, and no way to get
//! the stamp out of the composite. What this file pins, at the receiver:
//!
//! 1. `choices` are asked in the SAME decide call as `path`, the fallback of a
//!    choice is never offered to the model;
//! 2. each choice falls back on its own: an unsure path keeps the choice, an
//!    error or a late stamp gives every choice its declared fallback;
//! 3. a chosen value with a note puts the note on the stamp as a turn note,
//!    the collector carries it to the seam and talky lifts it into
//!    `context.turn_notes` on its curate edge -- the same turn, no round trip;
//! 4. `stamp_out` (off as shipped) sends the stamp out of talky without text
//!    and without the notes, through talky and through the assistant rim;
//! 5. without `choices` the stamp and the decide call are today's bytes;
//! 6. `context_turns` (0 as shipped) asks the curator's wall for the person's
//!    last turns before the decide call and puts them in a block before the
//!    current turn.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::{
    ASSEMBLE, assemble, bundle_reply, config_of, hop_str, lane, now_ms, run_cell, run_cell_stdout,
};
use serde_json::{Value, json};

const TALKY: &str = "templates/talky/config.json";
const ROUTE: &str = "templates/talky/route/config.json";
const ASSISTANT: &str = "templates/assistant/config.json";

fn choices() -> Value {
    json!({
        "topic": {
            "question": "Is the person talking about their health?",
            "options": {"health": "about their own health",
                        "other": "about anything else"},
            "fallback": "unknown",
            "notes": {"health": "Ask how they feel before you give advice."}
        }
    })
}

fn turn() -> Value {
    lane(
        "in_turn",
        json!({}),
        json!({"turn_id": "c-1", "channel": "tg:1", "audience_set": "[\"m-1\"]"}),
        json!([{"origin": "user", "type": "text", "text": "My knee hurts again."}]),
    )
}

fn decision(asked_at: i64, answers: Value) -> Value {
    let mut doc = lane(
        "in_decision",
        json!({"finish_reason": "stop", "asked_at": asked_at.to_string()}),
        json!({"turn_id": "c-1", "channel": "tg:1", "audience_set": "[\"m-1\"]"}),
        json!([]),
    );
    doc["decision"] = json!({"answers": answers, "missing": [], "model": "d"});
    doc
}

fn sure() -> Value {
    json!({
        "path": {"choice": "talk", "p": 0.9},
        "intent": {"choice": "chat", "p": 0.8},
        "choices.topic": {"choice": "health", "p": 0.85}
    })
}

fn on<'a>(out: &'a [Value], route: &str) -> &'a Value {
    out.iter()
        .find(|m| m["header"]["route"] == route)
        .unwrap_or_else(|| panic!("no {route} in {out:?}"))
}

fn keys(v: &Value) -> Vec<String> {
    let mut k: Vec<String> = v.as_object().expect("object").keys().cloned().collect();
    k.sort_unstable();
    k
}

fn edge<'a>(edges: &'a [Value], from: &str, to: &str, needle: &str) -> &'a Value {
    edges
        .iter()
        .find(|e| {
            e["from"] == from
                && e["to"] == to
                && e["condition"].as_str().unwrap_or("").contains(needle)
        })
        .unwrap_or_else(|| panic!("no edge {from} -> {to} on {needle}"))
}

fn edges_of(rel: &str) -> Vec<Value> {
    config_of(rel)["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .clone()
}

// Asked an hour ahead: the duration clamps to 0, so no start time of the
// cell's process ever makes a verdict late (dev-rules § 11).
fn in_time() -> i64 {
    now_ms() + 3_600_000
}

#[test]
fn gh1304_a_product_choice_is_asked_in_the_same_decide_call() {
    let out = run_cell(ROUTE, &[("choices", choices())], turn()).0;
    assert_eq!(out.len(), 1, "one call, all questions: {out:?}");
    let qs = &on(&out, "decide")["decide"]["questions"];
    assert!(qs["path"].is_object(), "{qs}");
    assert_eq!(qs["choices.topic"]["kind"], "choice", "{qs}");
    assert_eq!(
        qs["choices.topic"]["instructions"],
        "Is the person talking about their health?"
    );
}

#[test]
fn gh1304_the_fallback_is_never_offered_to_the_model() {
    let out = run_cell(ROUTE, &[("choices", choices())], turn()).0;
    let opts = &on(&out, "decide")["decide"]["questions"]["choices.topic"]["options"];
    assert_eq!(keys(opts), ["health", "other"], "{opts}");
    // A choice with one option, or one whose fallback is an option, is no
    // question the wire takes (`decide_invalid`): it is not asked.
    let mut bad = choices();
    bad["one"] = json!({"question": "q?", "options": {"a": "a"}, "fallback": "z"});
    bad["inside"] = json!({"question": "q?", "options": {"a": "a", "b": "b"},
                           "fallback": "a"});
    let out = run_cell(ROUTE, &[("choices", bad)], turn()).0;
    let qs = &on(&out, "decide")["decide"]["questions"];
    assert!(qs.get("choices.one").is_none(), "{qs}");
    assert!(qs.get("choices.inside").is_none(), "{qs}");
}

#[test]
fn gh1304_an_unsure_path_keeps_the_choice() {
    let mut a = sure();
    a["path"] = json!({"choice": "complex", "p": 0.2});
    let out = run_cell(ROUTE, &[("choices", choices())], decision(in_time(), a)).0;
    let s = &on(&out, "stamp")["stamp"];
    assert_eq!(s["path"], "talk", "{s}");
    assert_eq!(
        s["choices"]["topic"],
        json!({"value": "health", "p": 0.85, "source": "model"})
    );
}

#[test]
fn gh1304_a_choice_under_its_threshold_takes_the_fallback() {
    let mut c = choices();
    c["topic"]["threshold"] = json!(0.9);
    let s = on(
        &run_cell(ROUTE, &[("choices", c)], decision(in_time(), sure())).0,
        "stamp",
    )["stamp"]
        .clone();
    assert_eq!(s["choices"]["topic"]["value"], "unknown", "{s}");
    assert_eq!(s["choices"]["topic"]["source"], "fallback", "{s}");
    assert_eq!(
        s["turn_notes"],
        json!([]),
        "a fallback carries no note: {s}"
    );
}

#[test]
fn gh1304_an_error_stamp_takes_the_declared_fallback() {
    let mut failed = lane(
        "in_decision",
        json!({"finish_reason": "error", "error_code": "upstream_timeout",
               "asked_at": now_ms().to_string()}),
        json!({"turn_id": "c-1"}),
        json!([]),
    );
    failed["detail"] = json!("the decider timed out");
    let s = on(
        &run_cell(ROUTE, &[("choices", choices())], failed).0,
        "stamp",
    )["stamp"]
        .clone();
    assert_eq!(
        s["choices"]["topic"],
        json!({"value": "unknown", "p": 0.0, "source": "fallback"}),
        "never empty: {s}"
    );
    assert_eq!(s["turn_notes"], json!([]), "{s}");
}

#[test]
fn gh1304_a_late_stamp_takes_the_declared_fallback() {
    let out = run_cell(
        ROUTE,
        &[("choices", choices())],
        decision(now_ms() - 600_000, sure()),
    )
    .0;
    let s = &on(&out, "stamp")["stamp"];
    assert_eq!(s["choices"]["topic"]["value"], "unknown", "{s}");
    assert_eq!(s["turn_notes"], json!([]), "late: the round is gone: {s}");
}

#[test]
fn gh1304_a_choice_note_reaches_the_window_in_the_same_turn() {
    // route: the chosen value's note is a turn note of the stamp
    let out = run_cell(
        ROUTE,
        &[("choices", choices())],
        decision(in_time(), sure()),
    )
    .0;
    let s = on(&out, "stamp")["stamp"].clone();
    assert_eq!(
        keys(&s),
        [
            "choices",
            "duration",
            "families",
            "intent",
            "needs",
            "p",
            "path",
            "turn_id",
            "turn_notes"
        ],
        "nine keys with choices: {s}"
    );
    let note = json!({"id": "route.topic", "source": "route.topic",
                      "text": "Ask how they feel before you give advice.",
                      "scope": "turn", "priority": 0});
    assert_eq!(s["turn_notes"], json!([note.clone()]), "{s}");

    // collector: the route leg keeps it, the seam carries it
    const T: &str = "route-channel#0001";
    const FRESH: &str = "2999-01-01T00:00:00.000000Z";
    let leg = |role: &str, payload: Value| {
        json!({"turn_id": T, "iter": 0, "role": role, "turn": payload.to_string(),
               "fired": 0, "recorded_at": FRESH})
    };
    let window = leg(
        "leg-window",
        json!({"turns": [{"role": "user", "text": "My knee hurts again.",
                          "consult_id": ""}],
               "deferred": 0, "deferred_turns": []}),
    );
    let parked = run_cell(
        "templates/collector/assemble/config.json",
        &[("route_wait_ms", json!(150))],
        {
            let mut d = lane("in_route", json!({"turn_id": T}), json!({}), json!([]));
            d["stamp"] = s.clone();
            d
        },
    )
    .0;
    let row = serde_json::to_string(&parked).expect("json");
    assert!(row.contains("turn_notes"), "the leg keeps the notes: {row}");
    // No memory tier: the stamp's path is `talk`, which is not in
    // `route_no_recall`, so with a tier the fan-in waits for a memory leg this
    // read does not carry and parks (fix round 1: the seam was `[]`).
    let seams = assemble(
        &[("route_wait_ms", json!(150))],
        bundle_reply(
            "collect",
            T,
            &[(
                "c-collect-read",
                json!([window.clone(), leg("leg-route", s.clone())]),
            )],
        ),
    );
    let seam = on(&seams, "curate");
    let carried: Value =
        serde_json::from_str(&hop_str(seam, "turn_notes")).expect("turn_notes is JSON");
    assert_eq!(carried, json!([note]), "{seam}");
    // without a stamp's notes the key is present and empty, like every key
    // of the head (a CEL modifier that reads a missing key fails)
    let bare = assemble(
        &[("route_wait_ms", json!(150))],
        bundle_reply(
            "collect",
            T,
            &[(
                "c-collect-read",
                json!([window, leg("leg-route", json!({"path": "talk"}))]),
            )],
        ),
    );
    assert_eq!(on(&bare, "curate")["header"]["turn_notes"], json!(""));
    let hop = &config_of(ASSEMBLE)["contract"]["emits"]["hop"];
    assert_eq!(hop["turn_notes"]["type"], "string", "{hop}");

    // talky: the curate edge lifts it into the context
    let edges = edges_of(TALKY);
    let curate = edge(&edges, "./collector", "./curator", "'curate'");
    assert_eq!(
        curate["modifier"]["set_context"]["turn_notes"],
        "has(hop.turn_notes) ? hop.turn_notes : ''",
        "{curate}"
    );
}

#[test]
fn gh1304_stamp_out_off_sends_nothing() {
    let out = run_cell(
        ROUTE,
        &[("choices", choices())],
        decision(in_time(), sure()),
    )
    .0;
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(config_of(ROUTE)["params"]["stamp_out"], "0");
}

#[test]
fn gh1304_the_stamp_leaves_talky_without_text() {
    let out = run_cell(
        ROUTE,
        &[("choices", choices()), ("stamp_out", json!("1"))],
        decision(in_time(), sure()),
    )
    .0;
    assert_eq!(out.len(), 2, "{out:?}");
    let rim = on(&out, "stamp_out");
    let s = &rim["stamp"];
    assert_eq!(
        keys(s),
        [
            "audience_set",
            "channel",
            "choices",
            "duration",
            "intent",
            "path",
            "session_id",
            "turn_id"
        ],
        "{s}"
    );
    assert_eq!(s["audience_set"], "[\"m-1\"]");
    assert_eq!(s["channel"], "tg:1");
    assert!(!s["session_id"].as_str().unwrap_or("").is_empty(), "{s}");
    let text = serde_json::to_string(rim).expect("json");
    assert!(!text.contains("Ask how they feel"), "no note text: {text}");
    assert!(!text.contains("knee"), "no turn text: {text}");

    // talky carries it out on `stamp`, and the rim declares it
    let edges = edges_of(TALKY);
    let exit = edge(&edges, "./route", ".", "'stamp_out'");
    assert_eq!(exit["modifier"]["set_hop"]["route"], "'stamp'", "{exit}");
    let talky = config_of(TALKY);
    let emits = talky["params"]["contract"]["emits"]
        .as_array()
        .expect("emits");
    assert!(
        emits.iter().any(|e| e["route"] == "stamp"),
        "talky declares stamp"
    );

    // the assistant carries it out of both surfaces
    let a = edges_of(ASSISTANT);
    for surface in ["./talky", "./talky-chat"] {
        edge(&a, surface, ".", "'stamp'");
    }
    let assistant = config_of(ASSISTANT);
    let emits = assistant["params"]["contract"]["emits"]
        .as_array()
        .expect("emits");
    assert!(
        emits.iter().any(|e| e["route"] == "stamp"),
        "assistant declares stamp"
    );
}

// Fail-closed (review M-2): the rim stamp names the round it belongs to, and the
// reader outside assigns it by `audience_set`. Without a round there is nothing
// to assign, so the rim copy is not raised at all -- the stamp inside talky still
// goes, the turn is not held up.
#[test]
fn gh1304_the_rim_stamp_needs_a_round() {
    // `[]` is a round (the person alone); absent or empty is none.
    for round in [None, Some(json!(""))] {
        let mut doc = decision(in_time(), sure());
        match &round {
            None => {
                doc["header"]["context"]
                    .as_object_mut()
                    .expect("context")
                    .remove("audience_set");
            }
            Some(v) => doc["header"]["context"]["audience_set"] = v.clone(),
        }
        let out = run_cell(
            ROUTE,
            &[("choices", choices()), ("stamp_out", json!("1"))],
            doc,
        )
        .0;
        assert_eq!(out.len(), 1, "round {round:?}: only the stamp: {out:?}");
        on(&out, "stamp");
    }
}

// Today's bytes (GH #1304, review I-1): what the shipped route wrote on master
// 710554c8d for the same three messages, taken from its stdout as written --
// key order, one emission rather than a list. The decide call's `asked_at` is
// the moment of the ask, so it is checked as a number and then masked.
const TODAY_DECIDE: &str = r#"{"header": {"route": "decide", "asked_at": "<now>"}, "messages": [], "decide": {"state": "My knee hurts again.", "questions": {"path": {"kind": "choice", "instructions": "Which way should this request be answered?", "options": {"complex": "needs careful reasoning, planning or several steps before it can be answered", "research": "needs a deep search over many sources before it can be answered", "simple": "one quick answer: a fact, a short lookup or one tool call, no deliberation", "talk": "small talk, a greeting, a feeling or a reply that needs no fact and no tool"}}, "intent": {"kind": "choice", "instructions": "What does the person want with this request?", "options": {"chat": "to talk, share or be heard", "other": "none of these", "question": "to know something", "task": "to get something done"}}, "needs.reasoning": {"kind": "yes_no", "instructions": "Does answering it need careful step-by-step reasoning?"}, "needs.research": {"kind": "yes_no", "instructions": "Does answering it need a deep search over many sources?"}, "needs.tool": {"kind": "yes_no", "instructions": "Does answering it need a tool call?"}, "families.history": {"kind": "yes_no", "instructions": "Would answering it use earlier conversations with this person?"}, "families.web": {"kind": "yes_no", "instructions": "Would answering it use searching or reading the web?"}}}}"#;
const TODAY_SURE: &str = r#"{"header": {"route": "stamp", "turn_id": "c-1", "path": "talk"}, "messages": [], "stamp": {"turn_id": "c-1", "path": "talk", "p": 0.9, "intent": "chat", "needs": [], "families": [], "duration": 0}}"#;
const TODAY_UNSURE: &str = r#"{"header": {"route": "stamp", "turn_id": "c-1", "path": "talk", "fallback": "unsure"}, "messages": [], "stamp": {"turn_id": "c-1", "path": "talk", "p": 0.0, "intent": "task", "needs": [], "families": [], "duration": 0}}"#;

#[test]
fn gh1304_no_choices_means_todays_stamp() {
    let shipped = config_of(ROUTE)["params"].clone();
    assert_eq!(shipped["choices"], json!({}), "{shipped}");
    assert_eq!(shipped["context_turns"], json!(0), "{shipped}");
    assert_eq!(shipped["stamp_out"], json!("0"), "{shipped}");

    let call = run_cell_stdout(ROUTE, &[], turn()).0;
    let call = call.trim_end();
    let at = call
        .split("\"asked_at\": \"")
        .nth(1)
        .and_then(|r| r.split('"').next())
        .unwrap_or_else(|| panic!("no asked_at: {call}"));
    assert!(
        !at.is_empty() && at.bytes().all(|b| b.is_ascii_digit()),
        "asked_at is the moment in ms: {call}"
    );
    let masked = call.replacen(
        &format!("\"asked_at\": \"{at}\""),
        "\"asked_at\": \"<now>\"",
        1,
    );
    assert_eq!(
        masked, TODAY_DECIDE,
        "the decide call without choices is today's bytes"
    );

    let sure = json!({
        "path": {"choice": "talk", "p": 0.9},
        "intent": {"choice": "chat", "p": 0.8}
    });
    let out = run_cell_stdout(ROUTE, &[], decision(in_time(), sure)).0;
    assert_eq!(
        out.trim_end(),
        TODAY_SURE,
        "a sure stamp without choices is today's bytes"
    );

    let unsure = json!({
        "path": {"choice": "complex", "p": 0.3},
        "intent": {"choice": "task", "p": 0.8}
    });
    let out = run_cell_stdout(ROUTE, &[], decision(in_time(), unsure)).0;
    assert_eq!(
        out.trim_end(),
        TODAY_UNSURE,
        "a fallback stamp without choices is today's bytes"
    );
}

#[test]
fn gh1304_context_turns_ask_the_wall_before_the_decide_call() {
    let over = [
        ("context_turns", json!(3)),
        ("context_budget_tokens", json!(9000)),
    ];
    let out = run_cell(ROUTE, &over, turn()).0;
    assert_eq!(out.len(), 1, "{out:?}");
    let ask = on(&out, "turns_ask");
    assert_eq!(
        ask["turns_ask"],
        json!({"n": 3, "budget_tokens": 4000}),
        "capped: {ask}"
    );
    assert_eq!(hop_str(ask, "state"), "My knee hurts again.");
    let asked_at = hop_str(ask, "asked_at");
    assert!(!asked_at.is_empty(), "{ask}");

    // the wall's answer comes back: the earlier words before the turn
    let mut back = lane(
        "in_turns",
        json!({"state": "My knee hurts again.", "asked_at": asked_at}),
        json!({"turn_id": "c-1"}),
        json!([]),
    );
    back["turns"] = json!([{"turn_id": "c-0", "text": "I went hiking on Sunday."}]);
    let out = run_cell(ROUTE, &over, back).0;
    let d = &on(&out, "decide")["decide"];
    let state = d["state"].as_str().expect("state");
    let earlier = state
        .find("I went hiking on Sunday.")
        .expect("earlier turn");
    let now = state.find("My knee hurts again.").expect("current turn");
    assert!(earlier < now, "the block stands before the turn: {state}");
    assert_eq!(
        hop_str(&out[0], "asked_at"),
        hop_str(ask, "asked_at"),
        "{out:?}"
    );

    // an empty or broken wall: the call goes on with the turn alone
    let mut empty = lane(
        "in_turns",
        json!({"state": "My knee hurts again.", "asked_at": now_ms().to_string(),
               "degraded": "store"}),
        json!({"turn_id": "c-1"}),
        json!([]),
    );
    empty["turns"] = json!([]);
    let d = on(&run_cell(ROUTE, &over, empty).0, "decide")["decide"].clone();
    assert_eq!(d["state"], "My knee hurts again.", "{d}");

    // talky draws the read hop through the curator and carries the text
    let edges = edges_of(TALKY);
    let to = edge(&edges, "./route", "./curator", "'turns_ask'");
    assert_eq!(to["modifier"]["set_hop"]["route"], "'in_turns_ask'", "{to}");
    assert!(
        to["modifier"]["set_context"]["route_state"].is_string(),
        "{to}"
    );
    let from = edge(&edges, "./curator", "./route", "'turns'");
    assert_eq!(from["modifier"]["set_hop"]["route"], "'in_turns'", "{from}");
}
