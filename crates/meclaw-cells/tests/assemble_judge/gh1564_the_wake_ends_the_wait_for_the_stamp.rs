//! GH #1564 (OR-54g-271/272) -- the ONE bound of the wait has a wake.
//!
//! WHY this file exists: `route_wait_ms` of the collector is the one bound of
//! the wait for the route stamp (GH #1562), but the fan-in measured the window
//! leg's age only when a leg LANDED. Under the former `spec_ahead` (gone, GH #1584) every other leg was in
//! before the speculation fires, so nothing landed after the bound: the turn
//! waited for its stamp, and with a decider of 5 000 ms the answer waited
//! 5.0 s (lab os10, trace 01a1e851-0bbc) -- against "the answer waits only W"
//! (R-TR-27). Now the fan-in that waits for the stamp orders ONE one-shot of
//! the collector's own clock at the window leg + `route_wait_ms` (or at the end
//! of a route-hold notice, whichever is later); its strike reads the round back
//! and fires the turn without the stamp. A stamp that lands or a turn that
//! fires first takes the order back (`remove`); a strike for a fired turn does
//! nothing. `route_wait_ms` 0: no order, no clock message, nothing changes.
//!
//! Runs the shipped `collector/assemble` script (`support/assemble_cell.rs`);
//! no model and no clock: every moment is fixed data in the round rows, and the
//! strike is a message like any other.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::*;
use serde_json::{Value, json};

const TURN: &str = "wake-channel#0001";
const REC: &str = "2999-01-01T00:00:00.000400Z";
const LATER: &str = "2999-01-01T00:00:00.100000Z";
const AGED: &str = "2000-01-01T00:00:00.000000Z";

fn knob(w: i64) -> Vec<(&'static str, Value)> {
    vec![("route_wait_ms", json!(w))]
}

fn row(role: &str, payload: Value, fired: i64, rec: &str) -> Value {
    json!({"turn_id": TURN, "iter": 0, "role": role, "turn": payload.to_string(),
           "fired": fired, "recorded_at": rec})
}

fn window(fired: i64, rec: &str) -> Value {
    row(
        "leg-window",
        json!({"turns": [{"role": "user", "text": "what is the weather", "consult_id": ""}],
               "deferred": 0, "deferred_turns": []}),
        fired,
        rec,
    )
}

fn memory(rec: &str) -> Value {
    row("leg-memory", json!({"system": {}, "messages": []}), 0, rec)
}

fn stamp(rec: &str) -> Value {
    row(
        "leg-route",
        json!({"turn_id": TURN, "path": "simple", "p": 0.9, "intent": "chat",
               "needs": [], "families": [], "duration": 400}),
        0,
        rec,
    )
}

fn collect(over: &[(&str, Value)], rows: Value) -> Vec<Value> {
    assemble(
        over,
        bundle_reply("collect", TURN, &[("c-collect-read", rows)]),
    )
}

/// The read-back the strike asks for: a bare select under phase `collect-wake`.
fn wake_reply(over: &[(&str, Value)], rows: Value) -> Vec<Value> {
    assemble(
        over,
        json!({
            "header": {"context": {"session_id": SESSION, "turn_id": TURN, "iter": "0",
                                   "col_phase": "collect-wake", "store_origin": "collector"},
                       "hop": {"operation": "select", "rows_affected": 1}},
            "messages": [{"origin": "tool", "type": "tool_result", "id": "c-wake",
                          "text": rows.to_string()}]
        }),
    )
}

/// The reply to the strike's claim bundle; `landed` is the insert's
/// `rows_affected` (1 = the claim is this strike's, 0 = a row stood).
fn claim_reply(over: &[(&str, Value)], rows: Value, landed: i64) -> Vec<Value> {
    let mut doc = bundle_reply(
        "collect-wake-claim",
        TURN,
        &[("c-wake-claim", json!([])), ("c-wake-read", rows)],
    );
    doc["results"][0]["operation"] = json!("insert");
    doc["results"][0]["rows_affected"] = json!(landed);
    assemble(over, doc)
}

