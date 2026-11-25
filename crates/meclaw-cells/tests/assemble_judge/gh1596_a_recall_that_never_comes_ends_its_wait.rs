//! GH #1596 (d54s nachlauf, P1) -- the memory leg has a bound, and its end is said.
//!
//! WHY this file exists: a chat turn on path `talk` with `memory_tier` "1" waited
//! for its memory leg forever when the recall never came back (lab P1 on set
//! 0.73, trace 01a202c6: the recall ended as a `hive_no_route` dead letter at the
//! surface's rim, and a dead letter is never answered to its sender). No
//! `curate`, no brain call, no answer -- silence. Now the fan-in that waits for the
//! memory leg orders ONE one-shot of the collector's clock at the window leg +
//! `recall_wait_ms`; its strike claims the turn (`leg-recall-wake`) and the turn
//! goes on to the curator without the recall, with `hop.recall_missing` "1" on
//! the seam and a WARN line on stderr. A recall that comes takes the order back
//! and the turn is the one of before.
//!
//! Runs the shipped `collector/assemble` script (`support/assemble_cell.rs`);
//! no model and no clock: every moment is fixed data in the round rows, and the
//! strike is a message like any other.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::*;
use serde_json::{Value, json};

const TURN: &str = "recall-channel#0001";
const REC: &str = "2999-01-01T00:00:00.000400Z";
const LATER: &str = "2999-01-01T00:00:00.100000Z";
const AGED: &str = "2000-01-01T00:00:00.000000Z";

fn knobs(bound: i64) -> Vec<(&'static str, Value)> {
    vec![
        ("memory_tier", json!("1")),
        ("route_wait_ms", json!(150)),
        ("recall_wait_ms", json!(bound)),
    ]
}

fn row(role: &str, payload: Value, fired: i64, rec: &str) -> Value {
    json!({"turn_id": TURN, "iter": 0, "role": role, "turn": payload.to_string(),
           "fired": fired, "recorded_at": rec})
}

fn window(fired: i64, rec: &str) -> Value {
    row(
        "leg-window",
        json!({"turns": [{"role": "user", "text": "Hello, how are you?", "consult_id": ""}],
               "deferred": 0, "deferred_turns": []}),
        fired,
        rec,
    )
}

fn memory(rec: &str) -> Value {
    row("leg-memory", json!({"system": {}, "messages": []}), 0, rec)
}

