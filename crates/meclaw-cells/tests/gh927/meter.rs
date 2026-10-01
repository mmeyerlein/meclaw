//! GH #927 — the meter measures a role goal by its own counts, closes every
//! ask nobody answered, and hands the judge its open hints as hypotheses.
//!
//! Two kinds of table test, both against the shipped `script_inline`: the pure
//! functions through [`call`], and the phases of the flow through [`run`] over
//! the document one delivery would hand the cell. What is pinned is what can
//! be wrong quietly: a rate that reads zero where nothing was counted, an ask
//! that stays open forever, a hint that turns into a measurement, and a judge
//! that is asked on a window nobody measured.

use super::harness::{
    METER, assert_the_declaration_admits, call, call_with_params, can_run, config, on_route, run,
    script, shipped_params, store_answer, store_call, store_calls,
};
use meclaw_core::serde_json::{self, Value, json};

// ───────────────────────────────────────────────────────────────── fixtures

/// A goal measured at a role: its numbers come from that role's own counts.
fn talky_goal() -> Value {
    json!({"id": "goal:talky-gap_rate", "metric": "gap_rate", "source": "talky",
           "direction": "lower", "window_minutes": 1440, "min_samples": 30,
           "min_delta_pct": 10, "quality_gate": "tool_error_rate", "enabled": 1})
}

/// A goal measured at the colony's own ledger, as every goal was before.
fn colony_goal() -> Value {
    json!({"id": "goal:llm-cost", "metric": "llm_cost", "source": "colony",
           "direction": "lower", "window_minutes": 60, "min_samples": 30,
           "min_delta_pct": 10, "quality_gate": "answer_quality", "enabled": 1})
}

fn rules() -> Value {
    json!([
        {"id": "rule:radius", "kind": "radius", "value": "model,numeric_params,text_slots"},
        {"id": "rule:quality-floor", "kind": "quality_floor_pct", "value": "0"}
    ])
}

/// The `stats` object of an answer, in the form the answering side sends.
fn stats(calls: u64, gap: u64, tool_error: u64, truncated: bool) -> Value {
    json!({
        "from": "2026-09-29T00:00:00Z", "to": "2026-09-30T00:00:01Z",
        "counts": {"calls": calls, "gap": gap, "tool_error": tool_error},
        "samples": [{"at": "2026-09-29T10:00:00Z", "kind": "gap", "session_id": "s1",
                     "turn_id": "t1", "value": "searched"}],
        "truncated": truncated
    })
}

/// What the `in_stats` branch puts in the carry: the answer's `stats` beside
/// its hop's `error_code` and `detail`.
fn answer(stats: Value, error_code: &str) -> Value {
    json!({"stats": stats, "error_code": error_code, "detail": "",
           "stats_tag": "wait:s1"})
}

/// The carry a `stats` wait row remembers for one half of the loop.
fn wait_carry(ask: &str, goal: Value, row: Value) -> Value {
    let mut c = json!({"goal": goal, "prices": {}, "rules": rules(),
                       "cycle_id": "cycle:7", "ask": ask});
    if !row.is_null() {
        c["row"] = row;
    }
    c
}

/// The meter resumed in phase `wait`: the `waits` row comes back in the rows,
/// the answer travels in `ar_carry`.
fn resume(kind: &str, carry_of_the_row: Value, answer: Value, params: Option<Value>) -> Vec<Value> {
    let rows = json!([{"id": "wait:s1", "at": "2026-01-01T00:00:00.000000Z", "kind": kind,
                       "carry": carry_of_the_row, "status": "open"}]);
    let mut doc = store_answer(rows, "wait", answer);
    if let Some(p) = params {
        doc["params"] = p;
    }
    run(METER, doc)
}

/// A hop value that is JSON text, parsed.
fn parsed(v: &Value) -> Value {
    serde_json::from_str(
        v.as_str()
            .unwrap_or_else(|| panic!("JSON text expected: {v}")),
    )
    .unwrap_or_else(|e| panic!("not JSON ({e}): {v}"))
}

/// The row of the one `cycles` insert.
fn inserted_cycle(out: &[Value]) -> Value {
    store_call(out, "insert", "cycles")
        .unwrap_or_else(|| panic!("a cycles row is written: {out:?}"))["row"]
        .clone()
}

/// The text the judge is shown, parsed.
fn judge_text(m: &Value) -> Value {
    parsed(&m["messages"][0]["text"])
}

/// An applied role cycle as the `open` phase remembers it.
fn applied_row(gap_rate: f64, tool_error_rate: f64) -> Value {
    json!({"id": "cycle:7", "goal": "goal:talky-gap_rate",
           "at": "2026-01-01T00:00:00.000000Z", "status": "applied",
           "measured": {"rates": {"gap_rate": gap_rate, "tool_error_rate": tool_error_rate}},
           "change": {"target": "/main/talky/brain", "kind": "model", "to": "b"},
           "revert_plan": {"target": "/main/talky/brain", "kind": "model", "to": "a"}})
}

// ─────────────────────────────────────────────────────────── pure functions

#[test]
fn the_role_of_a_goal_is_its_source_unless_that_names_the_colony() {
    if !can_run() {
        return;
    }
    for (goal, role) in [
        (json!({"source": "talky"}), "talky"),
        (json!({"source": "cogny"}), "cogny"),
        (json!({"source": "colony"}), ""),
        (json!({"source": ""}), ""),
        (json!({}), ""),
    ] {
        assert_eq!(
            call(METER, "role_of", json!([goal])),
            json!(role),
            "a charter row written before `source` existed keeps meaning the colony: {goal}"
        );
    }
}

#[test]
fn stats_kinds_asks_for_calls_the_metric_and_the_gate_once_each() {
    if !can_run() {
        return;
    }
    let mut own_gate = talky_goal();
    own_gate["metric"] = json!("tool_error_rate");
    let mut observed = talky_goal();
    observed["metric"] = json!("ask_rate");
    observed["quality_gate"] = json!("");
    for (goal, kinds) in [
        (talky_goal(), json!(["calls", "gap", "tool_error"])),
        (own_gate, json!(["calls", "tool_error"])),
        (observed, json!(["calls", "ask"])),
    ] {
        assert_eq!(
            call(METER, "stats_kinds", json!([goal])),
            kinds,
            "`calls` is the denominator, the gate rides along, nothing twice: {goal}"
        );
    }
}

#[test]
fn a_stats_ask_names_its_window_kinds_and_tag_in_the_hop() {
    if !can_run() {
        return;
    }
    assert_eq!(
        call(METER, "rfc3339", json!([0])),
        json!("1970-01-01T00:00:00Z")
    );
    let m = call(
        METER,
        "stats_ask",
        json!([talky_goal(), 60, "wait:x", {"cycle_id": "cycle:1", "goal": "goal:talky-gap_rate"}]),
    );
    let h = &m["header"];
    assert_eq!(
        h["route"], "stats",
        "the ask leaves on the hive's stats lane: {m}"
    );
    assert_eq!(
        m["messages"],
        json!([]),
        "an empty turn list, the question is in the hop: {m}"
    );
    assert_eq!(h["stats_role"], "talky", "{h}");
    assert_eq!(
        h["stats_tag"], "wait:x",
        "the tag is the wait row's id: {h}"
    );
    assert_eq!(
        parsed(&h["stats_kinds"]),
        json!(["calls", "gap", "tool_error"]),
        "{h}"
    );
    assert_eq!(h["stats_samples"], json!(10), "a number, not a text: {h}");
    let at = |key: &str| {
        chrono::DateTime::parse_from_rfc3339(h[key].as_str().expect("a text"))
            .unwrap_or_else(|e| panic!("{key} is not RFC 3339 ({e}): {h}"))
            .timestamp()
    };
    assert_eq!(
        at("stats_to") - at("stats_from"),
        60 * 60 + 1,
        "the window, and `to` one second past now because it is exclusive: {h}"
    );
    assert_eq!(h["cycle_id"], "cycle:1", "{h}");
    assert_eq!(h["goal"], "goal:talky-gap_rate", "{h}");
    for key in ["rules", "goal_spec", "hints", "measured", "require_plan"] {
        assert_eq!(
            h[key], "",
            "the full hop form, empty where it means nothing: {key} {h}"
        );
    }
    assert_the_declaration_admits(METER, &[m]);
}