fn clock(out: &[Value]) -> Vec<&Value> {
    out.iter()
        .filter(|m| m["header"]["route"] == "clock")
        .collect()
}

fn curates(out: &[Value]) -> Vec<&Value> {
    out.iter()
        .filter(|m| m["header"]["route"] == "curate" && m["header"]["spec"] != "1")
        .collect()
}

#[test]
fn gh1564_the_wake_fires_the_turn_without_its_stamp() {
    // GH #1584: the speculation is gone, one run without it.
    {
        let spec = "off";
        let over = knob(150);
        // Every leg but the stamp is in: the fan-in orders its wake at the
        // window leg + route_wait_ms, to the millisecond (rounded up).
        let out = collect(&over, json!([window(0, REC), memory(REC)]));
        assert!(
            curates(&out).is_empty(),
            "spec {spec}: no fire yet: {out:?}"
        );
        let orders = clock(&out);
        assert_eq!(orders.len(), 1, "spec {spec}: one order: {out:?}");
        let order = orders[0];
        assert_eq!(order["op"], "add", "{order}");
        assert_eq!(order["rearm"], true, "{order}");
        assert_eq!(order["schedule_name"], "route_wait", "{order}");
        assert_eq!(order["at"], "2999-01-01T00:00:00.151Z", "{order}");
        assert_eq!(order["emit_to"], "../assemble", "{order}");
        assert_eq!(order["emit_headers"]["turn_id"], TURN, "{order}");
        assert_eq!(order["emit_headers"]["session_id"], SESSION, "{order}");
        // Review R1 M2: a strike lost to a standstill fires once at the start.
        assert_eq!(order["catch_up"], true, "{order}");
        let id = order["schedule_id"].as_str().expect("id").to_string();

        // The strike reads the round back -- and nothing else.
        let out = assemble(
            &over,
            lane(
                "in_route_wake",
                json!({"turn_id": TURN, "session_id": SESSION, "schedule_name": "route_wait"}),
                json!({"turn_id": TURN}),
                json!([]),
            ),
        );
        assert_eq!(out.len(), 1, "spec {spec}: one read: {out:?}");
        assert_eq!(out[0]["header"]["route"], "cstore", "{out:?}");
        assert_eq!(out[0]["header"]["phase"], "collect-wake", "{out:?}");

        // Its read-back finds the turn waiting and claims it: the `leg-wake`
        // row if absent, and the round read back, in ONE bundle -- no fire yet.
        let out = wake_reply(&over, json!([window(0, REC), memory(REC)]));
        assert_eq!(out.len(), 1, "spec {spec}: one claim: {out:?}");
        assert_eq!(out[0]["header"]["phase"], "collect-wake-claim", "{out:?}");
        let ops = calls_of(&out[0]);
        assert_eq!(ops.len(), 2, "{out:?}");
        assert_eq!(ops[0].1["operation"], "insert", "{out:?}");
        assert_eq!(ops[0].1["row"]["role"], "leg-wake", "{out:?}");
        assert_eq!(
            ops[0].1["if_absent"],
            json!({"turn_id": TURN, "role": "leg-wake"}),
            "{out:?}"
        );
        assert_eq!(ops[1].1["operation"], "select", "{out:?}");

        // The claim landed: the turn fires without the stamp, however fresh
        // the window leg looks to the wall clock -- the bound is over.
        let rows = json!([
            window(0, REC),
            memory(REC),
            row("leg-wake", json!({}), 1, LATER)
        ]);
        let out = claim_reply(&over, rows.clone(), 1);
        assert_eq!(
            curates(&out).len(),
            1,
            "spec {spec}: the wake fires: {out:?}"
        );
        assert_eq!(hop_str(curates(&out)[0], "path"), "", "{out:?}");
        // The order struck: it is not taken back.
        assert!(clock(&out).is_empty(), "spec {spec}: {out:?}");
        // A claim that did not land (another strike's row stood) fires nothing.
        let out = claim_reply(&over, rows, 0);
        assert!(out.is_empty(), "spec {spec}: a lost claim: {out:?}");
        let _ = id;
    }
}