fn talk(rec: &str) -> Value {
    row(
        "leg-route",
        json!({"turn_id": TURN, "path": "talk", "p": 0.9, "intent": "chat",
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

/// The strike's read-back: a bare select under phase `collect-recall-wake`.
fn wake_reply(over: &[(&str, Value)], rows: Value) -> Vec<Value> {
    assemble(
        over,
        json!({
            "header": {"context": {"session_id": SESSION, "turn_id": TURN, "iter": "0",
                                   "col_phase": "collect-recall-wake",
                                   "store_origin": "collector"},
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
        "collect-recall-claim",
        TURN,
        &[("c-recall-claim", json!([])), ("c-recall-read", rows)],
    );
    doc["results"][0]["operation"] = json!("insert");
    doc["results"][0]["rows_affected"] = json!(landed);
    assemble(over, doc)
}

fn recall_orders(out: &[Value]) -> Vec<&Value> {
    out.iter()
        .filter(|m| {
            m["header"]["route"] == "clock"
                && (m["schedule_name"] == "recall_wait" || m["op"] == "remove")
        })
        .collect()
}

fn curates(out: &[Value]) -> Vec<&Value> {
    out.iter()
        .filter(|m| m["header"]["route"] == "curate")
        .collect()
}

fn strike() -> Value {
    lane(
        "in_recall_wake",
        json!({"turn_id": TURN, "session_id": SESSION, "schedule_name": "recall_wait"}),
        json!({"turn_id": TURN}),
        json!([]),
    )
}

/// P1: the stamp says `talk`, the recall never comes. The turn reaches the
/// curator without it, and the seam says so.
#[test]
fn gh1596_a_recall_that_never_comes_ends_its_wait_and_says_so() {
    let over = knobs(5000);
    // Window and stamp are in, the memory leg is not: no fire, one order.
    let out = collect(&over, json!([window(0, REC), talk(REC)]));
    assert!(curates(&out).is_empty(), "the turn waits: {out:?}");
    let orders: Vec<&Value> = out
        .iter()
        .filter(|m| m["header"]["route"] == "clock" && m["schedule_name"] == "recall_wait")
        .collect();
    assert_eq!(orders.len(), 1, "one recall order: {out:?}");
    let order = orders[0];
    assert_eq!(order["op"], "add", "{order}");
    assert_eq!(order["rearm"], true, "{order}");
    assert_eq!(order["catch_up"], true, "{order}");
    assert_eq!(order["emit_to"], "../assemble", "{order}");
    assert_eq!(order["emit_headers"]["turn_id"], TURN, "{order}");
    assert_eq!(order["emit_headers"]["session_id"], SESSION, "{order}");
    assert!(
        order["after_ms"].as_i64().is_some_and(|ms| ms >= 1),
        "a lead time, never a moment the timer may refuse as past: {order}"
    );
    assert!(order.get("at").is_none(), "{order}");

    // The strike reads the round back -- and nothing else.
    let out = assemble(&over, strike());
    assert_eq!(out.len(), 1, "one read: {out:?}");
    assert_eq!(out[0]["header"]["route"], "cstore", "{out:?}");
    assert_eq!(out[0]["header"]["phase"], "collect-recall-wake", "{out:?}");

    // The read-back finds the memory leg missing and claims the turn.
    let out = wake_reply(&over, json!([window(0, REC), talk(REC)]));
    assert_eq!(out.len(), 1, "one claim: {out:?}");
    assert_eq!(out[0]["header"]["phase"], "collect-recall-claim", "{out:?}");
    let ops = calls_of(&out[0]);
    assert_eq!(ops.len(), 2, "{out:?}");
    assert_eq!(ops[0].1["operation"], "insert", "{out:?}");
    assert_eq!(ops[0].1["row"]["role"], "leg-recall-wake", "{out:?}");
    assert_eq!(
        ops[0].1["if_absent"],
        json!({"turn_id": TURN, "role": "leg-recall-wake"}),
        "{out:?}"
    );
    assert_eq!(ops[1].1["operation"], "select", "{out:?}");

    // The claim landed: the turn goes on to the curator without the recall,
    // on its stamped path, and the seam carries the mark.
    let rows = json!([
        window(0, REC),
        talk(REC),
        row("leg-recall-wake", json!({}), 1, LATER)
    ]);
    let (out, stderr) = run_cell(ASSEMBLE, &over, {
        let mut doc = bundle_reply(
            "collect-recall-claim",
            TURN,
            &[
                ("c-recall-claim", json!([])),
                ("c-recall-read", rows.clone()),
            ],
        );
        doc["results"][0]["operation"] = json!("insert");
        doc["results"][0]["rows_affected"] = json!(1);
        doc
    });
    let seam = curates(&out);
    assert_eq!(seam.len(), 1, "the turn reaches the curator: {out:?}");
    assert_eq!(hop_str(seam[0], "path"), "talk", "{out:?}");
    assert_eq!(hop_str(seam[0], "recall_missing"), "1", "the mark: {out:?}");
    assert!(
        stderr.contains("WARN") && stderr.contains("recall_wait_ms") && stderr.contains(TURN),
        "the end of the wait is said on stderr: {stderr}"
    );
    // The order struck: nothing to take back.
    assert!(recall_orders(&out).is_empty(), "{out:?}");

    // A claim that did not land fires nothing.
    let out = claim_reply(&over, rows.clone(), 0);
    assert!(out.is_empty(), "a lost claim: {out:?}");

    // A later leg reads the claim row: the memory leg is no longer waited for.
    let out = collect(
        &over,
        json!([
            window(0, REC),
            row("leg-recall-wake", json!({}), 1, LATER),
            talk(LATER)
        ]),
    );
    assert_eq!(curates(&out).len(), 1, "{out:?}");
    assert_eq!(hop_str(curates(&out)[0], "recall_missing"), "1", "{out:?}");
}

/// The window leg is already past the bound when the order is made: the lead
/// is the smallest one, the strike comes at once.
#[test]
fn gh1596_a_bound_already_over_strikes_at_once() {
    let over = knobs(5000);
    let out = collect(&over, json!([window(0, AGED), talk(AGED)]));
    let order = out
        .iter()
        .find(|m| m["header"]["route"] == "clock" && m["schedule_name"] == "recall_wait")
        .unwrap_or_else(|| panic!("one recall order: {out:?}"));
    assert_eq!(order["after_ms"], 1, "{order}");
}

/// Gegenprobe: the recall comes -- the turn fires with it as before, without
/// the mark, and takes its order back; a strike after the fire does nothing.
#[test]
fn gh1596_a_recall_that_comes_fires_as_before() {
    let over = knobs(5000);
    let id = out_order_id(&over);
    let out = collect(&over, json!([window(0, REC), talk(REC), memory(LATER)]));
    let seam = curates(&out);
    assert_eq!(seam.len(), 1, "{out:?}");
    assert_eq!(hop_str(seam[0], "path"), "talk", "{out:?}");
    assert!(
        seam[0]["header"].get("recall_missing").is_none(),
        "no mark when the recall came: {out:?}"
    );
    let removes: Vec<&Value> = out
        .iter()
        .filter(|m| m["header"]["route"] == "clock" && m["op"] == "remove")
        .collect();
    assert_eq!(removes.len(), 1, "the order is taken back: {out:?}");
    assert_eq!(removes[0]["schedule_id"], id, "{out:?}");

    // A strike after the fire finds the window fired and parks.
    let out = wake_reply(&over, json!([window(1, REC), talk(REC), memory(LATER)]));
    assert!(out.is_empty(), "{out:?}");
    // A strike that finds the memory leg in (the fire not yet marked) parks too.
    let out = wake_reply(&over, json!([window(0, REC), memory(LATER)]));
    assert!(out.is_empty(), "{out:?}");
}

fn out_order_id(over: &[(&str, Value)]) -> Value {
    let out = collect(over, json!([window(0, REC), talk(REC)]));
    out.iter()
        .find(|m| m["header"]["route"] == "clock" && m["schedule_name"] == "recall_wait")
        .unwrap_or_else(|| panic!("one recall order: {out:?}"))["schedule_id"]
        .clone()
}

/// `recall_wait_ms` 0: no order, no strike -- the fan-in of before. Without
/// `memory_tier` there is no memory leg and nothing to bound either.
#[test]
fn gh1596_no_bound_no_order() {
    let out = collect(&knobs(0), json!([window(0, REC), talk(REC)]));
    assert!(
        out.iter().all(|m| m["header"]["route"] != "clock"),
        "{out:?}"
    );
    assert!(assemble(&knobs(0), strike()).is_empty());
    let out = collect(
        &[
            ("route_wait_ms", json!(150)),
            ("recall_wait_ms", json!(5000)),
        ],
        json!([window(0, REC), talk(REC)]),
    );
    assert_eq!(
        curates(&out).len(),
        1,
        "no memory tier, no memory leg: {out:?}"
    );
    assert!(
        out.iter().all(|m| m["header"]["route"] != "clock"),
        "{out:?}"
    );
}

/// The bound ships on: params, setting and script fallback say the same
/// value, the collector's own operation bound (`external_timeout_ms`).
#[test]
fn gh1596_the_bound_ships_on() {
    let cfg = config_of(ASSEMBLE);
    assert_eq!(
        cfg["params"]["recall_wait_ms"],
        json!(10000),
        "{}",
        cfg["params"]
    );
    assert_eq!(
        cfg["params"]["recall_wait_ms"], cfg["params"]["external_timeout_ms"],
        "the value of the cell's operation bound"
    );
    assert_eq!(
        cfg["contract"]["settings"]["recall_wait_ms"]["default"],
        json!(10000),
        "the setting says the same"
    );
    let src = cfg["params"]["script_inline"].as_str().expect("script");
    assert!(
        src.contains(r#"_int("recall_wait_ms", 10000)"#),
        "the script's fallback"
    );
    assert!(
        cfg["contract"]["emits"]["hop"]["recall_missing"].is_object(),
        "the mark is declared"
    );
}

/// The strike reaches the cell: the hive's edge from `./clock` lifts a
/// `recall_wait` strike into `in_recall_wake` with the turn's context.
#[test]
fn gh1596_the_hive_carries_the_strike_in() {
    let hive = config_of("templates/collector/config.json");
    let edges = hive["params"]["initial_edges"]["edges"]
        .as_array()
        .or_else(|| hive["params"]["edges"].as_array())
        .cloned()
        .unwrap_or_else(|| find_edges(&hive));
    let edge = edges
        .iter()
        .find(|e| {
            e["from"] == "./clock"
                && e["to"] == "./assemble"
                && e["condition"]
                    .as_str()
                    .unwrap_or("")
                    .contains("'recall_wait'")
        })
        .unwrap_or_else(|| panic!("an edge for the recall strike: {edges:?}"));
    let route = edge["modifier"]["set_hop"]["route"].as_str().unwrap_or("");
    assert!(route.contains("in_recall_wake"), "{edge}");
    assert!(
        edge["modifier"]["set_context"]["channel"].is_string(),
        "the turn's context comes back with the strike: {edge}"
    );
}

fn simple(rec: &str) -> Value {
    row(
        "leg-route",
        json!({"turn_id": TURN, "path": "simple", "p": 0.9, "intent": "chat",
               "needs": [], "families": [], "duration": 400}),
        0,
        rec,
    )
}

fn claim_row(rec: &str) -> Value {
    row("leg-recall-wake", json!({}), 1, rec)
}

/// The memory leg's own park-and-read, as the store hands it back: the gate
/// it carried beside its op (`hop.gate`) comes back as `context.col_gate`.
fn memory_reply(over: &[(&str, Value)], rows: Value) -> Vec<Value> {
    let mut doc = bundle_reply("collect", TURN, &[("c-collect-read", rows)]);
    doc["header"]["context"]["col_gate"] = json!("leg-memory");
    assemble(over, doc)
}

fn recall_removes<'a>(out: &'a [Value], id: &Value) -> Vec<&'a Value> {
    out.iter()
        .filter(|m| {
            m["header"]["route"] == "clock" && m["op"] == "remove" && &m["schedule_id"] == id
        })
        .collect()
}

const LATEST: &str = "2999-01-01T00:00:00.200000Z";

/// Review C M1: the memory leg parks with its gate, so its read-back names it.
#[test]
fn gh1596_the_memory_leg_carries_its_gate() {
    let out = assemble(
        &knobs(5000),
        lane(
            "in_bundle",
            json!({"turn_id": TURN}),
            json!({"turn_id": TURN}),
            json!([]),
        ),
    );
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0]["header"]["gate"], "leg-memory", "{out:?}");
}

/// Review C M1, order 1: the strike's claim ran first in the store and fired
/// without the memory leg; the memory leg's bundle, queued behind it, reads
/// the claim and its own row before the `fired` mark landed -- and parks.
/// Exactly one turn.
#[test]
fn gh1596_the_claim_first_then_the_memory_leg_is_one_turn() {
    let over = knobs(5000);
    let out = claim_reply(
        &over,
        json!([window(0, REC), talk(REC), claim_row(LATER)]),
        1,
    );
    assert_eq!(curates(&out).len(), 1, "the claim fires: {out:?}");
    let out = memory_reply(
        &over,
        json!([window(0, REC), talk(REC), claim_row(LATER), memory(LATEST)]),
    );
    assert!(
        curates(&out).is_empty(),
        "the late memory leg parks: {out:?}"
    );
    // A leg of another kind that completes the set later fires as usual,
    // with the memory leg and without the mark.
    let out = collect(
        &over,
        json!([
            window(0, REC),
            claim_row(LATER),
            memory(LATER),
            talk(LATEST)
        ]),
    );
    assert_eq!(curates(&out).len(), 1, "{out:?}");
    assert!(
        curates(&out)[0]["header"].get("recall_missing").is_none(),
        "{out:?}"
    );
}

/// Review C M1, order 2: the memory leg ran first and fired with the recall;
/// the strike's claim, queued behind it, lands and reads both rows -- and
/// parks. Exactly one turn.
#[test]
fn gh1596_the_memory_leg_first_then_the_claim_is_one_turn() {
    let over = knobs(5000);
    let out = memory_reply(&over, json!([window(0, REC), talk(REC), memory(LATER)]));
    assert_eq!(curates(&out).len(), 1, "the memory leg fires: {out:?}");
    let out = claim_reply(
        &over,
        json!([window(0, REC), talk(REC), memory(LATER), claim_row(LATEST)]),
        1,
    );
    assert!(curates(&out).is_empty(), "the claim parks: {out:?}");
}

/// Review C m1: a stamp that needs no recall ends the wait for the memory
/// leg -- a strike after it claims nothing, and a claim that stood before it
/// fires without the mark and without the WARN.
#[test]
fn gh1596_a_stamp_without_recall_owes_no_mark() {
    let over = knobs(5000);
    let out = wake_reply(&over, json!([window(0, REC), simple(LATER)]));
    assert!(out.is_empty(), "no claim, nothing was waited for: {out:?}");
    let (out, stderr) = run_cell(
        ASSEMBLE,
        &over,
        bundle_reply(
            "collect",
            TURN,
            &[(
                "c-collect-read",
                json!([window(0, REC), claim_row(LATER), simple(LATEST)]),
            )],
        ),
    );
    assert_eq!(curates(&out).len(), 1, "{out:?}");
    assert!(
        curates(&out)[0]["header"].get("recall_missing").is_none(),
        "{out:?}"
    );
    assert!(!stderr.contains("WARN"), "{stderr}");
}

/// Review C delta m4: the strike read the round before a stamp that needs no
/// recall, and ordered its claim; the stamp ran first in the store and fired
/// without the memory leg. The claim, queued behind it, reads a complete set
/// (window, stamp, claim) -- and parks, because nothing waits for the memory
/// leg any more. Exactly one turn.
#[test]
fn gh1596_a_stamp_first_then_the_claim_is_one_turn() {
    let over = knobs(5000);
    let out = collect(&over, json!([window(0, REC), simple(LATER)]));
    assert_eq!(curates(&out).len(), 1, "the stamp fires: {out:?}");
    let out = claim_reply(
        &over,
        json!([window(0, REC), simple(LATER), claim_row(LATEST)]),
        1,
    );
    assert!(curates(&out).is_empty(), "the claim parks: {out:?}");
}

/// Review C m2: a fire takes back only an order that stands. A stamp read
/// before the window leg's read made none; one after it takes the order back
/// with its own read-back, once.
#[test]
fn gh1596_a_fire_takes_back_only_an_order_that_stands() {
    let over = knobs(5000);
    let id = out_order_id(&over);
    let out = collect(&over, json!([window(0, LATER), simple(REC)]));
    assert_eq!(curates(&out).len(), 1, "{out:?}");
    assert!(
        recall_removes(&out, &id).is_empty(),
        "no order, no remove: {out:?}"
    );
    let out = collect(&over, json!([window(0, REC), simple(LATER)]));
    assert_eq!(curates(&out).len(), 1, "{out:?}");
    assert_eq!(
        recall_removes(&out, &id).len(),
        1,
        "the order is taken back: {out:?}"
    );
}

/// Every `edges` array anywhere in the hive config (the shape nests them).
fn find_edges(v: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                if k == "edges" {
                    if let Some(a) = x.as_array() {
                        out.extend(a.iter().cloned());
                    }
                } else {
                    out.extend(find_edges(x));
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| out.extend(find_edges(x))),
        _ => {}
    }
    out
}