#[test]
fn a_rate_is_a_count_per_hundred_calls_or_none() {
    if !can_run() {
        return;
    }
    for (counts, metric, gate, rate, gate_rate) in [
        (
            json!({"calls": 40, "gap": 10, "tool_error": 2}),
            "gap_rate",
            "tool_error_rate",
            json!(25.0),
            json!(5.0),
        ),
        (
            json!({"calls": 3, "gap": 1}),
            "gap_rate",
            "tool_error_rate",
            json!(33.3333),
            Value::Null,
        ),
        (
            json!({"calls": 0, "gap": 0, "tool_error": 0}),
            "gap_rate",
            "tool_error_rate",
            Value::Null,
            Value::Null,
        ),
        (
            json!({"calls": 5}),
            "gap_rate",
            "",
            Value::Null,
            Value::Null,
        ),
    ] {
        let r = call(METER, "rates", json!([{"counts": counts}, metric, gate]));
        assert_eq!(
            r["rate"], rate,
            "no calls or no count of that kind is None, never zero: {counts} -> {r}"
        );
        assert_eq!(r["gate_rate"], gate_rate, "{counts} -> {r}");
        assert_eq!(r["calls"], counts["calls"], "{r}");
    }
}

#[test]
fn a_stats_answer_that_is_not_a_measurement_says_which_it_was() {
    if !can_run() {
        return;
    }
    for (given, unavailable) in [
        (answer(json!({}), "invalid_input"), "stats_invalid_input"),
        (answer(json!({}), "store_error"), "stats_store_error"),
        (answer(stats(40, 10, 2, true), ""), "stats_truncated"),
        (answer(json!("nope"), ""), "stats_no_counts"),
        (
            answer(json!({"from": "a", "to": "b"}), ""),
            "stats_no_counts",
        ),
    ] {
        let m = call(
            METER,
            "measure_from_stats",
            json!([given, talky_goal(), 60]),
        );
        assert_eq!(
            m["unavailable"], unavailable,
            "a refusal, a part of a window and no counts are not a zero: {given} -> {m}"
        );
        assert!(
            m.get("rates").is_none(),
            "no rate out of no measurement: {m}"
        );
    }
}

#[test]
fn a_stats_answer_measures_calls_rates_and_samples() {
    if !can_run() {
        return;
    }
    let m = call(
        METER,
        "measure_from_stats",
        json!([answer(stats(40, 10, 2, false), ""), talky_goal(), 1440]),
    );
    assert!(m.get("unavailable").is_none(), "{m}");
    assert_eq!(m["source"], "stats", "{m}");
    assert_eq!(m["role"], "talky", "{m}");
    assert_eq!(m["window_minutes"], 1440, "{m}");
    assert_eq!(m["calls"], 40, "{m}");
    assert_eq!(
        m["samples"], 40,
        "`min_samples` is held against the calls of the window: {m}"
    );
    assert_eq!(
        m["rates"],
        json!({"gap_rate": 25.0, "tool_error_rate": 5.0}),
        "{m}"
    );
    assert_eq!(
        m["sampled"],
        json!([{"at": "2026-09-29T10:00:00Z", "kind": "gap", "session_id": "s1",
                "turn_id": "t1", "value": "searched"}]),
        "the samples are metadata only: {m}"
    );
    assert_eq!(call(METER, "value_of", json!([m, "gap_rate"])), json!(25.0));
    assert_eq!(
        call(
            METER,
            "value_of",
            json!([{"rates": {"gap_rate": null}}, "gap_rate"])
        ),
        Value::Null,
        "a rate that could not be computed stays unknown"
    );
}

#[test]
fn hints_reach_the_judge_as_hypotheses_up_to_the_limit() {
    if !can_run() {
        return;
    }
    let rows = json!([
        {"id": "hint:1", "at": "a", "origin": "probe-origin", "confidence": 70, "line": "look at x"},
        {"id": "", "origin": "nameless"},
        "not a row",
        {"id": "hint:2", "at": "b", "origin": "probe-origin", "confidence": 20, "line": "y"},
        {"id": "hint:3", "at": "c", "origin": "probe-origin", "confidence": 1, "line": "z"}
    ]);
    let got = call(METER, "judge_hints", json!([rows, 2]));
    assert_eq!(
        got[1],
        json!(["hint:1", "hint:2"]),
        "the first two valid rows: {got}"
    );
    let block = &got[0];
    assert!(
        block["note"]
            .as_str()
            .is_some_and(|n| n.starts_with("hypotheses, not measurements")),
        "the label travels inside the block: {block}"
    );
    assert_eq!(
        block["hints"][0],
        json!({"id": "hint:1", "origin": "probe-origin", "confidence": 70, "line": "look at x"}),
        "{block}"
    );
    for (rows, limit) in [(json!([]), 5), (rows, 0)] {
        assert_eq!(
            call(METER, "judge_hints", json!([rows, limit])),
            json!([null, []]),
            "nothing to show is no block at all, not an empty one"
        );
    }
}

#[test]
fn the_judge_order_carries_rules_goal_and_the_hints_it_was_shown() {
    if !can_run() {
        return;
    }
    let measured = call(
        METER,
        "measure_from_stats",
        json!([answer(stats(40, 10, 2, false), ""), talky_goal(), 1440]),
    );
    let hints = json!([{"id": "hint:1", "origin": "probe-origin", "confidence": 5, "line": "l"}]);
    let picked = call(METER, "judge_hints", json!([hints, 5]));
    let m = call(
        METER,
        "judge_order",
        json!([
            "cycle:1",
            talky_goal(),
            rules(),
            measured,
            picked[0],
            picked[1]
        ]),
    );
    let h = &m["header"];
    assert_eq!(h["route"], "judge", "{m}");
    assert_eq!(h["cycle_id"], "cycle:1", "{h}");
    assert_eq!(h["goal"], "goal:talky-gap_rate", "{h}");
    assert_eq!(
        parsed(&h["rules"]),
        rules(),
        "the charter rows reach the mutator: {h}"
    );
    assert_eq!(
        parsed(&h["goal_spec"]),
        talky_goal(),
        "and the goal row: {h}"
    );
    assert_eq!(
        parsed(&h["hints"]),
        json!(["hint:1"]),
        "and which hints were shown: {h}"
    );
    assert_eq!(parsed(&h["measured"]), measured, "{h}");
    assert_eq!(h["require_plan"], "1", "{h}");

    let t = judge_text(&m);
    assert_eq!(
        t["hypotheses"], picked[0],
        "the block, as it was built: {t}"
    );
    assert_eq!(t["role"], "talky", "{t}");
    assert_eq!(t["rates"], measured["rates"], "{t}");
    assert_eq!(t["calls"], 40, "{t}");
    assert_eq!(t["samples"], measured["sampled"], "{t}");
    assert_eq!(t["radius"], "model,numeric_params,text_slots", "{t}");
    // The rules the charter does not carry here fall back fail-closed.
    assert_eq!(t["eval_min_batch"], "30", "{t}");
    assert_eq!(t["slot_classes"], "", "{t}");
    assert_eq!(t["owner_only"], "persona,behaviour", "{t}");
    assert_eq!(t["never"], "identity", "{t}");
    assert!(
        !t["measured"].to_string().contains("hint:1"),
        "a hint never becomes part of the measurement: {t}"
    );
    assert_the_declaration_admits(METER, &[m]);
}