#[test]
fn gh1564_a_hold_moves_the_wake_to_its_end() {
    // A route-hold notice asks for more time: the wake strikes when it ends.
    let over = knob(150);
    let until = 32_472_144_000_000_i64 + 5_000; // 2999-01-01T00:00:05Z
    let hold = row("route-hold", json!({"until_ms": until}), 0, REC);
    let out = collect(&over, json!([window(0, REC), memory(REC), hold]));
    let orders = clock(&out);
    assert_eq!(orders.len(), 1, "{out:?}");
    assert_eq!(orders[0]["at"], "2999-01-01T00:00:05.000Z", "{out:?}");
}

#[test]
fn gh1564_no_bound_no_wake() {
    // route_wait_ms 0: no route leg, no order, the fan-in of before.
    let over = knob(0);
    let out = collect(&over, json!([window(0, REC), memory(REC)]));
    assert!(clock(&out).is_empty(), "{out:?}");
    assert_eq!(curates(&out).len(), 1, "{out:?}");
    // A strike nobody ordered parks.
    let out = assemble(
        &over,
        lane(
            "in_route_wake",
            json!({"turn_id": TURN, "session_id": SESSION}),
            json!({"turn_id": TURN}),
            json!([]),
        ),
    );
    assert!(out.is_empty(), "{out:?}");
}

#[test]
fn gh1564_a_strike_after_the_fire_does_nothing() {
    let over = knob(150);
    let out = wake_reply(&over, json!([window(1, REC), memory(REC), stamp(LATER)]));
    assert!(out.is_empty(), "{out:?}");
    // The order struck before its `remove` arrived: the refusal is no finding.
    let out = assemble(
        &over,
        lane(
            "in_route_wake_error",
            json!({"turn_id": TURN, "error_code": "schedule_not_found",
                   "msg_type": "timer_op_error"}),
            json!({"turn_id": TURN}),
            json!([]),
        ),
    );
    assert!(out.is_empty(), "{out:?}");
}

#[test]
fn gh1564_the_order_dies_with_its_wait() {
    let over = knob(150);
    let id = clock(&collect(&over, json!([window(0, REC), memory(REC)])))[0]["schedule_id"].clone();
    // The stamp lands last: the turn fires with it and takes the order back.
    let out = collect(&over, json!([window(0, REC), memory(REC), stamp(LATER)]));
    assert_eq!(curates(&out).len(), 1, "{out:?}");
    let orders = clock(&out);
    assert_eq!(orders.len(), 1, "{out:?}");
    assert_eq!(orders[0]["op"], "remove", "{out:?}");
    assert_eq!(orders[0]["schedule_id"], id, "{out:?}");
    // A late leg fires the turn past the bound -- on its won claim (re-check
    // m3) -- and the order goes with it.
    let rows = json!([window(0, AGED), memory(LATER)]);
    let out = then_age_claim(&over, collect(&over, rows.clone()), rows);
    assert_eq!(curates(&out).len(), 1, "{out:?}");
    assert_eq!(clock(&out).len(), 1, "{out:?}");
    assert_eq!(clock(&out)[0]["op"], "remove", "{out:?}");
    // A stamp that was in before the window leg: no order was ever made.
    let out = collect(&over, json!([stamp(REC), window(0, LATER), memory(LATER)]));
    assert_eq!(curates(&out).len(), 1, "{out:?}");
    assert!(clock(&out).is_empty(), "{out:?}");
}

