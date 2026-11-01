//! #1279, the talky side: the route stamp reads the curator's judge
//! context and asks the screen question in the same decide call, a product's
//! `screen_form` choice stands on the stamp, `./screen_judge` sends the form
//! as a second stamp of the same turn, a voice partial's stamp goes to the
//! curator only, and all of it is off as shipped.
//!
//! What this file pins, at the receiver (the cells' scripts) and on the
//! composite's edges:
//!
//! 1. `screen_form` comes from a product choice of that name, or -- with
//!    `./screen_judge` on -- as a second stamp `{turn_id, audience_set,
//!    screen_form}` after a first stamp without it, which leaves the rim only
//!    on `verdict`; a rim stamp that carries `screen` or `screen_form` --
//!    with `judge_context` "1" every rim stamp -- leaves on `stamp` as before
//!    and a second time on `verdict`, the presenter's `in_verdict`;
//! 2. `judge_context` "1" asks the wall with `judge: true`, renders the judge
//!    context and asks `screen.topic`/`screen.lead`/`screen.further` (one each,
//!    candidates as `<topic>/<key>`, #1371);
//!    the stamp carries `screen`, never for `clarify`;
//! 3. a partial (`context.delta` "1") is stamped as `delta_stamp` with
//!    `text_norm` and reaches `./curator`, never the collector or the rim;
//! 4. `clarify` is asked only with its knob;
//! 5. shipped, the decide call and the stamp are today's bytes and the judge
//!    sends nothing.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::{config_of, hop_str, lane, now_ms, run_cell, run_cell_stdout};
use meclaw_colony::cel_eval::{evaluate_condition, parse_condition};
use serde_json::{Value, json};

const TALKY: &str = "templates/talky/config.json";
const ROUTE: &str = "templates/talky/route/config.json";
const JUDGE: &str = "templates/talky/screen_judge/config.json";
const MODEL: &str = "templates/talky/screen_model/config.json";

fn ctx() -> Value {
    json!({"turn_id": "c-1", "channel": "tg:1", "audience_set": "[\"m-1\"]"})
}

fn turn() -> Value {
    lane(
        "in_turn",
        json!({}),
        ctx(),
        json!([{"origin": "user", "type": "text", "text": "How is the weather tomorrow?"}]),
    )
}

// Asked an hour ahead: the duration clamps to 0, so no start time of the
// cell's process ever makes a verdict late (dev-rules § 11).
fn in_time() -> String {
    (now_ms() + 3_600_000).to_string()
}

fn decision(answers: Value, ctx: Value, hop: Value) -> Value {
    let mut h = json!({"finish_reason": "stop", "asked_at": in_time()});
    for (k, v) in hop.as_object().expect("hop") {
        h[k] = v.clone();
    }
    let mut doc = lane("in_decision", h, ctx, json!([]));
    doc["decision"] = json!({"answers": answers, "missing": [], "model": "d"});
    doc
}

fn form_choice() -> Value {
    json!({"screen_form": {
        "question": "How should the screen take part in the answer to this request?",
        "options": {"active": "the screen is the answer",
                    "accompanying": "the screen adds a picture",
                    "referring": "the voice points to the screen",
                    "none": "nothing on the screen helps"},
        "fallback": "unknown"}})
}

fn judge() -> Value {
    json!({
        "turns": [{"turn_id": "c-0", "role": "user", "text": "Good morning."},
                  {"turn_id": "c-0", "role": "assistant", "text": "Good morning to you."}],
        "topic": "the trip on Sunday",
        "screen": {"topics": [
            {"name": "weather", "purpose": "the weather today and the next days",
             "candidates": [{"key": "today", "describe": "today's weather"},
                            {"key": "week", "describe": "the week ahead"}]},
            {"name": "calendar", "purpose": "the person's appointments"}],
            "state": [{"name": "calendar", "focus": true}]},
        "now": {"iso": "2026-10-30T09:00:00+01:00", "weekday": "Friday",
                "timezone": "Europe/Berlin"},
        "place": "Berlin"
    })
}

fn judged_turns() -> Value {
    let mut doc = lane(
        "in_turns",
        json!({"state": "How is the weather tomorrow?", "asked_at": in_time()}),
        ctx(),
        json!([]),
    );
    doc["turns"] = json!([]);
    doc["judge"] = judge();
    doc
}

fn on<'a>(out: &'a [Value], route: &str) -> &'a Value {
    out.iter()
        .find(|m| m["header"]["route"] == route)
        .unwrap_or_else(|| panic!("no {route} in {out:?}"))
}