#[test]
fn the_effect_ruling_holds_a_role_goal_to_its_quality_gate() {
    if !can_run() {
        return;
    }
    let loose = json!([{"id": "rule:quality-floor", "kind": "quality_floor_pct", "value": "30"}]);
    let at = |gap: f64, tool_error: f64| json!({"rates": {"gap_rate": gap, "tool_error_rate": tool_error}});
    for (rule_rows, before, after, keep, reason) in [
        (
            rules(),
            at(20.0, 5.0),
            at(10.0, 5.0),
            true,
            "improved_50.0_pct",
        ),
        (
            rules(),
            at(20.0, 5.0),
            at(19.0, 5.0),
            false,
            "not_proven_5.0_pct",
        ),
        (
            rules(),
            at(20.0, 5.0),
            at(10.0, 6.0),
            false,
            "quality_gate_tool_error_rate_20.0_pct",
        ),
        (
            rules(),
            at(20.0, 0.0),
            at(10.0, 1.0),
            false,
            "quality_gate_tool_error_rate_100.0_pct",
        ),
        (
            loose,
            at(20.0, 5.0),
            at(10.0, 6.0),
            true,
            "improved_50.0_pct",
        ),
        (
            rules(),
            json!({"rates": {"gap_rate": null}}),
            at(10.0, 5.0),
            false,
            "unmeasurable",
        ),
    ] {
        let got = call(
            METER,
            "effect_ruling",
            json!([talky_goal(), rule_rows, before, after]),
        );
        assert_eq!(got[0], json!(keep), "{got}");
        assert_eq!(got[1], reason, "{got}");
        assert_eq!(
            got[2]["reason"], reason,
            "the effect names the same reason: {got}"
        );
        assert_eq!(got[2]["gate"], "tool_error_rate", "{got}");
    }
    // A colony goal is ruled exactly as before: no gate on its receipt.
    let got = call(
        METER,
        "effect_ruling",
        json!([colony_goal(), [], {"cost_eur": 0.02}, {"cost_eur": 0.009}]),
    );
    assert_eq!(got[0], json!(true), "{got}");
    assert_eq!(got[1], "improved_55.0_pct", "{got}");
    assert!(got[2].get("gate").is_none(), "{got}");
}

#[test]
fn the_hint_limit_is_read_as_a_knob() {
    if !can_run() {
        return;
    }
    let cfg = config(METER);
    assert_eq!(cfg["params"]["hint_limit"], 5);
    assert_eq!(cfg["contract"]["settings"]["hint_limit"]["default"], 5);
    assert!(
        script(METER).contains("HINT_LIMIT = _int(\"hint_limit\", 5)"),
        "the script reads the knob with the shipped default beside it"
    );
}

#[test]
fn the_contract_declares_the_stats_lane_and_the_judge_keys() {
    if !can_run() {
        return;
    }
    let cfg = config(METER);
    let hop = &cfg["contract"]["emits"]["hop"];
    assert!(
        hop["route"]["values"]
            .as_array()
            .is_some_and(|v| v.contains(&json!("stats"))),
        "{hop}"
    );
    for key in ["rules", "goal_spec", "hints"] {
        assert_eq!(
            hop[key],
            json!({"type": "string", "required": true}),
            "{key}"
        );
    }
    for key in [
        "stats_role",
        "stats_tag",
        "stats_from",
        "stats_to",
        "stats_kinds",
    ] {
        assert_eq!(
            hop[key],
            json!({"type": "string", "required": false}),
            "{key}"
        );
    }
    assert_eq!(
        hop["stats_samples"],
        json!({"type": "number", "required": false})
    );
    assert_eq!(
        cfg["contract"]["consumes"]["body"]["stats"],
        json!({"type": "object", "required": false})
    );
}

// ─────────────────────────────────────────────────────────────────── flows

#[test]
fn an_in_stats_answer_reads_back_its_open_ask_by_tag() {
    if !can_run() {
        return;
    }
    let out = run(
        METER,
        json!({
            "messages": [],
            "stats": stats(40, 10, 2, false),
            "header": {"hop": {"route": "in_stats", "stats_tag": "wait:s1",
                               "error_code": "", "detail": ""},
                       "context": {"argus_origin": "meter", "ar_phase": "", "ar_carry": ""}}
        }),
    );
    assert_eq!(out.len(), 1, "one lookup, nothing else: {out:?}");
    let args = store_call(&out, "select", "waits").expect("the memory is read back");
    assert_eq!(
        args["where"],
        json!({"id": "wait:s1", "status": "open", "kind": "stats"}),
        "only an OPEN stats ask can be resumed -- a late answer finds nothing: {args}"
    );
    assert_eq!(out[0]["header"]["phase"], "wait");
    assert_eq!(
        parsed(&out[0]["header"]["carry"]),
        answer(stats(40, 10, 2, false), ""),
        "the answer travels in the carry"
    );
    assert_the_declaration_admits(METER, &out);

    let out = run(
        METER,
        json!({
            "messages": [],
            "header": {"hop": {"route": "in_stats", "stats_tag": "wait:s1",
                               "error_code": "invalid_input", "detail": "stats_from"},
                       "context": {}}
        }),
    );
    let carry = parsed(&out[0]["header"]["carry"]);
    assert_eq!(
        carry["stats"],
        json!({}),
        "a refusal carries no stats: {carry}"
    );
    assert_eq!(carry["error_code"], "invalid_input", "{carry}");
}

#[test]
fn an_in_stats_answer_without_a_tag_is_dropped() {
    if !can_run() {
        return;
    }
    let out = run(
        METER,
        json!({"stats": stats(1, 1, 1, false),
               "header": {"hop": {"route": "in_stats"}, "context": {}}}),
    );
    assert!(
        out.is_empty(),
        "no tag names no ask; guessing would be worse: {out:?}"
    );
}

#[test]
fn a_tick_selects_the_source_of_every_goal() {
    if !can_run() {
        return;
    }
    // The charter is read once the tick's deadline has run (review finding I1).
    let out = run(METER, store_answer(json!([]), "stale", json!({})));
    let args = store_call(&out, "select", "goals").expect("the charter is read");
    assert!(
        args["columns"]
            .as_array()
            .is_some_and(|c| c.contains(&json!("source"))),
        "where a goal is measured is charter data: {args}"
    );
}