#[test]
fn gh1564_the_clock_route_is_in_the_contract() {
    // The runtime checks every emission against `contract.emits`: a route
    // outside the declared enum is a `contract_violation` that drops the whole
    // batch, the fire beside the order included. Both orders (add, remove)
    // leave on a route the contract names.
    let over = knob(150);
    let mut seen = clock(&collect(&over, json!([window(0, REC), memory(REC)])))
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    seen.extend(
        clock(&collect(
            &over,
            json!([window(0, REC), memory(REC), stamp(LATER)]),
        ))
        .into_iter()
        .cloned(),
    );
    assert_eq!(seen.len(), 2, "{seen:?}");
    let cfg = config_of(ASSEMBLE);
    let routes = cfg["contract"]["emits"]["hop"]["route"]["values"]
        .as_array()
        .expect("route enum");
    for m in &seen {
        assert!(
            routes.contains(&m["header"]["route"]),
            "{m} not in {routes:?}"
        );
    }
}

#[test]
fn gh1564_a_hold_after_the_order_moves_the_strike() {
    // Review R1 M1: a route-hold notice that landed after the order and still
    // runs when the strike reads back -- the strike orders anew at its end and
    // claims nothing.
    let over = knob(150);
    let until = 32_472_144_000_000_i64 + 5_000; // 2999-01-01T00:00:05Z
    let hold = row("route-hold", json!({"until_ms": until}), 0, LATER);
    let out = wake_reply(&over, json!([window(0, REC), memory(REC), hold]));
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0]["header"]["route"], "clock", "{out:?}");
    assert_eq!(out[0]["op"], "add", "{out:?}");
    assert_eq!(out[0]["at"], "2999-01-01T00:00:05.000Z", "{out:?}");
}

#[test]
fn gh1564_the_order_carries_the_turns_context() {
    // Review R1 C1: the window leg keeps the turn's context, the order hands it
    // to the clock (`emit_headers.wake_ctx`), and the clock's edge lifts it
    // back. The unit half: what the order carries.
    let over = knob(150);
    let kept = json!({"channel": "web", "channel_node": "/m/channels/web",
                      "engine": "duplex", "audience_set": "a,b"});
    let win = row(
        "leg-window",
        json!({"turns": [{"role": "user", "text": "hi", "consult_id": ""}],
               "deferred": 0, "deferred_turns": [], "ctx": kept}),
        0,
        REC,
    );
    let out = collect(&over, json!([win, memory(REC)]));
    let orders = clock(&out);
    assert_eq!(orders.len(), 1, "{out:?}");
    assert_eq!(orders[0]["emit_headers"]["wake_ctx"], kept, "{out:?}");
}

