//! D54g stamp (GH #1506, contract points 1 to 5, 8 and 9) -- the early router
//! acts only when it is sure; a second judge sharpens an unsure stamp.
//!
//! WHY this file exists: `talky/route` asks a small decider once per turn and
//! stamps what it said. One bar (`threshold`, 0.6) decided whether the class
//! stood, and a class that asks the person back (`clarify`) was stamped by
//! that first verdict alone. Measured on a live turn: a plain request was
//! stamped `clarify` with `p` 0.70 -- above the bar, so nothing fell back --
//! and the speaker asked the person back instead of doing what was asked.
//! A question back on a clear sentence tells the person they were not
//! understood.
//!
//! The rule these pins hold: every statement of the stamp that steers the
//! conversation has a bar of its own (`sure`); a statement the composition
//! names under `sharpen` that stays under its bar goes to a second judge, a
//! model that takes a window and orders its own view; the class that asks
//! the person back is stamped only when that second judge names it surely;
//! without a second judge the unsure statement is not stamped at all. The
//! composition may hand the router a list of moves (`moves`), asked in the
//! same decide call. Shipped, none of it is set and the stamp is the bytes it
//! was.
//!
//! THE NAMES THESE PINS FIX (strand ruling OR-d54g.stamp.1; one block, so a
//! builder who needs another name changes it here and nowhere else):
//!
//! - params of `talky/route`: `sure` (`{<statement>: bar}`, statements
//!   `path`, `move` and `second` -- the bar of the second judge's own word),
//!   `sharpen` (the statements that go to the second judge), `second_judge`
//!   (the route the call to the second judge leaves on; "" = none),
//!   `second_wait_ms` (how long a sharpened turn waits for it), `moves`
//!   (`{<name>: when it fits}`);
//! - lanes of `talky/route`: `in_second_view` (the view the second judge
//!   ordered) and `in_second` (its answer);
//! - the stateless cell carries what it needs on the hop: every header key
//!   of an emission comes back on the hop of the lane that answers it (the
//!   composite's edges lift them through the context, as `route_asked_at`
//!   and `route_state` travel today). The pins hand them back unread; the
//!   one key they name is `first`, the first verdict;
//! - the second judge answers with one JSON object,
//!   `{"path": {"choice", "p"}, "move"?: {"choice", "p"}}`;
//! - the stamp may carry `move` (a name of `moves`) and `trail`
//!   (`{first, p, fallback, sharpened, second, second_ms}`, the ledger's
//!   material, never the words of the person).
//!
//! Runs the shipped script of the cell (`support/assemble_cell.rs`); the
//! decider and the second judge are fixtures, no model is called.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::{
    assemble, config_of, in_phase, lane, now_ms, run_cell, run_cell_stdout, talky_view_of,
};
use serde_json::{Value, json};

const ROUTE: &str = "templates/talky/route/config.json";

const SURE: &str = "sure";
const SHARPEN: &str = "sharpen";
const SECOND: &str = "second_judge";
const SECOND_WAIT: &str = "second_wait_ms";
const MOVES: &str = "moves";
const IN_SECOND_VIEW: &str = "in_second_view";
const IN_SECOND: &str = "in_second";
const FIRST: &str = "first";

/// The route a composition of these pins gives its second judge.
const JUDGE: &str = "judge_two";
/// The class that asks the person back (the route's knob `clarify`).
const ASKING: &str = "clarify";
/// The class a stamp without a judged class leaves on (`fallback_class`).
const FALLBACK: &str = "talk";

/// A plain request: one reading, one thing to do.
const TEXT: &str = "Please put the drive to my mother in the calendar for two o'clock.";

/// The turn of every fixture (`ctx()`), also the key of its round rows.
const TURN: &str = "c-1";
/// A round row recorded "now" for any `route_wait_ms`, and one older than any:
/// no clock in the test (the form of `assemble/gh1244...`).
const FRESH: &str = "2999-01-01T00:00:00.000000Z";
const OLD: &str = "2000-01-01T00:00:00.000000Z";
/// The same two moments in ms since the epoch, for a hop or a row that carries
/// a moment as a number: one that is over for any bound, one that is ahead of
/// any clock.
const FRESH_MS: i64 = 32_472_144_000_000;
const OLD_MS: i64 = 946_684_800_000;
/// The round row a notice on `in_route` is parked as.
const HOLD: &str = "route-hold";