#[test]
fn a_tick_asks_for_overdue_asks_before_the_charter() {
    if !can_run() {
        return;
    }
    let out = run(
        METER,
        json!({"messages": [], "header": {"hop": {"schedule_name": "argus-cycle"}, "context": {}}}),
    );
    assert_eq!(out.len(), 1, "{out:?}");
    let args = store_call(&out, "select", "waits").expect("the overdue asks are read first");
    assert_eq!(
        args["where"],
        json!({"status": "open", "kind": {"in": ["stats", "eval"]}}),
        "{args}"
    );
    assert_eq!(out[0]["header"]["route"], "rstore");
    assert_eq!(out[0]["header"]["phase"], "stale");
    assert!(
        store_call(&out, "select", "goals").is_none()
            && store_call(&out, "select", "cycles").is_none(),
        "the charter and the applied question wait for the deadline: {out:?}"
    );
    assert_eq!(parsed(&out[0]["header"]["carry"]), json!({}));
    assert_the_declaration_admits(METER, &out);
}

#[test]
fn the_next_tick_closes_every_overdue_ask_with_its_receipt() {
    if !can_run() {
        return;
    }
    let rows = json!([
        {"id": "wait:b", "at": "1", "kind": "stats", "status": "open",
         "carry": {"goal": talky_goal(), "cycle_id": "cycle:b", "ask": "baseline",
                   "rules": [], "prices": {}}},
        {"id": "wait:e", "at": "2", "kind": "stats", "status": "open",
         "carry": {"goal": talky_goal(), "cycle_id": "cycle:e", "ask": "effect",
                   "rules": [], "prices": {}, "row": {"id": "cycle:e"}}},
        {"id": "wait:v", "at": "3", "kind": "eval", "status": "open",
         "carry": {"cycle_id": "cycle:v", "batch_min": 30}}
    ]);
    let carry = json!({"goals": [talky_goal()], "rules": rules()});
    let out = run(METER, store_answer(rows, "stale", carry.clone()));

    let inserts: Vec<Value> = store_calls(&out, "insert", "cycles")
        .into_iter()
        .map(|a| a["row"].clone())
        .collect();
    assert_eq!(
        inserts.len(),
        2,
        "one receipt per unanswered stats ask: {out:?}"
    );
    let baseline = inserts
        .iter()
        .find(|r| r["id"] == "cycle:b")
        .unwrap_or_else(|| panic!("a baseline closes the cycle it opened: {inserts:?}"));
    assert_eq!(baseline["outcome"], "skipped", "{baseline}");
    assert_eq!(baseline["reason_code"], "stats_unanswered", "{baseline}");
    assert_eq!(baseline["status"], "closed", "{baseline}");
    assert_eq!(baseline["role"], "talky", "{baseline}");
    let effect = inserts
        .iter()
        .find(|r| r["id"] != "cycle:b")
        .expect("the effect ask's receipt");
    assert_ne!(
        effect["id"], "cycle:e",
        "the applied cycle is not closed by a missing answer: {effect}"
    );
    assert_eq!(effect["reason_code"], "stats_unanswered", "{effect}");

    let updates = store_calls(&out, "update", "cycles");
    assert_eq!(updates.len(), 1, "{out:?}");
    assert_eq!(
        updates[0]["where"],
        json!({"id": "cycle:v", "status": "open"}),
        "{updates:?}"
    );
    assert_eq!(
        updates[0]["set"],
        json!({"status": "closed", "outcome": "discarded", "reason_code": "eval_unanswered"}),
        "a text change without its batch is thrown away: {updates:?}"
    );

    let mut closed: Vec<String> = store_calls(&out, "update", "waits")
        .iter()
        .map(|c| c["where"]["id"].as_str().unwrap_or_default().to_string())
        .collect();
    closed.sort();
    assert_eq!(
        closed,
        ["wait:b", "wait:e", "wait:v"],
        "every overdue ask is closed"
    );

    let last = out.last().expect("an emission");
    let args = parsed(&last["messages"][0]["text"]);
    assert_eq!(
        args["table"], "goals",
        "the charter question comes last: {args}"
    );
    assert_eq!(args["where"], json!({"enabled": 1}), "{args}");
    assert_eq!(last["header"]["route"], "cstore");
    assert_eq!(last["header"]["phase"], "goals");
    assert_eq!(parsed(&last["header"]["carry"]), carry);
    assert!(
        store_call(&out, "select", "cycles").is_none(),
        "the applied question waits for the charter: {out:?}"
    );
    assert_the_declaration_admits(METER, &out);

    let out = run(METER, store_answer(json!([]), "stale", carry));
    assert_eq!(
        out.len(),
        1,
        "nothing overdue, only the charter question: {out:?}"
    );
    assert!(store_call(&out, "select", "goals").is_some(), "{out:?}");
}

#[test]
fn a_role_goal_asks_the_stats_lane_for_its_baseline() {
    if !can_run() {
        return;
    }
    let out = run(
        METER,
        store_answer(
            json!([]),
            "open",
            json!({"goals": [talky_goal()], "rules": rules()}),
        ),
    );
    let row = store_call(&out, "insert", "waits").expect("the ask leaves a memory")["row"].clone();
    assert_eq!(row["kind"], "stats", "{row}");
    assert_eq!(row["status"], "open", "{row}");
    assert_eq!(row["carry"]["ask"], "baseline", "{row}");
    assert_eq!(row["carry"]["goal"]["id"], "goal:talky-gap_rate", "{row}");

    let asks = on_route(&out, "stats");
    assert_eq!(asks.len(), 1, "{out:?}");
    let h = &asks[0]["header"];
    assert_eq!(
        h["stats_tag"], row["id"],
        "the tag IS the wait row's id: {h}"
    );
    assert_eq!(h["stats_role"], "talky", "{h}");
    assert_eq!(
        parsed(&h["stats_kinds"]),
        json!(["calls", "gap", "tool_error"])
    );
    assert_eq!(h["cycle_id"], row["carry"]["cycle_id"], "{h}");
    assert!(
        on_route(&out, "ledger").is_empty(),
        "a role goal is not measured at the colony"
    );
    assert_the_declaration_admits(METER, &out);

    let out = run(
        METER,
        store_answer(
            json!([]),
            "open",
            json!({"goals": [colony_goal()], "rules": []}),
        ),
    );
    assert_eq!(
        on_route(&out, "ledger").len(),
        1,
        "a colony goal still asks the ledger"
    );
    assert!(on_route(&out, "stats").is_empty(), "{out:?}");
}

#[test]
fn an_applied_role_cycle_is_measured_by_the_stats_lane() {
    if !can_run() {
        return;
    }
    let out = run(
        METER,
        store_answer(
            json!([applied_row(20.0, 5.0)]),
            "open",
            json!({"goals": [talky_goal()], "rules": rules()}),
        ),
    );
    let row = store_call(&out, "insert", "waits").expect("a memory")["row"].clone();
    assert_eq!(row["kind"], "stats", "{row}");
    assert_eq!(row["carry"]["ask"], "effect", "{row}");
    assert_eq!(row["carry"]["row"]["id"], "cycle:7", "{row}");
    let asks = on_route(&out, "stats");
    assert_eq!(asks.len(), 1, "{out:?}");
    assert_eq!(asks[0]["header"]["cycle_id"], "cycle:7");
    assert_the_declaration_admits(METER, &out);
}