/// The script's `WAKE_CTX_KEYS` tuple, read out of the shipped script.
fn script_wake_keys() -> Vec<String> {
    let cfg = config_of(ASSEMBLE);
    let script = cfg["params"]["script_inline"].as_str().expect("script");
    let at = script
        .find("WAKE_CTX_KEYS = (")
        .expect("WAKE_CTX_KEYS in the script");
    let rest = &script[at + "WAKE_CTX_KEYS = (".len()..];
    let body = &rest[..rest.find(')').expect("tuple end")];
    body.split(',')
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn find_wake_edge(v: &Value) -> Option<Value> {
    match v {
        Value::Object(o) => {
            if o.get("from") == Some(&json!("./clock"))
                && o.get("condition")
                    .and_then(Value::as_str)
                    .is_some_and(|c| c.contains("route_wait"))
            {
                return Some(v.clone());
            }
            o.values().find_map(find_wake_edge)
        }
        Value::Array(a) => a.iter().find_map(find_wake_edge),
        _ => None,
    }
}

#[test]
fn gh1564_the_wake_lifts_every_kept_key() {
    // One list, two places: the keys the script keeps in the window leg and
    // the keys the clock's edge lifts back into the strike's context.
    let keys = script_wake_keys();
    assert!(keys.contains(&"channel_node".to_string()), "{keys:?}");
    let edge = find_wake_edge(&config_of("templates/collector/config.json")).expect("wake edge");
    let set = edge["modifier"]["set_context"]
        .as_object()
        .expect("set_context");
    let mut lifted: Vec<String> = set
        .keys()
        .filter(|k| !["turn_id", "session_id", "speaker", "turn_round"].contains(&k.as_str()))
        .cloned()
        .collect();
    lifted.sort();
    let mut want = keys.clone();
    want.sort();
    assert_eq!(lifted, want, "edge and script disagree");
    for k in &keys {
        assert_eq!(
            set[k.as_str()],
            json!(format!(
                "has(hop.wake_ctx) && has(hop.wake_ctx.{k}) ? hop.wake_ctx.{k} : ''"
            )),
            "{k}"
        );
    }
    // OR-54g-274: the stamped keys, in the one form a config edge may write
    // them (`cel_eval::is_stamped_restore`), and dropped off the hop (c).
    for k in ["speaker", "turn_round"] {
        assert_eq!(
            set[k],
            json!(format!("has(hop.ctx_{k}) ? hop.ctx_{k} : ''")),
            "{k}"
        );
    }
    assert_eq!(
        edge["modifier"]["delete_hop"],
        json!(["wake_ctx", "ctx_speaker", "ctx_turn_round"]),
        "{edge}"
    );
}

// ---- OR-54g-274: the stamped keys of a woken turn (I2) ----

const OTHER: &str = "wake-channel#0002";
const ANNA: &str = "member:anna";
const ROUND: &str = "[\"agent:talky\",\"member:anna\"]";

/// The turn's opening with `speaker`/`turn_round` in its context, a forged pair
/// on the hop and in the body beside it; returns the parked window row's payload.
fn opened_window(w: i64) -> Value {
    opened_window_with(&knob(w))
}

fn opened_window_with(over: &[(&str, Value)]) -> Value {
    let rows = json!([{"id": "2999-01-01T00:00:00.000400Z-aaaa0001", "session_id": SESSION,
                       "turn_id": TURN, "role": "user", "content": "hi",
                       "deferred": 0, "consult_id": ""}]);
    let mut doc = bundle_reply(
        "turn-open",
        TURN,
        &[
            ("c-open-round", json!([])),
            ("c-open-win", rows),
            ("c-open-scope", json!([])),
            ("c-open-roster", json!([])),
        ],
    );
    doc["header"]["context"]["speaker"] = json!(ANNA);
    doc["header"]["context"]["turn_round"] = json!(ROUND);
    doc["header"]["context"]["channel"] = json!("web");
    doc["header"]["hop"]["speaker"] = json!("member:mallory");
    doc["header"]["hop"]["ctx_speaker"] = json!("member:mallory");
    doc["header"]["hop"]["turn_round"] = json!("[\"member:mallory\"]");
    doc["speaker"] = json!("member:mallory");
    doc["messages"][1]["speaker"] = json!("member:mallory");
    let out = assemble(over, doc);
    let row = call(in_phase(&out, "collect"), "c-collect-row")["row"].clone();
    assert_eq!(row["role"], "leg-window", "{row}");
    serde_json::from_str(row["turn"].as_str().expect("the payload")).expect("json")
}

#[test]
fn gh1564_the_park_comes_off_the_turns_own_context() {
    // (b): the stamped keys are parked ONLY off the context the wiring gave the
    // turn -- a hop key or a body field of the same name is never read.
    let win = opened_window(150);
    assert_eq!(
        win["park"],
        json!({"speaker": ANNA, "turn_round": ROUND, "turn_id": TURN}),
        "{win}"
    );
    // (c): a turn that never waits parks nothing.
    let flat = opened_window(0);
    assert!(flat.get("park").is_none(), "{flat}");
}

#[test]
fn gh1564_a_turn_that_never_waits_keeps_no_context() {
    // Re-check 2 m-C: the strike's context snapshot is kept only where a
    // strike can come (`route_wait_ms` > 0); at 0 the window row carries
    // neither `ctx` nor `park`, byte-equal to the line without the wake.
    let waits = opened_window(150);
    assert_eq!(waits["ctx"]["channel"], json!("web"), "{waits}");
    let flat = opened_window(0);
    assert!(flat.get("ctx").is_none(), "{flat}");
    assert!(flat.get("park").is_none(), "{flat}");
}

#[test]
fn gh1648_a_turn_bounded_only_by_its_recall_keeps_its_context() {
    // The recall order (GH #1596) carries the same snapshot as the route wake
    // and ships on; with `route_wait_ms` 0 (shipped) its strike used to come
    // back with an empty context and the answer dead-lettered.
    let knobs = [
        ("memory_tier", json!("1")),
        ("route_wait_ms", json!(0)),
        ("recall_wait_ms", json!(10000)),
    ];
    let win = opened_window_with(&knobs);
    assert_eq!(win["ctx"]["channel"], json!("web"), "{win}");
    assert_eq!(win["park"]["speaker"], json!(ANNA), "{win}");
    // The order the fan-in makes off that very row carries it to the clock.
    let out = collect(&knobs, json!([row("leg-window", win, 0, REC)]));
    let orders = clock(&out);
    assert_eq!(orders.len(), 1, "{out:?}");
    assert_eq!(orders[0]["schedule_name"], "recall_wait", "{out:?}");
    let headers = &orders[0]["emit_headers"];
    assert_eq!(headers["wake_ctx"]["channel"], json!("web"), "{headers}");
    assert_eq!(headers["ctx_speaker"], json!(ANNA), "{headers}");
    assert_eq!(headers["ctx_turn_round"], json!(ROUND), "{headers}");

    // Without a memory leg, or without its bound, nothing can strike.
    for off in [("memory_tier", json!("")), ("recall_wait_ms", json!(0))] {
        let mut knobs = knobs.to_vec();
        knobs.push(off);
        let flat = opened_window_with(&knobs);
        assert!(flat.get("ctx").is_none(), "{flat}");
        assert!(flat.get("park").is_none(), "{flat}");
    }
}

fn parked_window(park: Value) -> Value {
    row(
        "leg-window",
        json!({"turns": [{"role": "user", "text": "hi", "consult_id": ""}],
               "deferred": 0, "deferred_turns": [], "ctx": {"channel": "web"},
               "park": park}),
        0,
        REC,
    )
}

#[test]
fn gh1564_the_order_hands_on_only_its_own_turns_park() {
    // (a): the park names its turn, and the order hands it to the clock only
    // for that turn. A park of another turn (a row the read-back should never
    // hold, but a store that mixed them up) gives this turn no speaker.
    let over = knob(150);
    let own = collect(
        &over,
        json!([
            parked_window(json!({"speaker": ANNA, "turn_round": ROUND, "turn_id": TURN})),
            memory(REC)
        ]),
    );
    let order = clock(&own);
    assert_eq!(order.len(), 1, "{own:?}");
    let h = &order[0]["emit_headers"];
    assert_eq!(h["ctx_speaker"], ANNA, "{h}");
    assert_eq!(h["ctx_turn_round"], ROUND, "{h}");
    assert_eq!(h["turn_id"], TURN, "{h}");

    let foreign = collect(
        &over,
        json!([
            parked_window(json!({"speaker": "member:bob", "turn_round": ROUND,
                                    "turn_id": OTHER})),
            memory(REC)
        ]),
    );
    let order = clock(&foreign);
    assert_eq!(order.len(), 1, "{foreign:?}");
    let h = order[0]["emit_headers"].as_object().expect("headers");
    assert!(
        !h.contains_key("ctx_speaker") && !h.contains_key("ctx_turn_round"),
        "another turn's park reached this turn's strike: {h:?}"
    );
}

#[test]
fn gh1564_only_the_clock_hands_back_a_parked_key() {
    // The privilege behind the restore (`contract.parks_context`, GH #979): the
    // collector's clock holds it, so the colony lets its `ctx_<stamped>` pass;
    // the script cell that builds the order does not need it and has none.
    let clock_cfg = config_of("templates/collector/clock/config.json");
    assert_eq!(clock_cfg["contract"]["parks_context"], true, "{clock_cfg}");
    let asm = config_of(ASSEMBLE);
    assert!(asm["contract"].get("parks_context").is_none());
}

// ---- Re-check m3: a fire on the window leg's age takes the strike's claim ----

#[test]
fn gh1564_an_aged_fire_takes_the_claim_first() {
    let over = knob(150);
    let rows = json!([window(0, AGED), memory(REC)]);
    // The complete set past the bound: no seam yet, the claim instead.
    let out = collect(&over, rows.clone());
    assert!(
        curates(&out).is_empty(),
        "an aged fire went out unclaimed: {out:?}"
    );
    let claim = in_phase(&out, "collect-age-claim");
    let calls = calls_of(claim);
    assert_eq!(calls[0].1["if_absent"]["role"], "leg-wake", "{claim}");

    // Its claim landed: the turn fires and takes the order back.
    let mut doc = bundle_reply(
        "collect-age-claim",
        TURN,
        &[("c-wake-claim", json!([])), ("c-wake-read", rows.clone())],
    );
    doc["results"][0]["operation"] = json!("insert");
    doc["results"][0]["rows_affected"] = json!(1);
    let won = assemble(&over, doc.clone());
    assert_eq!(curates(&won).len(), 1, "{won:?}");
    assert!(
        clock(&won).iter().any(|m| m["op"] == "remove"),
        "a won age claim takes the order back: {won:?}"
    );

    // The strike claimed first: nothing.
    doc["results"][0]["rows_affected"] = json!(0);
    let lost = assemble(&over, doc);
    assert!(curates(&lost).is_empty(), "{lost:?}");
}

// ---- Review R1 I1: of a stamp and a strike exactly one fires ----

fn race_knob() -> Vec<(&'static str, Value)> {
    vec![("route_wait_ms", json!(150)), ("memory_tier", json!("1"))]
}

fn strike_in() -> Value {
    lane(
        "in_route_wake",
        json!({"turn_id": TURN, "session_id": SESSION, "schedule_name": "route_wait"}),
        json!({"turn_id": TURN}),
        json!([]),
    )
}

fn stamp_in() -> Value {
    let mut v = lane(
        "in_route",
        json!({"turn_id": TURN}),
        json!({"turn_id": TURN}),
        json!([]),
    );
    v["stamp"] = json!({"turn_id": TURN, "path": "simple", "p": 0.9, "intent": "chat",
                        "needs": [], "families": [], "duration": 400});
    v
}

/// The memory leg's park as `round_park` writes it: its row and the read-back.
fn memory_park() -> Value {
    let row = json!({"turn_id": TURN, "session_id": SESSION, "iter": 0,
                     "role": "leg-memory",
                     "turn": json!({"system": {}, "messages": []}).to_string(),
                     "fired": 0, "recorded_at": LATER});
    let read = json!({"operation": "select", "table": "round",
                      "columns": ["turn_id", "iter", "role", "turn", "fired", "recorded_at"],
                      "where": {"turn_id": TURN}});
    json!({"header": {"route": "cstore", "phase": "collect", "turn_id": TURN,
                      "session_id": SESSION, "iter": "0", "gate": ""},
           "messages": [
               {"origin": "assistant", "type": "tool_call", "id": "c-collect-row",
                "text": json!({"operation": "insert", "table": "round", "row": row}).to_string()},
               {"origin": "assistant", "type": "tool_call", "id": "c-collect-read",
                "text": read.to_string()}]})
}

/// The one store op `doc` makes the cell send, with the context it ran in.
fn one_op(over: &[(&str, Value)], doc: Value) -> (Value, Value) {
    let ctx = doc["header"]["context"].clone();
    let out = assemble(over, doc);
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0]["header"]["route"], "cstore", "{out:?}");
    (out[0].clone(), ctx)
}