fn edges() -> Vec<Value> {
    config_of(TALKY)["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .clone()
}

fn edge(edges: &[Value], from: &str, to: &str, needle: &str) -> Value {
    edges
        .iter()
        .find(|e| {
            e["from"] == from
                && e["to"] == to
                && e["condition"].as_str().unwrap_or("").contains(needle)
        })
        .cloned()
        .unwrap_or_else(|| panic!("no edge {from} -> {to} on {needle}"))
}

fn sure() -> Value {
    json!({
        "path": {"choice": "simple", "p": 0.9},
        "intent": {"choice": "question", "p": 0.8},
        "screen.topic": {"choice": "weather", "p": 0.8},
        "screen.lead": {"choice": "weather/week", "p": 0.7},
        "screen.further": {"choice": "weather/today", "p": 0.6},
        "choices.screen_form": {"choice": "accompanying", "p": 0.9}
    })
}

#[test]
fn screen_form_from_judge_when_enabled_else_decider() {
    // The decider: the product's choice `screen_form` stands on the stamp and on its
    // rim copy.
    let over = [("choices", form_choice()), ("stamp_out", json!("1"))];
    let out = run_cell(ROUTE, &over, decision(sure(), ctx(), json!({}))).0;
    assert_eq!(on(&out, "stamp")["stamp"]["screen_form"], "accompanying");
    assert_eq!(
        on(&out, "stamp_out")["stamp"]["screen_form"],
        "accompanying"
    );
    // An error of the decider: the choice's fallback, still a value.
    let failed = lane(
        "in_decision",
        json!({"finish_reason": "error", "error_code": "upstream_timeout",
               "asked_at": in_time()}),
        ctx(),
        json!([]),
    );
    let s = on(&run_cell(ROUTE, &over, failed).0, "stamp")["stamp"].clone();
    assert_eq!(s["screen_form"], "unknown", "{s}");

    // The judge: without the choice the first stamp has no form ...
    let out = run_cell(ROUTE, &[], decision(sure(), ctx(), json!({}))).0;
    let first = &on(&out, "stamp")["stamp"];
    assert!(first.get("screen_form").is_none(), "{first}");
    // ... and the switched-on judge reads the turn's judge context.
    let ask = run_cell(JUDGE, &[("enabled", json!("1"))], turn()).0;
    let ask = on(&ask, "turns_ask");
    assert_eq!(ask["turns_ask"]["judge"], true, "{ask}");
    assert_eq!(hop_str(ask, "state"), "How is the weather tomorrow?");
    assert_eq!(hop_str(ask, "turn_id"), "c-1");
}

#[test]
fn screen_form_follows_as_second_stamp() {
    let en = [("enabled", json!("1"))];
    // The judge context comes back: ONE structured call of `./screen_model`.
    let call = run_cell(JUDGE, &en, judged_turns()).0;
    assert_eq!(call.len(), 1, "{call:?}");
    let call = on(&call, "screen_model");
    let text = call["messages"][0]["text"].as_str().expect("the request");
    assert!(text.contains("Good morning to you."), "{text}");
    assert!(
        text.ends_with("The request now:\nHow is the weather tomorrow?"),
        "{text}"
    );
    let instructions = call["system"]["instructions"]["text"]
        .as_str()
        .expect("instructions");
    for form in ["active", "accompanying", "referring", "none"] {
        assert!(instructions.contains(form), "{instructions}");
    }
    // The model answers structured: its schema allows exactly the four forms.
    let rf = &config_of(MODEL)["params"]["provider_extra"]["response_format"];
    assert_eq!(rf["type"], "json_schema", "{rf}");
    assert_eq!(
        rf["json_schema"]["schema"]["properties"]["screen_form"]["enum"],
        json!(["active", "accompanying", "referring", "none"])
    );
    let answer = lane(
        "in_verdict",
        json!({"finish_reason": "stop"}),
        ctx(),
        json!([{"origin": "assistant", "type": "text",
                "text": "{\"screen_form\": \"referring\"}"}]),
    );
    let out = run_cell(JUDGE, &en, answer.clone()).0;
    assert_eq!(out.len(), 1, "{out:?}");
    let second = on(&out, "verdict");
    assert_eq!(
        second["stamp"],
        json!({"turn_id": "c-1", "audience_set": "[\"m-1\"]", "screen_form": "referring"})
    );
    assert_eq!(hop_str(second, "turn_id"), "c-1", "same turn");
    // An answer that is no form, an error, or no round: nothing.
    let mut bad = answer.clone();
    bad["messages"][0]["text"] = json!("{\"screen_form\": \"loud\"}");
    assert!(run_cell(JUDGE, &en, bad).0.is_empty());
    let mut err = answer.clone();
    err["header"]["hop"]["finish_reason"] = json!("error");
    assert!(run_cell(JUDGE, &en, err).0.is_empty());
    let mut lone = answer;
    lone["header"]["context"]
        .as_object_mut()
        .expect("context")
        .remove("audience_set");
    assert!(run_cell(JUDGE, &en, lone).0.is_empty());

    // It leaves the rim on `verdict` and only there, its wall read takes its
    // own way back.
    let es = edges();
    let exit = edge(&es, "./screen_judge", ".", "'verdict'");
    assert!(exit["modifier"]["set_hop"].is_null(), "{exit}");
    assert_eq!(
        es.iter()
            .filter(|e| e["from"] == "./screen_judge" && e["to"] == ".")
            .count(),
        1,
        "one exit, never `stamp`"
    );
    let to = edge(&es, "./screen_judge", "./curator", "'turns_ask'");
    assert_eq!(
        to["modifier"]["set_context"]["judge_for"], "'screen_judge'",
        "{to}"
    );
    edge(
        &es,
        "./curator",
        "./screen_judge",
        "context.judge_for == 'screen_judge'",
    );
    let back = edge(&es, "./curator", "./route", "'turns'");
    assert!(
        back["condition"]
            .as_str()
            .unwrap_or("")
            .contains("!(has(context.judge_for)"),
        "{back}"
    );
    edge(&es, "./screen_judge", "./screen_model", "'screen_model'");
    edge(
        &es,
        "./screen_model",
        "./screen_judge",
        "refused_subscriber",
    );
    edge(&es, "./session-keeper", "./screen_judge", "'turn'");
}

/// GH #1279: the rim exit `verdict`. A rim stamp of `./route` that carries the
/// screen (`screen_form` or `screen`) is marked `hop.verdict` "1"; with
/// `judge_context` "1" every rim stamp is (clarify, topic `none`, a topic under
/// `threshold`): the presenter in its stamp mode closes such a turn as
/// `no_topic` only when a stamp arrives, else it waits into `timeout`. One rim
/// edge carries it out on `verdict`; the rim route `stamp` stays as it was and
/// drops the hop key.
#[test]
fn route_stamp_with_screen_is_also_the_verdict() {
    let over = [("choices", form_choice()), ("stamp_out", json!("1"))];
    let out = run_cell(ROUTE, &over, decision(sure(), ctx(), json!({}))).0;
    let rim = on(&out, "stamp_out");
    assert_eq!(hop_str(rim, "verdict"), "1", "{rim}");
    let inner = on(&out, "stamp");
    assert!(inner["header"].get("verdict").is_none(), "{inner}");

    // A rim stamp without the screen is no verdict.
    let plain = json!({"path": {"choice": "simple", "p": 0.9},
                       "intent": {"choice": "question", "p": 0.8}});
    let out = run_cell(
        ROUTE,
        &[("stamp_out", json!("1"))],
        decision(plain, ctx(), json!({})),
    )
    .0;
    let rim = on(&out, "stamp_out");
    assert!(rim["header"].get("verdict").is_none(), "{rim}");

    // With the judge context on, every rim stamp is the verdict: a clarify
    // stamp, topic `none` and a topic under `threshold` carry no screen and
    // still reach the presenter.
    let jc = [
        ("judge_context", json!("1")),
        ("clarify", json!("1")),
        ("stamp_out", json!("1")),
    ];
    let mut clarify = sure();
    clarify["path"] = json!({"choice": "clarify", "p": 0.9});
    let mut none = sure();
    none["screen.topic"] = json!({"choice": "none", "p": 0.9});
    let mut unsure = sure();
    unsure["screen.topic"] = json!({"choice": "weather", "p": 0.3});
    for (name, a) in [("clarify", clarify), ("none", none), ("unsure", unsure)] {
        let out = run_cell(ROUTE, &jc, decision(a, ctx(), json!({}))).0;
        let rim = on(&out, "stamp_out");
        assert!(rim["stamp"].get("screen").is_none(), "{name}: {rim}");
        assert_eq!(hop_str(rim, "verdict"), "1", "{name}: {rim}");
    }

    let es = edges();
    let stamp = es
        .iter()
        .find(|e| {
            e["from"] == "./route"
                && e["to"] == "."
                && e["condition"] == "has(hop.route) && hop.route == 'stamp_out'"
        })
        .expect("the rim route `stamp` as before");
    assert_eq!(stamp["modifier"]["set_hop"]["route"], "'stamp'");
    assert_eq!(
        stamp["modifier"]["delete_hop"],
        json!(["verdict"]),
        "the `stamp` contract names no verdict key"
    );
    let verdict = edge(&es, "./route", ".", "has(hop.verdict)");
    assert_eq!(
        verdict["condition"],
        "has(hop.route) && hop.route == 'stamp_out' && has(hop.verdict)"
    );
    assert_eq!(verdict["modifier"]["set_hop"]["route"], "'verdict'");
    assert_eq!(
        verdict["modifier"]["delete_context"], stamp["modifier"]["delete_context"],
        "the same context leaves"
    );

    // The rim declares the exit.
    let emits = config_of(TALKY)["params"]["contract"]["emits"].clone();
    assert!(
        emits
            .as_array()
            .expect("emits")
            .iter()
            .any(|e| e["route"] == "verdict"),
        "{emits}"
    );
}

#[test]
fn route_judge_context_asks_the_screen_question() {
    let jc = [("judge_context", json!("1"))];
    // The wall read asks for the judge context.
    let ask = run_cell(ROUTE, &jc, turn()).0;
    assert_eq!(on(&ask, "turns_ask")["turns_ask"]["judge"], true);

    // The judge context renders before the turn, both sides, the screen, now.
    let out = run_cell(ROUTE, &jc, judged_turns()).0;
    let d = &on(&out, "decide")["decide"];
    let state = d["state"].as_str().expect("state");
    for needle in [
        "- person: Good morning.",
        "- companion: Good morning to you.",
        "The conversation is about: the trip on Sunday",
        "- calendar, in focus",
        "Now: Friday, 2026-10-30T09:00:00+01:00 (Europe/Berlin)",
        "Place: Berlin",
    ] {
        assert!(state.contains(needle), "{needle}: {state}");
    }
    assert!(state.ends_with("The request now:\nHow is the weather tomorrow?"));
    let qs = &d["questions"];
    let topic = &qs["screen.topic"]["options"];
    assert!(topic["weather"].is_string() && topic["calendar"].is_string());
    assert!(topic["none"].is_string(), "{topic}");
    assert!(qs["screen.lead"]["options"]["weather/week"].is_string());
    assert!(qs["screen.further"]["options"]["none"].is_string());
    let lead = qs["screen.lead"]["options"].as_object().expect("lead");
    assert!(
        lead.keys().all(|k| k.starts_with("weather/")),
        "no candidates, no option: {qs}"
    );
    assert!(qs["path"].is_object(), "the same call: {qs}");

    // The stamp carries the verdict.
    let s = on(
        &run_cell(ROUTE, &jc, decision(sure(), ctx(), json!({}))).0,
        "stamp",
    )["stamp"]
        .clone();
    assert_eq!(
        s["screen"],
        json!({"topic": "weather", "confidence": 0.8, "lead": "week", "further": "today"})
    );
    // A lead or further of another topic is no lead (#1371).
    let mut a = sure();
    a["screen.lead"] = json!({"choice": "calendar/day", "p": 0.7});
    let s = on(
        &run_cell(ROUTE, &jc, decision(a, ctx(), json!({}))).0,
        "stamp",
    )["stamp"]
        .clone();
    assert_eq!(s["screen"]["lead"], "", "{s}");
    assert_eq!(s["screen"]["further"], "today", "{s}");
    // `none`, or a topic under `threshold`: no screen.
    for (choice, p) in [("none", 0.9), ("weather", 0.3)] {
        let mut a = sure();
        a["screen.topic"] = json!({"choice": choice, "p": p});
        let s = on(
            &run_cell(ROUTE, &jc, decision(a, ctx(), json!({}))).0,
            "stamp",
        )["stamp"]
            .clone();
        assert!(s.get("screen").is_none(), "{choice} {p}: {s}");
    }
    // `clarify` never carries a screen.
    let mut a = sure();
    a["path"] = json!({"choice": "clarify", "p": 0.9});
    let over = [("judge_context", json!("1")), ("clarify", json!("1"))];
    let s = on(
        &run_cell(ROUTE, &over, decision(a, ctx(), json!({}))).0,
        "stamp",
    )["stamp"]
        .clone();
    assert_eq!(s["path"], "clarify", "{s}");
    assert!(s.get("screen").is_none(), "{s}");
}

#[test]
fn route_asks_clarify_only_with_its_knob() {
    let mut classes = config_of(ROUTE)["params"]["classes"].clone();
    classes["clarify"] = json!("the person has to be asked how it is meant");
    let off = run_cell(ROUTE, &[("classes", classes.clone())], turn()).0;
    let opts = &on(&off, "decide")["decide"]["questions"]["path"]["options"];
    assert!(opts.get("clarify").is_none(), "off: {opts}");
    let with = run_cell(
        ROUTE,
        &[("classes", classes), ("clarify", json!("1"))],
        turn(),
    )
    .0;
    let opts = &on(&with, "decide")["decide"]["questions"]["path"]["options"];
    assert_eq!(
        opts["clarify"],
        "the person has to be asked how it is meant"
    );
    let fallback = run_cell(ROUTE, &[("clarify", json!("1"))], turn()).0;
    let opts = &on(&fallback, "decide")["decide"]["questions"]["path"]["options"];
    assert!(
        opts["clarify"].as_str().is_some_and(|d| !d.is_empty()),
        "{opts}"
    );
}

#[test]
fn delta_stamp_goes_to_the_curator_not_the_collector() {
    let mut dctx = ctx();
    dctx["delta"] = json!("1");
    let mut partial = turn();
    partial["header"]["context"] = {
        let mut c = partial["header"]["context"].clone();
        c["delta"] = json!("1");
        c
    };
    // Without a wall to read, a partial is decided at once, its text on the hop.
    let out = run_cell(ROUTE, &[], partial.clone()).0;
    assert_eq!(out.len(), 1, "{out:?}");
    let call = on(&out, "decide");
    assert_eq!(hop_str(call, "state"), "How is the weather tomorrow?");
    // live-2 #1369: with `context_turns` / `judge_context` a partial asks the wall
    // like a turn (the curator answers it from the round), its text riding along ...
    let over = [("context_turns", json!(3)), ("judge_context", json!("1"))];
    let out = run_cell(ROUTE, &over, partial).0;
    assert_eq!(out.len(), 1, "{out:?}");
    let ask = on(&out, "turns_ask");
    assert_eq!(hop_str(ask, "state"), "How is the weather tomorrow?");
    assert_eq!(ask["turns_ask"]["judge"], json!(true));
    // ... and the answer's decide call keeps the raw text on the hop, so the
    // delta stamp is compared on the words the person said, not the block.
    let mut back = lane(
        "in_turns",
        json!({"state": "How is the weather tomorrow?", "asked_at": in_time()}),
        dctx.clone(),
        json!([]),
    );
    back["turns"] = json!([]);
    back["judge"] = judge();
    let out = run_cell(ROUTE, &over, back).0;
    let call = on(&out, "decide");
    assert_eq!(hop_str(call, "state"), "How is the weather tomorrow?");
    assert_ne!(
        call["decide"]["state"],
        json!("How is the weather tomorrow?"),
        "the block"
    );
    // A turn (no delta) keeps today's hop: no `state` on its decide call.
    let mut plain = lane(
        "in_turns",
        json!({"state": "How is the weather tomorrow?", "asked_at": in_time()}),
        ctx(),
        json!([]),
    );
    plain["turns"] = json!([]);
    plain["judge"] = judge();
    let out = run_cell(ROUTE, &over, plain).0;
    assert!(
        on(&out, "decide")["header"].get("state").is_none(),
        "{out:?}"
    );

    let out = run_cell(
        ROUTE,
        &[("stamp_out", json!("1"))],
        decision(
            sure(),
            dctx,
            json!({"state": "  How is the Weather TOMORROW?!  "}),
        ),
    )
    .0;
    assert_eq!(out.len(), 1, "no rim copy: {out:?}");
    let d = on(&out, "delta_stamp");
    assert_eq!(d["stamp"]["text_norm"], "how is the weather tomorrow");
    assert_eq!(d["stamp"]["turn_id"], "c-1");

    let es = edges();
    let to = edge(&es, "./route", "./curator", "'delta_stamp'");
    assert_eq!(to["modifier"]["set_hop"]["route"], "'in_delta_stamp'");
    for e in &es {
        let c = e["condition"].as_str().unwrap_or("");
        if c.contains("'delta_stamp'") {
            assert_eq!(e["to"], "./curator", "only the curator: {e}");
        }
    }
    for (from, to, needle) in [
        ("./route", "./curator", "'turns_ask'"),
        ("./curator", "./route", "'turns'"),
    ] {
        let e = edge(&es, from, to, needle);
        let dc = e["modifier"]["delete_context"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            !dc.iter().any(|x| x == "delta"),
            "the wall keeps the delta: {e}"
        );
    }
    let door = edge(&es, ".", "./route", "'in_delta'");
    assert_eq!(door["modifier"]["set_hop"]["route"], "'in_turn'");
    assert_eq!(door["modifier"]["set_context"]["delta"], "'1'");
    // Its text rides out and back: its own decide pair.
    let ask = edge(&es, "./route", "./decide", "'decide' && has(hop.state)");
    assert_eq!(ask["modifier"]["set_context"]["route_state"], "hop.state");
    let back = edge(
        &es,
        "./decide",
        "./route",
        "refused_subscriber) && has(context.delta)",
    );
    assert!(back["modifier"]["set_hop"]["state"].is_string(), "{back}");
    // Every exit clears the new interior keys (GH #494).
    for e in es.iter().filter(|e| e["to"] == ".") {
        let dc = e["modifier"]["delete_context"].as_array().expect("exit");
        for k in ["delta", "judge_for"] {
            assert!(dc.iter().any(|x| x == k), "{k} leaks: {e}");
        }
    }
}

// Today's bytes, as `gh1304_no_choices_means_todays_stamp` holds them (master
// 710554c8d): the decide call and a sure stamp without choices.
const TODAY_DECIDE: &str = r#"{"header": {"route": "decide", "asked_at": "<now>"}, "messages": [], "decide": {"state": "How is the weather tomorrow?", "questions": {"path": {"kind": "choice", "instructions": "Which way should this request be answered?", "options": {"complex": "needs careful reasoning, planning or several steps before it can be answered", "research": "needs a deep search over many sources before it can be answered", "simple": "one quick answer: a fact, a short lookup or one tool call, no deliberation", "talk": "small talk, a greeting, a feeling or a reply that needs no fact and no tool"}}, "intent": {"kind": "choice", "instructions": "What does the person want with this request?", "options": {"chat": "to talk, share or be heard", "other": "none of these", "question": "to know something", "task": "to get something done"}}, "needs.reasoning": {"kind": "yes_no", "instructions": "Does answering it need careful step-by-step reasoning?"}, "needs.research": {"kind": "yes_no", "instructions": "Does answering it need a deep search over many sources?"}, "needs.tool": {"kind": "yes_no", "instructions": "Does answering it need a tool call?"}, "families.history": {"kind": "yes_no", "instructions": "Would answering it use earlier conversations with this person?"}, "families.web": {"kind": "yes_no", "instructions": "Would answering it use searching or reading the web?"}}}}"#;
const TODAY_SURE: &str = r#"{"header": {"route": "stamp", "turn_id": "c-1", "path": "simple"}, "messages": [], "stamp": {"turn_id": "c-1", "path": "simple", "p": 0.9, "intent": "question", "needs": [], "families": [], "duration": 0}}"#;

#[test]
fn talky_fu2_defaults_byte_identical() {
    let route = config_of(ROUTE)["params"].clone();
    assert_eq!(route["judge_context"], "0", "{route}");
    assert_eq!(route["clarify"], "0", "{route}");
    assert_eq!(config_of(JUDGE)["params"]["enabled"], "0");

    let call = run_cell_stdout(ROUTE, &[], turn()).0;
    let call = call.trim_end();
    let at = call
        .split("\"asked_at\": \"")
        .nth(1)
        .and_then(|r| r.split('"').next())
        .unwrap_or_else(|| panic!("no asked_at: {call}"));
    let masked = call.replacen(
        &format!("\"asked_at\": \"{at}\""),
        "\"asked_at\": \"<now>\"",
        1,
    );
    // serde_json sorts the options of the parsed config (BTreeMap), so the
    // golden is compared as values, its key order as the shipped strings.
    let got: Value = serde_json::from_str(&masked).expect("json");
    let want: Value = serde_json::from_str(TODAY_DECIDE).expect("json");
    assert_eq!(got, want, "the decide call is today's");

    let plain = json!({"path": {"choice": "simple", "p": 0.9},
                       "intent": {"choice": "question", "p": 0.8},
                       "screen.topic": {"choice": "weather", "p": 0.9}});
    let out = run_cell_stdout(ROUTE, &[], decision(plain, ctx(), json!({}))).0;
    assert_eq!(out.trim_end(), TODAY_SURE, "the stamp is today's bytes");

    // A judge body without the knob: the person's words alone, no screen
    // question.
    let mut back = judged_turns();
    back["turns"] = json!([{"turn_id": "c-0", "text": "Good morning."}]);
    let d = on(&run_cell(ROUTE, &[], back).0, "decide")["decide"].clone();
    assert!(d["questions"].get("screen.topic").is_none(), "{d}");
    assert!(
        d["state"]
            .as_str()
            .unwrap_or("")
            .starts_with("Earlier in this conversation the person said"),
        "{d}"
    );
    // The judge hears every turn and sends nothing: no new rim message.
    assert!(run_cell(JUDGE, &[], turn()).0.is_empty());
    assert!(run_cell(JUDGE, &[], judged_turns()).0.is_empty());
}

/// GH #1279: the set binds at the assistant, so the assistant
/// declares every #1279 lane of talky's rim and carries it through both keepers
/// (a rim edge of one keeper has its twin around the other), the exits with
/// the context `stamp` drops.
#[test]
fn the_assistant_carries_the_fu2_lanes() {
    const ASSISTANT: &str = "templates/assistant/config.json";
    let a = config_of(ASSISTANT);
    assert!(
        a["params"].get("ports").is_none(),
        "the assistant stays open"
    );
    let declared = |side: &str, route: &str| {
        a["params"]["contract"][side]
            .as_array()
            .expect("contract")
            .iter()
            .any(|e| e["route"] == route)
    };
    let es = a["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .clone();
    let stamp_exit = edge(&es, "./talky", ".", "'stamp'");
    for route in [
        "in_delta",
        "in_screen_topics",
        "in_seed_ask",
        "in_seed_line",
    ] {
        assert!(declared("accepts", route), "assistant accepts {route}");
        for surface in ["./talky", "./talky-chat"] {
            let door = edge(&es, ".", surface, &format!("'{route}'"));
            assert!(door.get("modifier").is_none(), "{route}: {door}");
        }
        // Which surfaces one message on this lane reaches, by the channel the
        // round came in on (the edges' own conditions, evaluated).
        let reached = |node: Option<&str>| -> Vec<String> {
            let mut context = serde_json::Map::new();
            if let Some(n) = node {
                context.insert("channel_node".into(), json!(n));
            }
            let mut hop = serde_json::Map::new();
            hop.insert("route".into(), json!(route));
            es.iter()
                .filter(|e| e["from"] == ".")
                .filter(|e| {
                    let src = e["condition"].as_str().unwrap_or("");
                    let c = parse_condition(src).expect("condition parses");
                    evaluate_condition(&c, &context, &hop).unwrap_or(false)
                })
                .map(|e| e["to"].as_str().unwrap_or("").to_string())
                .collect()
        };
        if route == "in_screen_topics" {
            // Both curators keep the topic table: either surface may judge.
            for node in [Some("freeswitch"), Some("chat"), None] {
                assert_eq!(
                    reached(node),
                    ["./talky", "./talky-chat"],
                    "{route} {node:?}"
                );
            }
        } else {
            // One ask, one partial, one line reaches exactly one surface: the
            // one of the channel, like `in_delegation`. Two would answer one
            // seed ask twice and judge one partial twice.
            assert_eq!(reached(Some("freeswitch")), ["./talky"], "{route}");
            assert_eq!(reached(None), ["./talky"], "{route}");
            assert_eq!(reached(Some("chat")), ["./talky-chat"], "{route}");
        }
    }
    for route in ["seed", "live_feed", "verdict", "propose"] {
        assert!(declared("emits", route), "assistant emits {route}");
        for surface in ["./talky", "./talky-chat"] {
            let exit = edge(&es, surface, ".", &format!("'{route}'"));
            assert_eq!(
                exit["modifier"]["delete_context"], stamp_exit["modifier"]["delete_context"],
                "{route} out of {surface}"
            );
        }
    }
    // Every one of them is a lane of talky's rim.
    let t = config_of(TALKY);
    for (side, route) in [
        ("accepts", "in_delta"),
        ("accepts", "in_screen_topics"),
        ("accepts", "in_seed_ask"),
        ("accepts", "in_seed_line"),
        ("emits", "seed"),
        ("emits", "live_feed"),
        ("emits", "verdict"),
        ("emits", "propose"),
    ] {
        assert!(
            t["params"]["contract"][side]
                .as_array()
                .expect("talky contract")
                .iter()
                .any(|e| e["route"] == route),
            "talky {side} {route}"
        );
    }
}

/// GH #1279: the text `./route` stamps a delta with and the text
/// the curator's policy compares a final turn by are one rule -- were they to
/// drift, a delta would never be reused, silently. And the place and the time
/// zone the policy's frame names are the ones the judge context names.
#[test]
fn the_delta_text_rule_and_the_frame_are_one_in_route_and_curator() {
    const POLICY: &str = "templates/curator/policy/config.json";
    const HISTORY: &str = "templates/curator/history/config.json";
    let script = |path: &str| {
        config_of(path)["params"]["script_inline"]
            .as_str()
            .expect("script")
            .to_string()
    };
    // The function alone: its lines up to the first one back at the margin.
    let def = |s: &str, start: &str| -> String {
        let at = s.find(start).unwrap_or_else(|| panic!("no {start}"));
        let mut lines = s[at + 1..].lines();
        let mut out = vec![lines.next().unwrap_or("").to_string()];
        for l in lines {
            if !l.is_empty() && !l.starts_with(' ') {
                break;
            }
            out.push(l.to_string());
        }
        out.join("\n").trim_end().to_string()
    };
    let (r, p) = (script(ROUTE), script(POLICY));
    assert_eq!(def(&r, "\ndef normalize("), def(&p, "\ndef normalize("));
    let line = |s: &str| {
        s.lines()
            .find(|l| l.starts_with("TRAILING = "))
            .expect("TRAILING")
            .to_string()
    };
    assert_eq!(line(&r), line(&p));
    let (pp, hp) = (config_of(POLICY), config_of(HISTORY));
    for k in ["place", "timezone"] {
        assert_eq!(pp["params"][k], hp["params"][k], "{k}");
    }
}

/// live-2 #1370: a set writes its switches as JSON `true` (soul2me 0.45
/// `04-assistant.json`); `str(True)` is "True", so every switch read
/// `== "1"` stayed off without a word. One rule for every switch of the
/// wave, the presenter's `knob()`: JSON true, "1", "true" or "on" is on.
#[test]
fn talky_switches_read_json_true_as_on() {
    let yes = json!(true);
    // screen_judge `enabled`
    let ask = run_cell(JUDGE, &[("enabled", yes.clone())], turn()).0;
    assert_eq!(on(&ask, "turns_ask")["turns_ask"]["judge"], true, "{ask:?}");
    // route `judge_context`
    let ask = run_cell(ROUTE, &[("judge_context", yes.clone())], turn()).0;
    assert_eq!(on(&ask, "turns_ask")["turns_ask"]["judge"], true, "{ask:?}");
    // route `clarify`
    let out = run_cell(ROUTE, &[("clarify", yes.clone())], turn()).0;
    let opts = &on(&out, "decide")["decide"]["questions"]["path"]["options"];
    assert!(
        opts["clarify"].as_str().is_some_and(|d| !d.is_empty()),
        "{opts}"
    );
    // route `stamp_out`
    let plain = json!({"path": {"choice": "simple", "p": 0.9},
                       "intent": {"choice": "question", "p": 0.8}});
    let out = run_cell(
        ROUTE,
        &[("stamp_out", yes.clone())],
        decision(plain.clone(), ctx(), json!({})),
    )
    .0;
    on(&out, "stamp_out");
    // The words of the presenter's rule, and nothing beyond them.
    for w in ["on", "true", " 1 "] {
        let out = run_cell(
            ROUTE,
            &[("stamp_out", json!(w))],
            decision(plain.clone(), ctx(), json!({})),
        )
        .0;
        on(&out, "stamp_out");
    }
    for w in [
        json!(false),
        json!("0"),
        json!("off"),
        json!("yes"),
        json!(""),
    ] {
        let out = run_cell(
            ROUTE,
            &[("stamp_out", w.clone())],
            decision(plain.clone(), ctx(), json!({})),
        )
        .0;
        assert!(
            out.iter().all(|m| m["header"]["route"] != "stamp_out"),
            "{w} is off: {out:?}"
        );
    }
}

/// #1371: the screen question does not grow with the topics. 30 topics with three
/// candidates each: one `screen.topic`, one `screen.lead`, one `screen.further` --
/// the decide call stays within the decisions wire (the llm cell's
/// `translate_decisions` refuses more than 64 questions), every option named.
#[test]
fn many_screen_topics_ask_three_screen_questions() {
    let topics: Vec<Value> = (0..30)
        .map(|i| {
            json!({"name": format!("app{i:02}"), "purpose": format!("what app {i} shows"),
                   "candidates": [{"key": "a", "describe": "first view"},
                                  {"key": "b", "describe": "second view"},
                                  {"key": "c", "describe": "third view"}]})
        })
        .collect();
    let mut doc = judged_turns();
    doc["judge"]["screen"]["topics"] = Value::Array(topics);
    let jc = [("judge_context", json!("1"))];
    let out = run_cell(ROUTE, &jc, doc).0;
    let qs = on(&out, "decide")["decide"]["questions"]
        .as_object()
        .expect("questions")
        .clone();
    let screen: Vec<&String> = qs.keys().filter(|k| k.starts_with("screen.")).collect();
    assert_eq!(screen.len(), 3, "{screen:?}");
    assert!(qs.len() <= 64, "{} questions", qs.len());
    assert_eq!(qs["screen.topic"]["options"].as_object().unwrap().len(), 31);
    assert_eq!(qs["screen.lead"]["options"].as_object().unwrap().len(), 90);
    assert_eq!(
        qs["screen.further"]["options"].as_object().unwrap().len(),
        91
    );
}