#[test]
fn a_truncated_stats_answer_is_skipped_and_never_judged() {
    if !can_run() {
        return;
    }
    let out = resume(
        "stats",
        wait_carry("baseline", talky_goal(), Value::Null),
        answer(stats(40, 10, 2, true), ""),
        None,
    );
    let row = inserted_cycle(&out);
    assert_eq!(row["outcome"], "skipped", "{row}");
    assert_eq!(row["reason_code"], "stats_truncated", "{row}");
    assert_eq!(
        row["id"], "cycle:7",
        "the baseline closes its own cycle: {row}"
    );
    assert_eq!(row["role"], "talky", "{row}");
    assert!(
        on_route(&out, "judge").is_empty() && store_call(&out, "select", "hints").is_none(),
        "a part of a window never reaches the judge: {out:?}"
    );
    let closed = store_call(&out, "update", "waits").expect("the wait is closed");
    assert_eq!(closed["where"], json!({"id": "wait:s1"}));
    assert_the_declaration_admits(METER, &out);
}

#[test]
fn a_refused_stats_ask_is_skipped_with_its_error_code() {
    if !can_run() {
        return;
    }
    let out = resume(
        "stats",
        wait_carry("baseline", talky_goal(), Value::Null),
        answer(json!({}), "invalid_input"),
        None,
    );
    let row = inserted_cycle(&out);
    assert_eq!(row["reason_code"], "stats_invalid_input", "{row}");
    assert_eq!(row["outcome"], "skipped", "{row}");
    assert!(on_route(&out, "judge").is_empty(), "{out:?}");

    let out = resume(
        "stats",
        wait_carry("effect", talky_goal(), applied_row(20.0, 5.0)),
        answer(json!({}), "store_error"),
        None,
    );
    let row = inserted_cycle(&out);
    assert_eq!(row["reason_code"], "stats_store_error", "{row}");
    assert_ne!(
        row["id"], "cycle:7",
        "the applied cycle stays applied: {row}"
    );
    assert!(
        store_call(&out, "update", "cycles").is_none(),
        "nothing is ruled on a refusal: {out:?}"
    );
    assert_the_declaration_admits(METER, &out);
}

#[test]
fn a_role_baseline_below_min_samples_counts_calls() {
    if !can_run() {
        return;
    }
    let out = resume(
        "stats",
        wait_carry("baseline", talky_goal(), Value::Null),
        answer(stats(12, 3, 0, false), ""),
        None,
    );
    let row = inserted_cycle(&out);
    assert_eq!(row["reason_code"], "below_min_samples_12", "{row}");
    assert_eq!(row["role"], "talky", "{row}");
    assert!(store_call(&out, "select", "hints").is_none(), "{out:?}");
}

#[test]
fn an_observed_role_rate_is_recorded_without_a_model() {
    if !can_run() {
        return;
    }
    let goal = json!({"id": "goal:cogny-ask_rate", "metric": "ask_rate", "source": "cogny",
                      "direction": "observe", "window_minutes": 1440, "min_samples": 30,
                      "min_delta_pct": 10, "quality_gate": "", "enabled": 1});
    let counts = json!({"from": "a", "to": "b", "counts": {"calls": 40, "ask": 5},
                        "samples": [], "truncated": false});
    let out = resume(
        "stats",
        wait_carry("baseline", goal, Value::Null),
        answer(counts, ""),
        None,
    );
    let row = inserted_cycle(&out);
    assert_eq!(row["outcome"], "observed", "{row}");
    assert_eq!(row["reason_code"], "observed_ask_rate_12.5", "{row}");
    assert_eq!(row["role"], "cogny", "{row}");
    assert!(
        on_route(&out, "alert").is_empty(),
        "a rate is normally above zero, so it is no alarm: {out:?}"
    );
    assert!(
        on_route(&out, "judge").is_empty() && store_call(&out, "select", "hints").is_none(),
        "an observe goal never reaches a model: {out:?}"
    );
    assert_the_declaration_admits(METER, &out);
}

#[test]
fn a_measured_role_baseline_reads_the_open_hints_first() {
    if !can_run() {
        return;
    }
    let out = resume(
        "stats",
        wait_carry("baseline", talky_goal(), Value::Null),
        answer(stats(40, 10, 2, false), ""),
        None,
    );
    let args = store_call(&out, "select", "hints").expect("the open hints are read");
    assert_eq!(
        args["where"],
        json!({"consumed_by": {"or_null": {"eq": ""}}}),
        "{args}"
    );
    assert_eq!(args["limit"], 5, "{args}");
    assert_eq!(
        args["order_by"],
        json!([{"col": "at", "dir": "asc"}]),
        "{args}"
    );
    assert_eq!(
        args["columns"],
        json!(["id", "at", "origin", "confidence", "line"]),
        "{args}"
    );
    let ask = out
        .iter()
        .find(|m| m["header"]["phase"] == "hints")
        .expect("the hints read resumes in phase hints");
    let carry = parsed(&ask["header"]["carry"]);
    assert_eq!(carry["cycle_id"], "cycle:7", "{carry}");
    assert_eq!(carry["goal"], talky_goal(), "{carry}");
    assert_eq!(carry["rules"], rules(), "{carry}");
    assert_eq!(carry["measured"]["rates"]["gap_rate"], 25.0, "{carry}");
    assert!(
        on_route(&out, "judge").is_empty(),
        "the judge waits for the hints: {out:?}"
    );
    let closed = store_call(&out, "update", "waits").expect("the answered ask is closed");
    assert_eq!(closed["where"], json!({"id": "wait:s1"}));
    assert_the_declaration_admits(METER, &out);
}

#[test]
fn a_hint_limit_of_zero_asks_the_judge_at_once() {
    if !can_run() {
        return;
    }
    let mut params = shipped_params(METER);
    params["hint_limit"] = json!(0);
    let out = resume(
        "stats",
        wait_carry("baseline", talky_goal(), Value::Null),
        answer(stats(40, 10, 2, false), ""),
        Some(params.clone()),
    );
    assert!(store_call(&out, "select", "hints").is_none(), "{out:?}");
    let judge = on_route(&out, "judge");
    assert_eq!(judge.len(), 1, "{out:?}");
    assert!(judge_text(judge[0]).get("hypotheses").is_none());
    assert_eq!(
        call_with_params(METER, &params, "judge_hints", json!([[{"id": "h"}], 0])),
        json!([null, []])
    );
}

#[test]
fn the_hints_phase_hands_the_judge_the_block_and_marks_it_consumed() {
    if !can_run() {
        return;
    }
    let measured = call(
        METER,
        "measure_from_stats",
        json!([answer(stats(40, 10, 2, false), ""), talky_goal(), 1440]),
    );
    let carry = json!({"cycle_id": "cycle:7", "goal": talky_goal(), "rules": rules(),
                       "measured": measured});
    let rows = json!([
        {"id": "hint:1", "at": "a", "origin": "probe-origin", "confidence": 70, "line": "look at x"},
        {"id": "hint:2", "at": "b", "origin": "probe-origin", "confidence": 20, "line": "y"}
    ]);
    let out = run(METER, store_answer(rows, "hints", carry));
    let judge = on_route(&out, "judge");
    assert_eq!(judge.len(), 1, "{out:?}");
    let t = judge_text(judge[0]);
    assert_eq!(
        t["hypotheses"]["hints"][1]["id"], "hint:2",
        "both hints, in the block of their own: {t}"
    );
    assert_eq!(
        t["measured"], measured,
        "the measurement is untouched by them: {t}"
    );
    let h = &judge[0]["header"];
    assert_eq!(parsed(&h["hints"]), json!(["hint:1", "hint:2"]), "{h}");
    assert_eq!(parsed(&h["rules"]), rules(), "{h}");
    assert_eq!(parsed(&h["goal_spec"])["id"], "goal:talky-gap_rate", "{h}");

    let mark = store_call(&out, "update", "hints").expect("the shown hints are consumed");
    assert_eq!(mark["set"], json!({"consumed_by": "cycle:7"}), "{mark}");
    assert_eq!(
        mark["where"],
        json!({"id": {"in": ["hint:1", "hint:2"]}, "consumed_by": {"or_null": {"eq": ""}}}),
        "exactly the hints shown, and only while still open: {mark}"
    );
    assert_the_declaration_admits(METER, &out);
}