/// The strike up to its claim: the read-back runs, the claim is built.
fn strike_to_claim(over: &[(&str, Value)], store: &mut WindowStore) -> (Value, Value) {
    let (sel, ctx) = one_op(over, strike_in());
    let back = store.reply(&sel, &ctx);
    let (claim, ctx) = one_op(over, back);
    assert_eq!(claim["header"]["phase"], "collect-wake-claim", "{claim}");
    (claim, ctx)
}

fn stamp_park(over: &[(&str, Value)]) -> (Value, Value) {
    let (park, ctx) = one_op(over, stamp_in());
    assert_eq!(park["header"]["gate"], "leg-route", "{park}");
    (park, ctx)
}

fn fired(left: &[Value]) -> Vec<String> {
    curates(left).iter().map(|m| hop_str(m, "path")).collect()
}

#[test]
fn gh1564_a_stamp_and_a_strike_fire_once_in_either_order() {
    let over = race_knob();
    for claim_first in [true, false] {
        let mut store = WindowStore::holding("round", vec![window(0, REC), memory(REC)]);
        let (claim, claim_ctx) = strike_to_claim(&over, &mut store);
        let (stamp, stamp_ctx) = stamp_park(&over);
        // Both bundles run in the store before either reply is handled: the
        // `fired` mark of the first fire is not in yet when the second reads.
        let (a, b) = if claim_first {
            let a = store.reply(&claim, &claim_ctx);
            (a, store.reply(&stamp, &stamp_ctx))
        } else {
            let b = store.reply(&stamp, &stamp_ctx);
            (store.reply(&claim, &claim_ctx), b)
        };
        let mut left = drive_with(&over, &mut store, a);
        left.extend(drive_with(&over, &mut store, b));
        let paths = fired(&left);
        // The first bundle decided: the strike fires without the stamp, the
        // stamp with its path -- and only one of them.
        let want = if claim_first { "" } else { "simple" };
        assert_eq!(
            paths,
            vec![want.to_string()],
            "claim_first {claim_first}: {left:?}"
        );
        assert!(
            left.iter().all(|m| m["header"]["spec"] != "1"),
            "no speculation: {left:?}"
        );
    }
}

#[test]
fn gh1564_a_strike_short_of_a_leg_then_the_stamp_then_the_memory_fire_once() {
    let over = race_knob();
    let mut store = WindowStore::holding("round", vec![window(0, REC)]);
    // The strike claims while the memory leg is still out: nothing fires.
    let (claim, ctx) = strike_to_claim(&over, &mut store);
    let back = store.reply(&claim, &ctx);
    let left = drive_with(&over, &mut store, back);
    assert!(fired(&left).is_empty(), "{left:?}");
    // The stamp reads the claim beside it and parks.
    let (stamp, ctx) = stamp_park(&over);
    let back = store.reply(&stamp, &ctx);
    let left = drive_with(&over, &mut store, back);
    assert!(fired(&left).is_empty(), "the stamp parks: {left:?}");
    // The memory leg completes the round, once, with the stamp that is in.
    let back = store.reply(
        &memory_park(),
        &json!({"session_id": SESSION, "turn_id": TURN}),
    );
    let left = drive_with(&over, &mut store, back);
    assert_eq!(fired(&left), vec!["simple".to_string()], "{left:?}");
    assert!(
        left.iter().all(|m| m["header"]["spec"] != "1"),
        "no speculation: {left:?}"
    );
}