type Knob = (&'static str, Value);

/// A collector that waits for the stamp (`route_wait_ms` above 0).
fn waiting() -> Vec<Knob> {
    vec![("memory_tier", json!("1")), ("route_wait_ms", json!(150))]
}

/// One round row of the turn as the store hands it back.
fn leg(role: &str, payload: Value, recorded_at: &str) -> Value {
    json!({"turn_id": TURN, "iter": 0, "role": role, "turn": payload.to_string(),
           "fired": 0, "recorded_at": recorded_at})
}

fn window(recorded_at: &str) -> Value {
    leg(
        "leg-window",
        json!({"turns": [{"role": "user", "text": TEXT, "consult_id": ""}],
               "deferred": 0, "deferred_turns": []}),
        recorded_at,
    )
}

fn memory() -> Value {
    leg("leg-memory", json!({"system": {}, "messages": []}), FRESH)
}

/// The collector's fan-in over the round rows `rows`.
fn collect(rows: Value) -> Vec<Value> {
    assemble(
        &waiting(),
        assemble_cell::bundle_reply("collect", TURN, &[("c-collect-read", rows)]),
    )
}

/// The seam of a fan-in that fired, None while the turn waits.
fn seam(out: &[Value]) -> Option<&Value> {
    out.iter().find(|m| {
        let r = m["header"]["route"].as_str();
        r == Some("curate") || r == Some("answer")
    })
}

/// The stamp `msg` of the route cell as the composite hands it to the
/// collector's `in_route` (the stamp in the body, its header keys on the hop),
/// and the leg the collector parks for it.
fn parked(msg: &Value) -> Value {
    let mut hop = msg["header"].clone();
    hop.as_object_mut().expect("header").remove("route");
    let mut doc = lane("in_route", hop, ctx(), json!([]));
    doc["stamp"] = msg["stamp"].clone();
    let out = assemble(&waiting(), doc);
    let row = assemble_cell::call(in_phase(&out, "collect"), "c-collect-row")["row"].clone();
    assert_eq!(row["role"], json!("leg-route"), "{out:?}");
    serde_json::from_str(row["turn"].as_str().expect("the leg's payload")).expect("json")
}

fn ctx() -> Value {
    json!({"turn_id": "c-1", "channel": "tg:1", "audience_set": "[\"m-1\"]"})
}

fn turn() -> Value {
    lane(
        "in_turn",
        json!({}),
        ctx(),
        json!([{"origin": "user", "type": "text", "text": TEXT}]),
    )
}

// Asked an hour ahead: the duration clamps to 0, so no start time of the
// cell's process ever makes a verdict late (dev-rules § 11).
fn in_time() -> String {
    (now_ms() + 3_600_000).to_string()
}

/// The first verdict: `path` with `p`, and what the decider said beside it.
fn first(path: &str, p: f64) -> Value {
    json!({"path": {"choice": path, "p": p},
           "intent": {"choice": "task", "p": 0.86},
           "needs.tool": {"yes": 0.8},
           "families.history": {"yes": 0.69}})
}

fn with_move(mut answers: Value, name: &str, p: f64) -> Value {
    answers["move"] = json!({"choice": name, "p": p});
    answers
}

/// The decider's answer as the composite hands it back: the edge
/// `./decide -> ./route` of talky sets `hop.state` from `context.route_state`,
/// so the request of the turn is on the hop of `in_decision` (the stateless
/// cell has it from nowhere else).
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

/// A composition that sharpens: bars of 0.9, the class sharpened, a second
/// judge and its wait bound; `more` on top (a later entry wins).
fn knobs(more: &[Knob]) -> Vec<Knob> {
    let mut all = vec![
        (SURE, json!({"path": 0.9, "move": 0.9, "second": 0.9})),
        (SHARPEN, json!(["path"])),
        (SECOND, json!(JUDGE)),
        (SECOND_WAIT, json!(5000)),
    ];
    all.extend(more.iter().cloned());
    all
}

fn moves() -> Value {
    json!({"mirror": "the person shares a feeling and wants to be heard, not advised",
           "steady": "the person is upset and needs calm, short sentences"})
}

/// The entries of an object, owned.
fn obj_of(v: &Value) -> Vec<(String, Value)> {
    v.as_object()
        .map(|o| o.iter().map(|(k, x)| (k.clone(), x.clone())).collect())
        .unwrap_or_default()
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

fn fallback_of(out: &[Value]) -> String {
    on(out, "stamp")["header"]["fallback"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

/// What the composite's edges bring back to the cell: every header key of
/// `msg` but its route, on the hop of the answering lane, and the moment it
/// was asked.
fn carried(msg: &Value, asked_at: String) -> Value {
    let mut hop = json!({});
    for (k, v) in msg["header"].as_object().expect("header") {
        if k != "route" {
            hop[k.as_str()] = v.clone();
        }
    }
    hop["asked_at"] = json!(asked_at);
    hop
}

/// The judge context of the wall: one earlier turn of both sides, its topic,
/// the moment and the place.
fn wall() -> Value {
    json!({
        "turns": [{"turn_id": "c-0", "role": "user", "text": "Good morning."},
                  {"turn_id": "c-0", "role": "assistant", "text": "Good morning to you."}],
        "topic": "the trip on Sunday",
        "screen": {"topics": []},
        "now": {"iso": "2026-10-30T09:00:00+01:00", "weekday": "Friday",
                "timezone": "Europe/Berlin"},
        "place": "Berlin"
    })
}

/// The view the second judge ordered with `ask`, back at the cell.
fn view_back(ask: &Value) -> Value {
    let (messages, system) = talky_view_of(&wall());
    let mut doc = lane(IN_SECOND_VIEW, carried(ask, in_time()), ctx(), messages);
    doc["system"] = system;
    doc
}

/// The second judge's answer to `call`, back at the cell: `text` as the one
/// assistant message, `more` on the hop beside what the call carried.
fn verdict_back(call: &Value, more: Value, text: &str, asked_at: String) -> Value {
    let mut hop = carried(call, asked_at);
    for (k, v) in more.as_object().expect("hop") {
        hop[k.as_str()] = v.clone();
    }
    lane(
        IN_SECOND,
        hop,
        ctx(),
        json!([{"origin": "assistant", "type": "text", "text": text}]),
    )
}

/// The second judge's word for the class.
fn second_says(path: &str, p: f64) -> Value {
    json!({"path": {"choice": path, "p": p}})
}

/// The first verdict `answers` under `over`, up to the call that leaves for
/// the second judge: what the cell sent on the verdict, and what it sent on
/// the view.
fn sharpening(over: &[Knob], answers: Value) -> (Vec<Value>, Vec<Value>) {
    let asked = run_cell(ROUTE, over, decision(answers)).0;
    let ask = on(&asked, "view_ask").clone();
    let called = run_cell(ROUTE, over, view_back(&ask)).0;
    (asked, called)
}

/// The turn's emissions once the second judge said `verdict` in time.
fn sharpened(over: &[Knob], answers: Value, verdict: &Value) -> Vec<Value> {
    let (_, called) = sharpening(over, answers);
    let call = on(&called, JUDGE);
    run_cell(
        ROUTE,
        over,
        verdict_back(
            call,
            json!({"finish_reason": "stop"}),
            &verdict.to_string(),
            in_time(),
        ),
    )
    .0
}

/// Contract point 1: each steering statement has its own bar, and what stays
/// under it is not in the stamp.
#[test]
fn gh_d54g_stamp_a_steering_statement_below_its_threshold_is_not_stamped() {
    let bars = [(SURE, json!({"path": 0.9, "move": 0.9})), (MOVES, moves())];
    // The class under its bar (0.7 under 0.9, above the shipped 0.6): not
    // stamped, the stamp says why on the hop.
    let out = run_cell(ROUTE, &bars, decision(first("simple", 0.7))).0;
    assert_eq!(routes(&out), vec!["stamp".to_string()], "{out:?}");
    assert_eq!(stamp_of(&out)["path"], json!(FALLBACK), "{out:?}");
    assert_eq!(fallback_of(&out), "unsure", "{out:?}");
    // At its bar it stands.
    let out = run_cell(ROUTE, &bars, decision(first("simple", 0.95))).0;
    assert_eq!(stamp_of(&out)["path"], json!("simple"), "{out:?}");
    assert_eq!(fallback_of(&out), "", "{out:?}");

    // The move has its own bar: under it the stamp carries none, and the
    // class beside it stands untouched.
    let out = run_cell(
        ROUTE,
        &bars,
        decision(with_move(first("simple", 0.95), "mirror", 0.7)),
    )
    .0;
    assert_eq!(stamp_of(&out)["path"], json!("simple"), "{out:?}");
    assert!(stamp_of(&out).get("move").is_none(), "{out:?}");
    let out = run_cell(
        ROUTE,
        &bars,
        decision(with_move(first("simple", 0.95), "mirror", 0.95)),
    )
    .0;
    assert_eq!(stamp_of(&out)["move"], json!("mirror"), "{out:?}");

    // Two statements, two bars: the same 0.7 stamps the class under a bar
    // of 0.6 and leaves the move out under one of 0.9.
    let split = [(SURE, json!({"path": 0.6, "move": 0.9})), (MOVES, moves())];
    let out = run_cell(
        ROUTE,
        &split,
        decision(with_move(first("simple", 0.7), "mirror", 0.7)),
    )
    .0;
    assert_eq!(stamp_of(&out)["path"], json!("simple"), "{out:?}");
    assert!(stamp_of(&out).get("move").is_none(), "{out:?}");
}

/// Contract point 2: an unsure statement the composition names under
/// `sharpen` goes to the second judge, and its verdict takes the place of
/// the unsure one.
#[test]
fn gh_d54g_stamp_an_unsure_statement_is_sharpened_by_the_second_judge() {
    let over = knobs(&[]);
    let (asked, called) = sharpening(&over, first("simple", 0.7));
    // The unsure verdict stamps nothing yet: the second judge orders its
    // view, and nothing else leaves.
    assert_eq!(routes(&asked), vec!["view_ask".to_string()], "{asked:?}");
    // On the view, one call leaves on the route the composition named.
    assert_eq!(routes(&called), vec![JUDGE.to_string()], "{called:?}");

    let out = sharpened(&over, first("simple", 0.7), &second_says("complex", 0.95));
    assert_eq!(routes(&out), vec!["stamp".to_string()], "{out:?}");
    let stamp = stamp_of(&out);
    assert_eq!(
        stamp["path"],
        json!("complex"),
        "the second verdict: {out:?}"
    );
    assert_eq!(stamp["p"], json!(0.95), "with its confidence: {out:?}");
    assert_eq!(fallback_of(&out), "", "a judged stamp: {out:?}");
    // What the first verdict said beside the class stays.
    assert_eq!(stamp["intent"], json!("task"), "{out:?}");
    assert_eq!(stamp["families"], json!(["history"]), "{out:?}");
    assert_eq!(stamp["turn_id"], json!("c-1"), "{out:?}");
    // Contract point 8: the stamp says that and how it was sharpened.
    let trail = &stamp["trail"];
    assert_eq!(trail["sharpened"], json!(true), "{out:?}");
    assert_eq!(trail[FIRST], json!("simple"), "{out:?}");
    assert_eq!(trail["second"], json!("complex"), "{out:?}");
    assert!(
        !out.iter().any(|m| m.to_string().contains(TEXT)),
        "the words of the person are not on the stamp: {out:?}"
    );
}

/// Contract point 2, last sentence: a sure first verdict that asks nothing
/// back is never sharpened.
#[test]
fn gh_d54g_stamp_a_sure_first_verdict_is_never_sharpened() {
    let over = knobs(&[(ASKING, json!("1"))]);
    for path in ["simple", "talk", "complex"] {
        let out = run_cell(ROUTE, &over, decision(first(path, 0.95))).0;
        assert_eq!(
            routes(&out),
            vec!["stamp".to_string()],
            "{path}: one stamp at once, no view and no call: {out:?}"
        );
        let stamp = stamp_of(&out);
        assert_eq!(stamp["path"], json!(path), "{out:?}");
        assert_eq!(stamp["p"], json!(0.95), "{out:?}");
        assert_eq!(fallback_of(&out), "", "{out:?}");
        assert_eq!(stamp["trail"]["sharpened"], json!(false), "{out:?}");
    }

    // A partial of a voice call is stamped many times per utterance: only
    // the stamp a turn is cut with is ever sharpened, never a partial's,
    // however unsure it is.
    let mut partial = decision(first("simple", 0.7));
    partial["header"]["context"]["delta"] = json!("1");
    partial["header"]["hop"]["state"] = json!(TEXT);
    let out = run_cell(ROUTE, &over, partial).0;
    assert_eq!(
        routes(&out),
        vec!["delta_stamp".to_string()],
        "a partial's stamp leaves at once: {out:?}"
    );

    // Contract point 3 on a partial (review R1 I1 (a)): it is never
    // sharpened, so the class that asks the person back -- the first
    // verdict's alone, however sure -- is not on its stamp either. A turn that
    // runs under a partial's stamp would otherwise ask back without a second
    // judge.
    let partial_of = |answers: Value| {
        let mut doc = decision(answers);
        doc["header"]["context"]["delta"] = json!("1");
        doc
    };
    let out = run_cell(ROUTE, &over, partial_of(first(ASKING, 0.97))).0;
    assert_eq!(routes(&out), vec!["delta_stamp".to_string()], "{out:?}");
    let s = on(&out, "delta_stamp");
    assert_eq!(s["stamp"]["path"], json!(FALLBACK), "{out:?}");
    assert_eq!(s["header"]["path"], json!(FALLBACK), "{out:?}");
    // Without `sharpen` the partial's stamp is the one of before (contract
    // point 9): the asking class stands.
    let plain = knobs(&[(ASKING, json!("1")), (SHARPEN, json!([]))]);
    let out = run_cell(ROUTE, &plain, partial_of(first(ASKING, 0.97))).0;
    assert_eq!(
        on(&out, "delta_stamp")["stamp"]["path"],
        json!(ASKING),
        "{out:?}"
    );
}

/// Contract point 2 with the head's point 2: the second judge orders its
/// view itself, with the request the first verdict's view is ordered with,
/// and the first verdict travels with its call.
#[test]
fn gh_d54g_stamp_the_second_judge_gets_the_same_view() {
    let over = knobs(&[("judge_context", json!("1")), ("context_turns", json!(2))]);
    // The request of the first verdict's view.
    let opened = run_cell(ROUTE, &over, turn()).0;
    let want = on(&opened, "view_ask")["view_ask"].clone();
    assert_eq!(
        want,
        json!({"turns": 2, "budget_tokens": 1500, "target": "decide"}),
        "check of the setup: the first verdict's request is today's"
    );

    let (asked, called) = sharpening(&over, first("simple", 0.7));
    let ask = on(&asked, "view_ask");
    assert_eq!(ask["view_ask"], want, "the same request: {asked:?}");
    assert!(
        !ask["header"][FIRST].is_null(),
        "the first verdict rides out with the ask: {asked:?}"
    );
    // The stateless cell keeps the turn's words on the hop, as the first
    // ask does.
    assert_eq!(ask["header"]["state"], json!(TEXT), "{asked:?}");

    let call = on(&called, JUDGE);
    assert!(
        !call["header"][FIRST].is_null(),
        "the first verdict travels with the second call: {called:?}"
    );
    // The judge takes a window: the view's turns, its topic and the request
    // are in what the call carries.
    let body = json!({"messages": call["messages"], "system": call["system"]}).to_string();
    for part in [
        "Good morning.",
        "Good morning to you.",
        "the trip on Sunday",
        TEXT,
    ] {
        assert!(body.contains(part), "the call carries {part:?}: {call}");
    }
}

/// Contract point 3: the class that asks the person back is stamped only
/// when the second judge names it surely; the first verdict alone never
/// stamps it, however sure it is.
#[test]
fn gh_d54g_stamp_the_asking_class_is_stamped_only_by_the_second_judge() {
    let over = knobs(&[(ASKING, json!("1"))]);
    // The first verdict says it with 0.97, far above every bar: no stamp,
    // the second judge is asked.
    let (asked, called) = sharpening(&over, first(ASKING, 0.97));
    assert_eq!(routes(&asked), vec!["view_ask".to_string()], "{asked:?}");
    assert_eq!(routes(&called), vec![JUDGE.to_string()], "{called:?}");

    // The second judge names it surely: now it stands.
    let out = sharpened(&over, first(ASKING, 0.97), &second_says(ASKING, 0.95));
    assert_eq!(routes(&out), vec!["stamp".to_string()], "{out:?}");
    assert_eq!(stamp_of(&out)["path"], json!(ASKING), "{out:?}");
    assert_eq!(fallback_of(&out), "", "{out:?}");
    assert_eq!(stamp_of(&out)["trail"]["sharpened"], json!(true), "{out:?}");

    // The second judge reads the request as plain: its class stands, and
    // the person is not asked.
    let out = sharpened(&over, first(ASKING, 0.97), &second_says("simple", 0.95));
    assert_eq!(stamp_of(&out)["path"], json!("simple"), "{out:?}");

    // No other way leads there (review R1 I1 (b)): a composition whose
    // `notes_class` is the asking class, and a product choice that sets a
    // note. Under `sharpen` the note does not steer the stamp onto the asking
    // class -- not on a sure first verdict, not on the stamp without the
    // class, not after the second judge named another class.
    let choices = json!({"risk": {"question": "Is the person at risk?",
                                  "options": {"acute": "acute risk", "low": "no risk"},
                                  "fallback": "unknown",
                                  "notes": {"acute": "Name the emergency number."}}});
    let noted = |answers: Value| {
        let mut a = answers;
        a["choices.risk"] = json!({"choice": "acute", "p": 0.99});
        a
    };
    let steering = knobs(&[
        (ASKING, json!("1")),
        ("notes_class", json!(ASKING)),
        ("choices", choices.clone()),
    ]);
    let out = run_cell(ROUTE, &steering, decision(noted(first("simple", 0.95)))).0;
    assert_eq!(
        stamp_of(&out)["path"],
        json!("simple"),
        "sure first: {out:?}"
    );
    let mut bare = decision(noted(first("simple", 0.7)));
    bare["header"]["hop"]["state"] = json!("");
    let out = run_cell(ROUTE, &steering, bare).0;
    assert_eq!(
        stamp_of(&out)["path"],
        json!(FALLBACK),
        "unsure first: {out:?}"
    );
    assert_eq!(fallback_of(&out), "unsure", "{out:?}");
    let out = sharpened(
        &steering,
        noted(first("simple", 0.7)),
        &second_says("simple", 0.95),
    );
    assert_eq!(stamp_of(&out)["path"], json!("simple"), "second: {out:?}");
    // The second judge's own sure word for it still stands.
    let out = sharpened(
        &steering,
        noted(first("simple", 0.7)),
        &second_says(ASKING, 0.95),
    );
    assert_eq!(stamp_of(&out)["path"], json!(ASKING), "{out:?}");
    // Without `sharpen` the note steers as before (contract point 9).
    let before = knobs(&[
        (ASKING, json!("1")),
        ("notes_class", json!(ASKING)),
        ("choices", choices),
        (SHARPEN, json!([])),
    ]);
    let out = run_cell(ROUTE, &before, decision(noted(first("simple", 0.95)))).0;
    assert_eq!(stamp_of(&out)["path"], json!(ASKING), "{out:?}");
    assert_eq!(
        on(&out, "stamp")["header"]["steered"],
        json!("route.risk"),
        "{out:?}"
    );
}

/// Contract point 3: when the second judge is unsure too, no asking class is
/// in the stamp, and there is no third judge.
#[test]
fn gh_d54g_stamp_an_unsure_second_verdict_stamps_no_asking_class() {
    let over = knobs(&[(ASKING, json!("1"))]);
    for start in [first(ASKING, 0.97), first("simple", 0.7)] {
        let out = sharpened(&over, start.clone(), &second_says(ASKING, 0.6));
        assert_eq!(
            routes(&out),
            vec!["stamp".to_string()],
            "one stamp, nobody else is asked: {out:?}"
        );
        let stamp = stamp_of(&out);
        assert_ne!(stamp["path"], json!(ASKING), "{start}: {out:?}");
        assert_eq!(stamp["path"], json!(FALLBACK), "{start}: {out:?}");
        assert_eq!(fallback_of(&out), "unsure", "{start}: {out:?}");
        assert_eq!(stamp["trail"]["sharpened"], json!(true), "{out:?}");
        assert_eq!(stamp["trail"]["second"], json!(ASKING), "{out:?}");
    }
    // A composition whose fallback is the asking class: a fallback is no
    // verdict of a model, so where the class is sharpened it stamps the
    // shipped fallback instead (strand ruling OR-d54g.stamp.22) -- after an
    // unsure second judge, without one, and on an error of the decider.
    let asking_fallback = knobs(&[(ASKING, json!("1")), ("fallback_class", json!(ASKING))]);
    for start in [first(ASKING, 0.97), first("simple", 0.7)] {
        let out = sharpened(&asking_fallback, start.clone(), &second_says(ASKING, 0.6));
        assert_eq!(stamp_of(&out)["path"], json!(FALLBACK), "{start}: {out:?}");
        assert_eq!(fallback_of(&out), "unsure", "{start}: {out:?}");
    }
    let no_judge = knobs(&[
        (ASKING, json!("1")),
        ("fallback_class", json!(ASKING)),
        (SECOND, json!("")),
    ]);
    let out = run_cell(ROUTE, &no_judge, decision(first("simple", 0.7))).0;
    assert_eq!(stamp_of(&out)["path"], json!(FALLBACK), "{out:?}");
    let failed = lane(
        "in_decision",
        json!({"finish_reason": "error", "error_code": "timeout", "asked_at": in_time()}),
        ctx(),
        json!([]),
    );
    let out = run_cell(ROUTE, &asking_fallback, failed).0;
    assert_eq!(stamp_of(&out)["path"], json!(FALLBACK), "{out:?}");
    assert_eq!(fallback_of(&out), "error", "{out:?}");
    // Without `sharpen` the param stands as before.
    let plain = [(ASKING, json!("1")), ("fallback_class", json!(ASKING))];
    let out = run_cell(ROUTE, &plain, decision(first("simple", 0.5))).0;
    assert_eq!(stamp_of(&out)["path"], json!(ASKING), "{out:?}");
    assert_eq!(fallback_of(&out), "unsure", "{out:?}");
}

/// Contract point 2: without a second judge a statement that would be
/// sharpened is not stamped.
#[test]
fn gh_d54g_stamp_without_a_second_judge_an_unsure_statement_is_dropped() {
    let over = knobs(&[(SECOND, json!("")), (ASKING, json!("1"))]);
    // The class under its bar.
    let out = run_cell(ROUTE, &over, decision(first("simple", 0.7))).0;
    assert_eq!(
        routes(&out),
        vec!["stamp".to_string()],
        "nobody to ask: one stamp at once: {out:?}"
    );
    assert_eq!(stamp_of(&out)["path"], json!(FALLBACK), "{out:?}");
    assert_eq!(fallback_of(&out), "unsure", "{out:?}");
    // The asking class of the first verdict alone, however sure.
    let out = run_cell(ROUTE, &over, decision(first(ASKING, 0.97))).0;
    assert_eq!(routes(&out), vec!["stamp".to_string()], "{out:?}");
    assert_ne!(stamp_of(&out)["path"], json!(ASKING), "{out:?}");
    assert_eq!(stamp_of(&out)["path"], json!(FALLBACK), "{out:?}");
    // A sure class that asks nothing stands as ever.
    let out = run_cell(ROUTE, &over, decision(first("simple", 0.95))).0;
    assert_eq!(stamp_of(&out)["path"], json!("simple"), "{out:?}");
}

/// Contract point 4 with the head's point 4: a sharpened turn waits for the
/// stamp it is cut with; a second judge that does not answer closes it
/// without the unsure statement. An order of events, no time window. The
/// cell's clock has no test override (`scripts/wall_clock.py`: no injectable
/// wall clock in the tree), so what the bound does is held with fixed moments
/// in the data, the form of `assemble/gh1244...`: the year 2000 is over for
/// any bound, the year 2999 ahead of any clock (steps 5 and 6, strand ruling
/// OR-d54g.stamp.17). Nothing here waits and nothing is measured.
#[test]
fn gh_d54g_stamp_a_silent_second_judge_closes_the_turn_at_the_wait_bound() {
    let over = knobs(&[]);
    // What the turn closes with when nobody sharpens: the unsure statement
    // dropped.
    let plain = knobs(&[(SECOND, json!(""))]);
    let dropped = run_cell(ROUTE, &plain, decision(first("simple", 0.7))).0;
    let want = stamp_of(&dropped).clone();
    assert_eq!(
        want["path"],
        json!(FALLBACK),
        "check of the setup: {dropped:?}"
    );

    // 1. The unsure verdict and the view: no stamp has left, the turn waits.
    let (asked, called) = sharpening(&over, first("simple", 0.7));
    for step in [&asked, &called] {
        assert!(
            !routes(step).contains(&"stamp".to_string()),
            "no stamp before the second judge answered: {step:?}"
        );
    }
    let call = on(&called, JUDGE);

    // 2. The second judge does not answer; its cell reports the bound it
    // ran into. The turn closes with ONE stamp, without the unsure class.
    let silent = verdict_back(
        call,
        json!({"finish_reason": "error", "error_code": "provider_timeout"}),
        "",
        in_time(),
    );
    let out = run_cell(ROUTE, &over, silent).0;
    assert_eq!(routes(&out), vec!["stamp".to_string()], "{out:?}");
    let got = stamp_of(&out);
    for key in ["turn_id", "path", "p", "intent", "needs", "families"] {
        assert_eq!(got[key], want[key], "{key}: {out:?}");
    }
    assert_ne!(fallback_of(&out), "", "the hop says why: {out:?}");
    assert_eq!(got["trail"]["sharpened"], json!(false), "{out:?}");

    // 3. The ask that opens the sharpening names the bound the turn now
    // waits under (`second_wait_ms`, not `late_ms`); without a bound it
    // names none.
    let ask = on(&asked, "view_ask");
    assert_eq!(ask["header"]["wait_ms"], json!("5000"), "{asked:?}");
    let unbound = knobs(&[(SECOND_WAIT, json!(0))]);
    let (asked, _) = sharpening(&unbound, first("simple", 0.7));
    assert!(
        on(&asked, "view_ask")["header"]["wait_ms"].is_null(),
        "no bound, no notice: {asked:?}"
    );

    // 4. The composite hands that ask to the collector as a notice on
    // `in_route` (talky's edge: no stamp, `hop.wait_ms`). A collector that
    // waits for the stamp parks it as a hold of the turn, never as the
    // stamp's leg; the shipped collector (`route_wait_ms` 0) waits for no
    // stamp and parks the notice unread.
    let notice = || lane("in_route", json!({"wait_ms": "5000"}), ctx(), json!([]));
    let held = assemble(&[("route_wait_ms", json!(150))], notice());
    let row = assemble_cell::call(in_phase(&held, "collect"), "c-collect-row")["row"].clone();
    assert_eq!(row["role"], json!("route-hold"), "{held:?}");
    assert_eq!(row["turn_id"], json!("c-1"), "{held:?}");
    assert!(
        !held.iter().any(|m| m.to_string().contains("leg-route")),
        "a notice is no stamp: {held:?}"
    );
    assert_eq!(assemble(&[], notice()), Vec::<Value>::new(), "shipped");

    // ONE wait, ONE start (review R1 M2). The cell counts the bound from
    // `hop.second_at`; talky's notice edge hands that moment to the collector
    // as `hop.wait_from`, and the hold ends at that moment plus the bound --
    // not at the notice's arrival plus the bound.
    let talky = config_of("templates/talky/config.json").to_string();
    assert!(
        talky.contains(r#""wait_from":"has(hop.second_at) ? hop.second_at : ''""#),
        "talky's notice edge hands the start of the sharpening on as wait_from"
    );
    let from_start = lane(
        "in_route",
        json!({"wait_ms": "5000", "wait_from": OLD_MS.to_string()}),
        ctx(),
        json!([]),
    );
    let held = assemble(&[("route_wait_ms", json!(150))], from_start);
    let row = assemble_cell::call(in_phase(&held, "collect"), "c-collect-row")["row"].clone();
    let until: Value = serde_json::from_str(row["turn"].as_str().expect("payload")).expect("json");
    assert_eq!(until["until_ms"], json!(OLD_MS + 5000), "{held:?}");
    // A start or a bound that is no finite number (re-check 1, N1: "inf"
    // overflowed `int()` and the cell exited 1, the hold was lost) counts as
    // missing, like an unreadable field: the start falls back to the notice's
    // arrival -- some moment after the year 2000 and before 2999 --, the bound
    // to none, and the cell ends with its regular output.
    for bad in ["inf", "-inf", "1e999"] {
        let odd_start = lane(
            "in_route",
            json!({"wait_ms": "5000", "wait_from": bad}),
            ctx(),
            json!([]),
        );
        let held = assemble(&[("route_wait_ms", json!(150))], odd_start);
        let row = assemble_cell::call(in_phase(&held, "collect"), "c-collect-row")["row"].clone();
        assert_eq!(row["role"], json!(HOLD), "wait_from {bad}: {held:?}");
        let until: Value =
            serde_json::from_str(row["turn"].as_str().expect("payload")).expect("json");
        let ms = until["until_ms"].as_i64().unwrap_or(0);
        assert!(
            ms > OLD_MS + 5000 && ms < FRESH_MS,
            "wait_from {bad} counts from the arrival: {held:?}"
        );
        let odd_bound = lane("in_route", json!({"wait_ms": bad}), ctx(), json!([]));
        let held = assemble(&[("route_wait_ms", json!(150))], odd_bound);
        assert!(
            !held.iter().any(|m| m.to_string().contains(HOLD)),
            "wait_ms {bad} asks for no hold: {held:?}"
        );
    }
    // The second judge's own operation timeout is the bound of its ONE call
    // (AGENTS rule 12, A), not a second wait: shipped under the bound, so a
    // judge that does not answer reports its error before the turn's bound.
    let ext =
        config_of("templates/talky/second_judge/config.json")["params"]["external_timeout_ms"]
            .as_u64()
            .expect("external_timeout_ms");
    let bound = config_of(ROUTE)["params"][SECOND_WAIT]
        .as_u64()
        .expect("second_wait_ms");
    assert!(
        ext < bound,
        "second_judge's operation timeout {ext} under the bound {bound}"
    );

    // 5. An answer AFTER the bound is not the turn's verdict (strand ruling
    // OR-d54g.stamp.17: fixed moments, no clock in the test). The sharpening
    // began in the year 2000, so the bound is over for any clock: the turn
    // closes with ONE stamp without the unsure class, and says it was late.
    let (_, called) = sharpening(&over, first("simple", 0.7));
    let call = on(&called, JUDGE);
    let said = second_says("simple", 0.95).to_string();
    let answer = |knobs: &[Knob], began: i64, asked: i64| {
        run_cell(
            ROUTE,
            knobs,
            verdict_back(
                call,
                json!({"finish_reason": "stop", "second_at": began.to_string()}),
                &said,
                asked.to_string(),
            ),
        )
        .0
    };
    let out = answer(&over, OLD_MS, FRESH_MS);
    assert_eq!(routes(&out), vec!["stamp".to_string()], "{out:?}");
    let got = stamp_of(&out);
    for key in ["turn_id", "path", "p", "intent", "needs", "families"] {
        assert_eq!(got[key], want[key], "after the bound, {key}: {out:?}");
    }
    assert_eq!(fallback_of(&out), "late", "{out:?}");
    assert_eq!(got["trail"]["sharpened"], json!(false), "{out:?}");
    assert_eq!(got["trail"]["fallback"], json!("late"), "{out:?}");
    // The same answer of a sharpening that began in the year 2999 is inside
    // any bound: it is the verdict.
    let out = answer(&over, FRESH_MS, FRESH_MS);
    assert_eq!(stamp_of(&out)["path"], json!("simple"), "{out:?}");
    assert_eq!(stamp_of(&out)["trail"]["sharpened"], json!(true), "{out:?}");
    assert_eq!(fallback_of(&out), "", "{out:?}");
    // Without a bound nothing is late here, and `late_ms` (1500 as shipped)
    // does not judge a sharpened turn: asked and begun in the year 2000, the
    // second judge's word still stands.
    let out = answer(&unbound, OLD_MS, OLD_MS);
    assert_eq!(stamp_of(&out)["path"], json!("simple"), "{out:?}");
    assert_eq!(fallback_of(&out), "", "{out:?}");
    // The second judge's VIEW after the bound closes the turn the same way,
    // and the judge is not called any more.
    let (asked, _) = sharpening(&over, first("simple", 0.7));
    let mut view = view_back(on(&asked, "view_ask"));
    view["header"]["hop"]["second_at"] = json!(OLD_MS.to_string());
    let out = run_cell(ROUTE, &over, view).0;
    assert_eq!(routes(&out), vec!["stamp".to_string()], "{out:?}");
    assert_eq!(fallback_of(&out), "late", "{out:?}");
    assert_eq!(stamp_of(&out)["path"], want["path"], "{out:?}");

    // 6. The collector's hold, running and over (`expected_legs`), with the
    // moments as fixed data in the round rows. Check of the setup: without a
    // hold a turn whose window leg is older than `route_wait_ms` stops
    // waiting for its stamp.
    let hold = |until: i64| leg(HOLD, json!({"until_ms": until}), FRESH);
    let out = collect(json!([window(OLD), memory()]));
    assert!(seam(&out).is_some(), "check of the setup: {out:?}");
    // A hold that runs (until the year 2999): the turn waits on, however old
    // its window leg is.
    let out = collect(json!([window(OLD), memory(), hold(FRESH_MS)]));
    assert!(seam(&out).is_none(), "the hold runs: {out:?}");
    // A hold that is over (since the year 2000): `route_wait_ms` decides
    // again, and the old window leg ends the wait.
    let out = collect(json!([window(OLD), memory(), hold(OLD_MS)]));
    assert!(seam(&out).is_some(), "the hold is over: {out:?}");
    // A hold only lengthens: over, it does not end the wait of a turn whose
    // window leg is fresh.
    let out = collect(json!([window(FRESH), memory(), hold(OLD_MS)]));
    assert!(seam(&out).is_none(), "a hold never shortens: {out:?}");
    // The stamp ends a running hold: the turn leaves with its class.
    let stamped = leg("leg-route", json!({"path": "simple"}), FRESH);
    let out = collect(json!([window(OLD), hold(FRESH_MS), stamped]));
    let s = seam(&out).unwrap_or_else(|| panic!("the stamp came: {out:?}"));
    assert_eq!(s["header"]["path"], json!("simple"), "{out:?}");
}

/// Contract point 5: the composition's moves are asked in the SAME decide
/// call, with `none`; an unsure move alone sharpens nothing; when the turn
/// is sharpened anyway, the second judge names the move too. And the stamp's
/// move and trail reach the curate seam: the collector parks both with the
/// stamp's leg and carries them as `hop.move` and `hop.trail`.
#[test]
fn gh_d54g_stamp_a_stamp_may_carry_a_move_asked_in_the_same_call() {
    let over = knobs(&[(MOVES, moves())]);
    // One decide call, the move among its questions.
    let out = run_cell(ROUTE, &over, turn()).0;
    assert_eq!(routes(&out), vec!["decide".to_string()], "{out:?}");
    let q = &on(&out, "decide")["decide"]["questions"]["move"];
    assert_eq!(q["kind"], json!("choice"), "{out:?}");
    let options = q["options"].as_object().expect("the move's options");
    let mut names: Vec<&str> = options.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec!["mirror", "none", "steady"],
        "the list and `none`: {q}"
    );
    assert_eq!(options["mirror"], moves()["mirror"], "{q}");
    // Without a list the question is not asked.
    let bare = run_cell(ROUTE, &knobs(&[]), turn()).0;
    assert!(
        on(&bare, "decide")["decide"]["questions"]
            .get("move")
            .is_none(),
        "{bare:?}"
    );

    // A sure move stands on the stamp; `none` and a name outside the list
    // never do.
    let sure = with_move(first("talk", 0.95), "mirror", 0.95);
    let out = run_cell(ROUTE, &over, decision(sure)).0;
    assert_eq!(stamp_of(&out)["move"], json!("mirror"), "{out:?}");
    assert_eq!(stamp_of(&out)["path"], json!("talk"), "{out:?}");
    for name in ["none", "made_up"] {
        let out = run_cell(
            ROUTE,
            &over,
            decision(with_move(first("talk", 0.95), name, 0.99)),
        )
        .0;
        assert!(stamp_of(&out).get("move").is_none(), "{name}: {out:?}");
    }

    // An unsure move beside a sure class: no move, and nobody is asked.
    let unsure = with_move(first("talk", 0.95), "mirror", 0.7);
    let out = run_cell(ROUTE, &over, decision(unsure)).0;
    assert_eq!(
        routes(&out),
        vec!["stamp".to_string()],
        "an unsure move alone sharpens nothing: {out:?}"
    );
    assert!(stamp_of(&out).get("move").is_none(), "{out:?}");
    assert_eq!(stamp_of(&out)["path"], json!("talk"), "{out:?}");

    // A sure move is a statement of its own (strand ruling OR-d54g.stamp.12):
    // beside an unsure class that nobody sharpens, the class falls back and
    // the move stands.
    let alone = knobs(&[(MOVES, moves()), (SECOND, json!(""))]);
    let unsure_class = with_move(first("simple", 0.7), "mirror", 0.95);
    let out = run_cell(ROUTE, &alone, decision(unsure_class)).0;
    assert_eq!(routes(&out), vec!["stamp".to_string()], "{out:?}");
    assert_eq!(stamp_of(&out)["path"], json!(FALLBACK), "{out:?}");
    assert_eq!(fallback_of(&out), "unsure", "{out:?}");
    assert_eq!(stamp_of(&out)["move"], json!("mirror"), "{out:?}");

    // The class is unsure, so the turn is sharpened anyway: the second
    // judge is asked for the move with the class (the composition's list in
    // its question), and without a list it is asked for none.
    let (_, called) = sharpening(&over, with_move(first("talk", 0.7), "mirror", 0.7));
    let asked = on(&called, JUDGE)["system"].to_string();
    for (name, when) in obj_of(&moves()) {
        let when = when.as_str().expect("a sentence");
        assert!(
            asked.contains(name.as_str()) && asked.contains(when),
            "the second judge is asked for {name}: {asked}"
        );
    }
    let (_, called) = sharpening(&knobs(&[]), first("talk", 0.7));
    let bare = on(&called, JUDGE)["system"].to_string();
    for (_, when) in obj_of(&moves()) {
        assert!(
            !bare.contains(when.as_str().expect("a sentence")),
            "no list, no move in the question: {bare}"
        );
    }
    let mut verdict = second_says("talk", 0.95);
    verdict["move"] = json!({"choice": "steady", "p": 0.95});
    let out = sharpened(
        &over,
        with_move(first("talk", 0.7), "mirror", 0.7),
        &verdict,
    );
    assert_eq!(stamp_of(&out)["path"], json!("talk"), "{out:?}");
    assert_eq!(stamp_of(&out)["move"], json!("steady"), "{out:?}");

    // The second half (the orchestrator's decision on the strand's open
    // question): the collector is the stamp's only way to the curator. The
    // stamp of the sharpened turn above, handed to `in_route`, is parked with
    // its move and its trail ...
    let sent = on(&out, "stamp");
    let trail = sent["stamp"]["trail"].clone();
    assert_eq!(
        trail["sharpened"],
        json!(true),
        "check of the setup: {out:?}"
    );
    let kept = parked(sent);
    assert_eq!(kept["move"], json!("steady"), "{kept}");
    assert_eq!(kept["trail"], trail, "{kept}");
    assert_eq!(kept["path"], json!("talk"), "{kept}");
    // ... and both ride on the seam of the turn beside the class: the move
    // as its name, the trail as the JSON text of the object.
    let fired = collect(json!([
        window(FRESH),
        memory(),
        leg("leg-route", kept, FRESH)
    ]));
    let s = seam(&fired).unwrap_or_else(|| panic!("every leg stands: {fired:?}"));
    assert_eq!(s["header"]["path"], json!("talk"), "{s}");
    assert_eq!(s["header"]["move"], json!("steady"), "{s}");
    let rode: Value = serde_json::from_str(s["header"]["trail"].as_str().expect("the trail"))
        .expect("the trail is JSON text");
    assert_eq!(rode, trail, "{s}");
    // A stamp without them leaves the seam of before: neither key is there.
    let plain = run_cell(ROUTE, &[], decision(first("talk", 0.95))).0;
    let kept = parked(on(&plain, "stamp"));
    assert!(
        kept.get("move").is_none() && kept.get("trail").is_none(),
        "{kept}"
    );
    let fired = collect(json!([
        window(FRESH),
        memory(),
        leg("leg-route", kept, FRESH)
    ]));
    let s = seam(&fired).unwrap_or_else(|| panic!("every leg stands: {fired:?}"));
    assert!(
        s["header"].get("move").is_none() && s["header"].get("trail").is_none(),
        "no move, no trail, the seam of before: {s}"
    );
}

/// What the route of master stamped for these verdicts (computed from
/// `templates/talky/route/config.json` at 5450dc7ba5, asked an hour ahead).
const TODAY_SURE: &str = r#"{"header": {"route": "stamp", "turn_id": "c-1", "path": "simple"}, "messages": [], "stamp": {"turn_id": "c-1", "path": "simple", "p": 0.9, "intent": "task", "needs": ["tool"], "families": ["history"], "duration": 0}}"#;
const TODAY_UNSURE: &str = r#"{"header": {"route": "stamp", "turn_id": "c-1", "path": "talk", "fallback": "unsure"}, "messages": [], "stamp": {"turn_id": "c-1", "path": "talk", "p": 0.0, "intent": "task", "needs": [], "families": [], "duration": 0}}"#;
const TODAY_ASKING: &str = r#"{"header": {"route": "stamp", "turn_id": "c-1", "path": "clarify"}, "messages": [], "stamp": {"turn_id": "c-1", "path": "clarify", "p": 0.7, "intent": "task", "needs": [], "families": ["history"], "duration": 0}}"#;
const TODAY_ERROR: &str = r#"{"header": {"route": "stamp", "turn_id": "c-1", "path": "talk", "fallback": "error"}, "messages": [], "stamp": {"turn_id": "c-1", "path": "talk", "p": 0.0, "intent": "", "needs": [], "families": [], "duration": 0}}"#;

/// Contract point 9: without bars, without a list and without a second
/// judge the stamp is the bytes it was -- and that is how the cell ships.
#[test]
fn gh_d54g_stamp_fresh_params_leave_the_stamp_byte_identical() {
    let mut failures = Vec::new();
    // The stamps of today, byte for byte, under the shipped params.
    let mut asking_low_tool = first(ASKING, 0.7);
    asking_low_tool["needs.tool"] = json!({"yes": 0.46});
    let failed = lane(
        "in_decision",
        json!({"finish_reason": "error", "error_code": "provider_timeout",
               "asked_at": in_time()}),
        ctx(),
        json!([]),
    );
    let cases: [(&str, Vec<Knob>, Value, &str); 4] = [
        ("sure", vec![], decision(first("simple", 0.9)), TODAY_SURE),
        (
            "unsure",
            vec![],
            decision(first("simple", 0.55)),
            TODAY_UNSURE,
        ),
        // With today's knob alone the first verdict stamps the asking class
        // as it did: the new rule begins with the new params.
        (
            "asking",
            vec![(ASKING, json!("1"))],
            decision(asking_low_tool),
            TODAY_ASKING,
        ),
        ("error", vec![], failed, TODAY_ERROR),
    ];
    for (name, over, doc, want) in cases {
        let got = run_cell_stdout(ROUTE, &over, doc).0;
        if got.trim_end() != want {
            failures.push(format!(
                "{name}: the stamp is not today's bytes:\n got {}\nwant {want}",
                got.trim_end()
            ));
        }
    }
    // The decide call of a shipped turn asks no move.
    let out = run_cell(ROUTE, &[], turn()).0;
    if on(&out, "decide")["decide"]["questions"]
        .get("move")
        .is_some()
    {
        failures.push(format!("shipped, the decide call asks a move: {out:?}"));
    }
    // And the cell ships the new params neutral.
    let params = config_of(ROUTE)["params"].clone();
    for (name, neutral) in [
        (SURE, json!({})),
        (SHARPEN, json!([])),
        (SECOND, json!("")),
        (MOVES, json!({})),
    ] {
        if params.get(name) != Some(&neutral) {
            failures.push(format!(
                "the route ships `{name}` as {neutral}: {:?}",
                params.get(name)
            ));
        }
    }
    if !params.get(SECOND_WAIT).is_some_and(|v| v.is_number()) {
        failures.push(format!(
            "the route ships `{SECOND_WAIT}` as a number: {:?}",
            params.get(SECOND_WAIT)
        ));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