#[test]
fn without_open_hints_the_judge_order_has_no_hypotheses() {
    if !can_run() {
        return;
    }
    let carry = json!({"cycle_id": "cycle:7", "goal": colony_goal(), "rules": [],
                       "measured": {"cost_eur": 0.1, "samples": 40}});
    let out = run(METER, store_answer(json!([]), "hints", carry));
    assert_eq!(out.len(), 1, "the judge order and no hints write: {out:?}");
    let t = judge_text(&out[0]);
    assert!(
        t.get("hypotheses").is_none(),
        "no block rather than an empty one: {t}"
    );
    assert!(t.get("role").is_none(), "a colony goal names no role: {t}");
    assert_eq!(parsed(&out[0]["header"]["hints"]), json!([]));
    assert_the_declaration_admits(METER, &out);
}

#[test]
fn a_role_effect_that_proves_itself_is_kept() {
    if !can_run() {
        return;
    }
    let out = resume(
        "stats",
        wait_carry("effect", talky_goal(), applied_row(50.0, 5.0)),
        answer(stats(40, 10, 2, false), ""),
        None,
    );
    let update = store_call(&out, "update", "cycles").expect("the applied cycle is ruled on");
    assert_eq!(update["where"], json!({"id": "cycle:7"}), "{update}");
    assert_eq!(update["set"]["outcome"], "kept", "{update}");
    assert_eq!(
        update["set"]["reason_code"], "improved_50.0_pct",
        "{update}"
    );
    assert!(
        update["set"].get("role").is_none(),
        "the row carries its role since its insert: {update}"
    );
    assert_eq!(update["set"]["effect"]["gate_after"], 5.0, "{update}");
    assert!(on_route(&out, "revert").is_empty(), "{out:?}");
    let closed = store_call(&out, "update", "waits").expect("the wait is closed");
    assert_eq!(closed["where"], json!({"id": "wait:s1"}));
    assert_the_declaration_admits(METER, &out);
}

#[test]
fn a_role_effect_that_costs_quality_is_reverted_with_the_rules() {
    if !can_run() {
        return;
    }
    let out = resume(
        "stats",
        wait_carry("effect", talky_goal(), applied_row(50.0, 1.0)),
        answer(stats(40, 10, 2, false), ""),
        None,
    );
    let update = store_call(&out, "update", "cycles").expect("the applied cycle is ruled on");
    assert_eq!(update["set"]["outcome"], "reverted", "{update}");
    assert_eq!(
        update["set"]["reason_code"], "quality_gate_tool_error_rate_400.0_pct",
        "a better metric bought with a worse gate is no improvement: {update}"
    );
    let order = on_route(&out, "revert");
    assert_eq!(order.len(), 1, "{out:?}");
    let args = parsed(&order[0]["messages"][0]["text"]);
    assert_eq!(args["op"], "revert", "{args}");
    assert_eq!(
        args["plan"]["to"], "a",
        "the way back is the stored one: {args}"
    );
    assert_eq!(
        args["rules"],
        rules(),
        "the mutator gets the charter rows: {args}"
    );
    assert_the_declaration_admits(METER, &out);
}

#[test]
fn a_colony_baseline_also_reads_the_open_hints_first() {
    if !can_run() {
        return;
    }
    let ledger = json!({
        "query": {"since": 1, "until": 2, "group_by": "model", "tag": "wait:s1"},
        "messages": {"total": 40, "errors": 0,
                     "by_model": {"m": {"calls": 40, "tokens_prompt": 100,
                                        "tokens_completion": 100}}},
        "dead_letters": {"total": 0}, "scan_truncated": false
    });
    let carry = json!({"goal": colony_goal(), "prices": {}, "rules": [], "cycle_id": "cycle:1"});
    let out = resume("baseline", carry, ledger, None);
    assert!(
        store_call(&out, "select", "hints").is_some(),
        "every judged cycle sees the open hints: {out:?}"
    );
    assert!(on_route(&out, "judge").is_empty(), "{out:?}");
    assert_the_declaration_admits(METER, &out);
}

#[test]
fn a_ledger_refusal_only_ever_closes_a_ledger_ask() {
    if !can_run() {
        return;
    }
    let out = run(
        METER,
        json!({"ledger": {"status": "error", "error_code": "invalid_query", "details": "x"},
               "header": {"hop": {}, "context": {}}}),
    );
    let args = store_call(&out, "select", "waits").expect("a refusal is still an answer");
    assert_eq!(
        args["where"],
        json!({"status": "open", "kind": {"in": ["baseline", "effect"]}}),
        "a refusal must never pick a stats or eval ask off the same table: {args}"
    );
}

// ───────────────────────────────────────────────── review findings (fix round)

/// The emission that carries this store call, beside the call's arguments.
fn emission_of(out: &[Value], operation: &str, table: &str) -> (Value, Value) {
    out.iter()
        .find_map(|m| {
            let args: Value = serde_json::from_str(m["messages"][0]["text"].as_str()?).ok()?;
            (args["operation"] == operation && args["table"] == table).then(|| (m.clone(), args))
        })
        .unwrap_or_else(|| panic!("no {operation} on {table}: {out:?}"))
}

/// The store answer to `m`, resumed in the phase and with the carry `m` named.
fn answer_to(m: &Value, rows: Value) -> Value {
    let phase = m["header"]["phase"].as_str().expect("a phase");
    store_answer(rows, phase, parsed(&m["header"]["carry"]))
}

/// One unanswered `stats` baseline ask and one unanswered `eval` order.
fn overdue_rows() -> Value {
    json!([
        {"id": "wait:b", "at": "1", "kind": "stats", "status": "open",
         "carry": {"goal": talky_goal(), "cycle_id": "cycle:b", "ask": "baseline",
                   "rules": [], "prices": {}}},
        {"id": "wait:v", "at": "2", "kind": "eval", "status": "open",
         "carry": {"cycle_id": "cycle:v", "batch_min": 30, "ar_goal_spec": talky_goal()}}
    ])
}

/// Review finding I1: the deadline runs in every tick, an idle one included.
/// Behind the charter it ran only while a goal was enabled, so switching every
/// goal off left a baseline cycle without a receipt and a text change whose
/// late verdict would still be applied.
#[test]
fn an_idle_tick_closes_every_overdue_ask_before_its_idle_receipt() {
    if !can_run() {
        return;
    }
    let out = run(
        METER,
        json!({"messages": [], "header": {"hop": {"schedule_name": "argus-cycle"}, "context": {}}}),
    );
    assert_eq!(out.len(), 1, "{out:?}");
    let (ask, args) = emission_of(&out, "select", "waits");
    assert_eq!(ask["header"]["route"], "rstore", "{ask}");
    assert_eq!(
        args["where"],
        json!({"status": "open", "kind": {"in": ["stats", "eval"]}}),
        "{args}"
    );
    assert_the_declaration_admits(METER, &out);

    let out = run(METER, answer_to(&ask, overdue_rows()));
    let baseline = store_calls(&out, "insert", "cycles")
        .into_iter()
        .map(|a| a["row"].clone())
        .find(|r| r["id"] == "cycle:b")
        .unwrap_or_else(|| panic!("the baseline closes the cycle it opened: {out:?}"));
    assert_eq!(baseline["outcome"], "skipped", "{baseline}");
    assert_eq!(baseline["reason_code"], "stats_unanswered", "{baseline}");
    let discard = store_call(&out, "update", "cycles").expect("the text change is discarded");
    assert_eq!(discard["set"]["outcome"], "discarded", "{discard}");
    assert_eq!(
        discard["set"]["reason_code"], "eval_unanswered",
        "{discard}"
    );
    let mut closed: Vec<String> = store_calls(&out, "update", "waits")
        .iter()
        .map(|c| c["where"]["id"].as_str().unwrap_or_default().to_string())
        .collect();
    closed.sort();
    assert_eq!(closed, ["wait:b", "wait:v"], "both asks are closed");
    let (charter, args) = emission_of(&out, "select", "goals");
    assert_eq!(charter["header"]["route"], "cstore", "{charter}");
    assert_eq!(args["where"], json!({"enabled": 1}), "{args}");
    assert!(
        store_call(&out, "select", "cycles").is_none(),
        "no applied question before the charter: {out:?}"
    );
    assert_the_declaration_admits(METER, &out);

    let out = run(METER, answer_to(&charter, json!([])));
    assert_eq!(out.len(), 1, "the idle receipt and nothing else: {out:?}");
    let idle = inserted_cycle(&out);
    assert_eq!(idle["outcome"], "idle", "{idle}");
    assert_eq!(idle["reason_code"], "no_enabled_goal", "{idle}");
    assert_the_declaration_admits(METER, &out);
}

/// Review finding I1, the active half: the order moved the deadline to the
/// front and added no hop -- the rules answer asks for the applied cycle.
#[test]
fn an_active_tick_asks_the_rules_then_the_applied_cycle() {
    if !can_run() {
        return;
    }
    let out = run(
        METER,
        store_answer(json!([talky_goal()]), "goals", json!({})),
    );
    let (ask, _) = emission_of(&out, "select", "rules");
    assert_eq!(ask["header"]["phase"], "rules", "{ask}");
    let out = run(METER, answer_to(&ask, rules()));
    assert_eq!(out.len(), 1, "{out:?}");
    let (applied, args) = emission_of(&out, "select", "cycles");
    assert_eq!(args["where"], json!({"status": "applied"}), "{args}");
    assert_eq!(applied["header"]["phase"], "open", "{applied}");
    let carry = parsed(&applied["header"]["carry"]);
    assert_eq!(carry["goals"], json!([talky_goal()]), "{carry}");
    assert_eq!(carry["rules"], rules(), "{carry}");
    assert!(
        store_call(&out, "select", "waits").is_none(),
        "the deadline already ran at the tick: {out:?}"
    );
    assert_the_declaration_admits(METER, &out);
}

/// Review findings M3 and M9: the discard is a compare-and-set on `open`, so
/// it never lands on a row the mutator has applied meanwhile, and it sets no
/// `role` -- the row has carried it since its insert.
#[test]
fn an_unanswered_eval_only_closes_a_cycle_that_is_still_open() {
    if !can_run() {
        return;
    }
    let out = run(METER, store_answer(overdue_rows(), "stale", json!({})));
    let discard = store_call(&out, "update", "cycles").expect("the discard");
    assert_eq!(
        discard["where"],
        json!({"id": "cycle:v", "status": "open"}),
        "an applied row stays applied: {discard}"
    );
    assert_eq!(
        discard["set"],
        json!({"status": "closed", "outcome": "discarded", "reason_code": "eval_unanswered"}),
        "no role on a later update: {discard}"
    );
}

/// Review finding M9: the effect ruling fills in only what the receipts
/// store names as filled in later.
#[test]
fn an_effect_ruling_writes_no_role() {
    if !can_run() {
        return;
    }
    let out = resume(
        "stats",
        wait_carry("effect", talky_goal(), applied_row(50.0, 5.0)),
        answer(stats(40, 10, 2, false), ""),
        None,
    );
    let update = store_call(&out, "update", "cycles").expect("the ruling");
    let mut keys: Vec<String> = update["set"]
        .as_object()
        .expect("a set object")
        .keys()
        .cloned()
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        ["effect", "outcome", "reason_code", "status"],
        "{update}"
    );
}

/// Review finding M1: an echo naming an `eval` row must not resume -- and
/// close -- it as an unmeasured stats ask.
#[test]
fn an_in_stats_answer_only_resumes_a_stats_ask() {
    if !can_run() {
        return;
    }
    let out = run(
        METER,
        json!({
            "messages": [],
            "stats": stats(40, 10, 2, false),
            "header": {"hop": {"route": "in_stats", "stats_tag": "wait:v",
                               "error_code": "", "detail": ""},
                       "context": {}}
        }),
    );
    let args = store_call(&out, "select", "waits").expect("the lookup");
    assert_eq!(
        args["where"],
        json!({"id": "wait:v", "status": "open", "kind": "stats"}),
        "{args}"
    );
}

/// Review finding M5: a quality gate that was not counted on both sides, or
/// names no rate a role counts, is no evidence that quality held -- the change
/// is reverted, never kept.
#[test]
fn an_unknown_quality_gate_is_never_kept() {
    if !can_run() {
        return;
    }
    let rates = |gap: f64, tool_error: Value| json!({"rates": {"gap_rate": gap, "tool_error_rate": tool_error}});
    for (before, after) in [
        (rates(20.0, Value::Null), rates(10.0, json!(5.0))),
        (rates(20.0, json!(5.0)), rates(10.0, Value::Null)),
        (
            json!({"rates": {"gap_rate": 20.0}}),
            json!({"rates": {"gap_rate": 10.0}}),
        ),
    ] {
        let got = call(
            METER,
            "effect_ruling",
            json!([talky_goal(), rules(), before, after]),
        );
        assert_eq!(got[0], json!(false), "{got}");
        assert_eq!(got[1], "quality_gate_tool_error_rate_unknown", "{got}");
        assert_eq!(got[2]["reason"], got[1], "{got}");
    }
    let mut foreign_gate = talky_goal();
    foreign_gate["quality_gate"] = json!("answer_quality");
    let got = call(
        METER,
        "effect_ruling",
        json!([
            foreign_gate,
            rules(),
            rates(20.0, json!(5.0)),
            rates(10.0, json!(5.0))
        ]),
    );
    assert_eq!(got[0], json!(false), "{got}");
    assert_eq!(got[1], "quality_gate_answer_quality_unknown", "{got}");
    // A change that proved nothing keeps the reason it has.
    let got = call(
        METER,
        "effect_ruling",
        json!([
            talky_goal(),
            rules(),
            rates(20.0, Value::Null),
            rates(19.0, Value::Null)
        ]),
    );
    assert_eq!(got[1], "not_proven_5.0_pct", "{got}");

    // The flow: the gate's count is missing from the effect window.
    let counts = json!({"from": "a", "to": "b", "counts": {"calls": 40, "gap": 10},
                        "samples": [], "truncated": false});
    let out = resume(
        "stats",
        wait_carry("effect", talky_goal(), applied_row(50.0, 5.0)),
        answer(counts, ""),
        None,
    );
    let update = store_call(&out, "update", "cycles").expect("the ruling");
    assert_eq!(update["set"]["outcome"], "reverted", "{update}");
    assert_eq!(
        update["set"]["reason_code"], "quality_gate_tool_error_rate_unknown",
        "{update}"
    );
    assert_eq!(on_route(&out, "revert").len(), 1, "{out:?}");
    assert_the_declaration_admits(METER, &out);
}

/// Review finding M6: a role's rate is written whole -- `%d` cut 0.5 to 0 --
/// and, being normally above zero, it raises no alert.
#[test]
fn an_observed_role_rate_is_written_whole_and_never_alerted() {
    if !can_run() {
        return;
    }
    let goal = json!({"id": "goal:cogny-ask_rate", "metric": "ask_rate", "source": "cogny",
                      "direction": "observe", "window_minutes": 1440, "min_samples": 30,
                      "min_delta_pct": 10, "quality_gate": "", "enabled": 1});
    for (calls, asks, reason) in [
        (40, 5, "observed_ask_rate_12.5"),
        (200, 1, "observed_ask_rate_0.5"),
        (50, 6, "observed_ask_rate_12"),
        (30, 10, "observed_ask_rate_33.33"),
        (40, 0, "observed_ask_rate_0"),
    ] {
        let counts = json!({"from": "a", "to": "b", "counts": {"calls": calls, "ask": asks},
                            "samples": [], "truncated": false});
        let out = resume(
            "stats",
            wait_carry("baseline", goal.clone(), Value::Null),
            answer(counts, ""),
            None,
        );
        let row = inserted_cycle(&out);
        assert_eq!(row["outcome"], "observed", "{row}");
        assert_eq!(
            row["reason_code"], reason,
            "{calls} calls, {asks} asks: {row}"
        );
        assert!(on_route(&out, "alert").is_empty(), "{out:?}");
        assert!(on_route(&out, "judge").is_empty(), "{out:?}");
        assert_the_declaration_admits(METER, &out);
    }
    let counts = json!({"from": "a", "to": "b", "counts": {"calls": 40},
                        "samples": [], "truncated": false});
    let out = resume(
        "stats",
        wait_carry("baseline", goal, Value::Null),
        answer(counts, ""),
        None,
    );
    assert_eq!(
        inserted_cycle(&out)["reason_code"],
        "observed_ask_rate_unknown"
    );
    assert!(on_route(&out, "alert").is_empty(), "{out:?}");
}

/// Review finding M6, the other half: a colony symptom keeps its count and
/// its alert exactly as before.
#[test]
fn a_colony_symptom_keeps_its_count_and_its_alert() {
    if !can_run() {
        return;
    }
    let goal = json!({"id": "goal:dlq-watch", "metric": "dlq_rate", "source": "colony",
                      "direction": "observe", "window_minutes": 60, "min_samples": 0,
                      "min_delta_pct": 0, "quality_gate": "", "enabled": 1});
    let carry = json!({"goal": goal, "prices": {}, "rules": [], "cycle_id": "cycle:1"});
    let ledger = |dead: u64| {
        json!({"query": {"since": 1, "until": 2, "group_by": "model", "tag": "wait:s1"},
               "messages": {"total": 4, "errors": 0, "by_model": {}},
               "dead_letters": {"total": dead}, "scan_truncated": false})
    };
    let out = resume("baseline", carry.clone(), ledger(3), None);
    assert_eq!(inserted_cycle(&out)["reason_code"], "observed_dlq_rate_3");
    let alerts = on_route(&out, "alert");
    assert_eq!(alerts.len(), 1, "{out:?}");
    assert_eq!(
        parsed(&alerts[0]["messages"][0]["text"])["value"],
        json!(3.0)
    );
    let out = resume("baseline", carry, ledger(0), None);
    assert_eq!(
        inserted_cycle(&out)["reason_code"],
        "observed_dlq_rate_clean"
    );
    assert!(on_route(&out, "alert").is_empty(), "{out:?}");
}

/// Review finding M2: the answering side refuses a span over 31 days, so a
/// longer charter window asks for the most recent 31 days.
#[test]
fn a_window_past_31_days_is_capped_at_31_days() {
    if !can_run() {
        return;
    }
    let span = |window_minutes: i64| {
        let m = call(
            METER,
            "stats_ask",
            json!([talky_goal(), window_minutes, "wait:x", {}]),
        );
        let at = |key: &str| {
            chrono::DateTime::parse_from_rfc3339(m["header"][key].as_str().expect("a text"))
                .unwrap_or_else(|e| panic!("{key} is not RFC 3339 ({e}): {m}"))
                .timestamp()
        };
        at("stats_to") - at("stats_from")
    };
    let day = 24 * 60 * 60;
    for (window, seconds) in [
        (40 * 24 * 60, 31 * day),
        (31 * 24 * 60, 31 * day),
        (30 * 24 * 60, 30 * day + 1),
        (24 * 60, day + 1),
    ] {
        assert_eq!(span(window), seconds, "a window of {window} minutes");
    }
}

/// Review finding M4: the rate a goal is about could not be computed -- its
/// metric is no role rate, or nobody counted its mark -- so the cycle is
/// skipped and no judge is asked. Unknown, never zero.
#[test]
fn a_metric_nobody_counted_is_skipped_and_never_judged() {
    if !can_run() {
        return;
    }
    let counts = json!({"from": "a", "to": "b", "counts": {"calls": 40, "tool_error": 2},
                        "samples": [], "truncated": false});
    let out = resume(
        "stats",
        wait_carry("baseline", talky_goal(), Value::Null),
        answer(counts, ""),
        None,
    );
    let row = inserted_cycle(&out);
    assert_eq!(row["outcome"], "skipped", "{row}");
    assert_eq!(row["reason_code"], "stats_missing_gap_rate", "{row}");
    assert_eq!(
        row["id"], "cycle:7",
        "the baseline closes its own cycle: {row}"
    );
    assert!(
        on_route(&out, "judge").is_empty() && store_call(&out, "select", "hints").is_none(),
        "{out:?}"
    );
    let closed = store_call(&out, "update", "waits").expect("the wait is closed");
    assert_eq!(closed["where"], json!({"id": "wait:s1"}));
    assert_the_declaration_admits(METER, &out);

    let mut foreign = talky_goal();
    foreign["metric"] = json!("answer_quality");
    let out = resume(
        "stats",
        wait_carry("baseline", foreign, Value::Null),
        answer(stats(40, 10, 2, false), ""),
        None,
    );
    assert_eq!(
        inserted_cycle(&out)["reason_code"],
        "stats_missing_answer_quality"
    );
    assert!(
        on_route(&out, "judge").is_empty() && store_call(&out, "select", "hints").is_none(),
        "{out:?}"
    );
}
