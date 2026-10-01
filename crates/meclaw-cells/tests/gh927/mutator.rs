//! GH #927 — the mutator's text-slot half, table by table.
//!
//! A text change is the one change this loop may not make on the judge's word:
//! it is sorted first (identity never, the owner's classes only as a proposal,
//! then the radius and the class list), re-checked with a plan that has to
//! restore the original byte for byte, sent to an evaluator, and applied only
//! when a batch at least `max(eval_min_batch, min_samples)` large passes and
//! moves the goal's metric past its floor. Every step of that is a pure
//! function of the shipped script, called here one by one through [`call`];
//! the flows around them are whole runs through [`run`], judged by the cell's
//! own declared `emits` contract.
//!
//! The model and numeric half is pinned by `gh155_argus_loop.rs`. What is
//! asked of it here is only that it did NOT move: a model change still leaves
//! as a params update, now with the three new columns on its row.

use super::harness::*;
use meclaw_core::serde_json::{self, Value, json};

/// The cell a text change is addressed to — another part of the colony.
const TARGET: &str = "/main/assistant/brain";
/// The slot's text before the change, and the text the judge proposes.
const OLD: &str = "Look the answer up before you guess.";
const NEW: &str = "Look the answer up first, and say so when nothing was found.";

/// The shipped charter's rules, as the meter hands them over.
fn rules() -> Value {
    json!([
        {"id": "rule:radius", "kind": "radius", "value": "model,numeric_params,text_slots"},
        {"id": "rule:eval-min-batch", "kind": "eval_min_batch", "value": "30"},
        {"id": "rule:slot-classes", "kind": "slot_classes",
         "value": "tool_description,prompt_block,curator_knob"},
        {"id": "rule:owner-only", "kind": "owner_only", "value": "persona,behaviour"},
        {"id": "rule:never", "kind": "never", "value": "identity"}
    ])
}

/// The same rules with text slots taken out of the radius.
fn rules_without_text_slots() -> Value {
    let mut r = rules();
    r[0]["value"] = json!("model,numeric_params");
    r
}

/// A goal whose source is a role.
fn goal() -> Value {
    json!({"id": "goal:assistant-gap_rate", "metric": "gap_rate", "source": "assistant",
           "direction": "lower", "window_minutes": 1440, "min_samples": 30,
           "min_delta_pct": 10, "quality_gate": "tool_error_rate", "enabled": 1})
}

fn with(mut v: Value, key: &str, value: Value) -> Value {
    v[key] = value;
    v
}

fn without(mut v: Value, key: &str) -> Value {
    if let Some(o) = v.as_object_mut() {
        o.remove(key);
    }
    v
}

fn text_change(class: &str) -> Value {
    json!({"target": TARGET, "kind": "text_slot", "class": class, "key": "lookup",
           "from": OLD, "to": NEW})
}

fn text_plan() -> Value {
    json!({"target": TARGET, "kind": "text_slot", "key": "lookup", "to": OLD})
}

/// The judge's context for one cycle, the way the edge `./meter -> ./judge`
/// lifts it: every value a JSON text.
fn context_with(rules: &Value) -> Value {
    json!({
        "ar_cycle": "cycle:1",
        "ar_goal": "goal:assistant-gap_rate",
        "ar_measured": json!({"gap_rate": 12.5, "calls": 80}).to_string(),
        "ar_require_plan": "1",
        "ar_rules": rules.to_string(),
        "ar_goal_spec": goal().to_string(),
        "ar_hints": json!(["hint:1"]).to_string()
    })
}

/// The judge's answer in the shape the `llm` cell really emits — the
/// provider's function object, arguments one level in (GH #462).
fn judge_answer(args: Value, context: Value) -> Value {
    json!({
        "messages": [{
            "origin": "assistant", "type": "tool_call", "id": "c1",
            "text": json!({"name": "argus_change", "arguments": args.to_string()}).to_string()
        }],
        "header": {"hop": {}, "context": context}
    })
}

fn slot_decision(change: Value, plan: Value) -> Value {
    json!({"cycle_id": "cycle:1", "action": "change",
           "reasoning": "the gap rate says the description misleads the brain",
           "simulated": {}, "change": change, "revert_plan": plan})
}

/// An evaluator's verdict body.
fn evidence(n: i64, verdict: &str, delta_pct: Value) -> Value {
    json!({"verdict": verdict, "batch_id": "batch:1", "n": n,
           "baseline": 12.0, "candidate": 9.0, "delta_pct": delta_pct})
}

/// The `waits` row the decide path wrote for the order, as `./receipts`
/// answers it.
fn wait_row(batch_min: i64) -> Value {
    json!({"id": "wait:abc", "at": "2026-01-01T00:00:00.000000Z", "kind": "eval",
           "status": "open",
           "carry": {"cycle_id": "cycle:1", "change": text_change("tool_description"),
                     "revert_plan": text_plan(), "ar_goal_spec": goal(),
                     "ar_rules": rules(), "batch_min": batch_min}})
}

/// A verdict resumed: the wait row read back, the verdict in the carry.
fn resumed(rows: Value, ev: &Value) -> Value {
    store_answer(rows, "evalwait", json!({"eval": ev}))
}

/// A revert order. The meter's carries the rules; the probe's does not. Both
/// arrive on the lane `revert`, the only lane the mutator takes a way back
/// from (review finding C1).
fn revert_order(plan: Value, rules: Option<Value>) -> Value {
    let mut args = json!({"op": "revert", "cycle_id": "cycle:1", "plan": plan});
    if let Some(r) = rules {
        args["rules"] = r;
    }
    json!({
        "messages": [{"origin": "assistant", "type": "tool_call", "id": "c1",
                      "text": args.to_string()}],
        "header": {"hop": {"route": "revert"}, "context": {}}
    })
}

/// The judge's answer with its arguments flat in the turn's text — the form a
/// `code` cell writes, and one a model can write as well.
fn judge_answer_flat(args: Value, context: Value) -> Value {
    json!({
        "messages": [{
            "origin": "assistant", "type": "tool_call", "id": "c1",
            "text": args.to_string()
        }],
        "header": {"hop": {}, "context": context}
    })
}

/// A revert order in the judge's words: the id of another cycle, a plan of
/// its own.
fn judges_revert(plan: Value) -> Value {
    json!({"op": "revert", "cycle_id": "cycle:0", "plan": plan})
}

/// A message on the verdict lane, the way the hive's door delivers one:
/// `hop.route` `in_eval`, the tag the evaluator echoed, a fresh context.
fn on_the_verdict_lane(body: Value, eval_tag: &str) -> Value {
    let mut m = body;
    if m.get("messages").is_none() {
        m["messages"] = json!([]);
    }
    m["header"] = json!({
        "hop": {"route": "in_eval", "eval_tag": eval_tag},
        "context": {"argus_origin": "mutator", "ar_phase": "", "ar_carry": ""}
    });
    m
}

/// The one receipt a refused answer leaves, with nothing beside it.
fn the_only_emission_is_a_refusal(out: &[Value], reason: &str) -> Value {
    assert_the_declaration_admits(MUTATOR, out);
    every_hop_carries_carry(out);
    assert_eq!(
        out.len(),
        1,
        "{reason}: the receipt and nothing else: {out:?}"
    );
    for lane in ["slot_update", "mutate", "probe", "eval"] {
        assert!(
            on_route(out, lane).is_empty(),
            "{reason}: nothing leaves on `{lane}`: {out:?}"
        );
    }
    let row = store_call(out, "insert", "cycles").expect("the refusal is a receipt")["row"].clone();
    assert_eq!(row["status"], "closed", "{row}");
    assert_eq!(row["outcome"], "refused", "{row}");
    assert_eq!(row["reason_code"], reason, "{row}");
    row
}

fn routes(out: &[Value]) -> Vec<&str> {
    out.iter()
        .map(|m| m["header"]["route"].as_str().unwrap_or(""))
        .collect()
}

/// The tool-call arguments of one emission (a store call, a probe order).
fn args_of(m: &Value) -> Value {
    serde_json::from_str(m["messages"][0]["text"].as_str().expect("a tool call"))
        .expect("tool-call arguments are JSON")
}

/// The edge `./mutator -> ./receipts` lifts `hop.carry`, and an edge whose
/// modifier reads a missing key is skipped — so every emission carries one.
fn every_hop_carries_carry(out: &[Value]) {
    for m in out {
        assert!(
            m["header"]["carry"].is_string(),
            "an emission without `hop.carry` loses its edge to the store: {m}"
        );
    }
}

// ---------------------------------------------------------------------------
// The pure functions
// ---------------------------------------------------------------------------

#[test]
fn a_text_class_is_sorted_identity_first() {
    if !can_run() {
        return;
    }
    let table = [
        (
            "identity",
            rules(),
            json!(["refused", "identity_out_of_radius"]),
        ),
        (
            "persona",
            rules(),
            json!(["proposed", "owner_approval_required"]),
        ),
        (
            "behaviour",
            rules(),
            json!(["proposed", "owner_approval_required"]),
        ),
        (
            "tool_description",
            rules_without_text_slots(),
            json!(["refused", "outside_radius"]),
        ),
        ("", rules(), json!(["refused", "slot_class_missing"])),
        (
            "greeting",
            rules(),
            json!(["refused", "key_outside_radius_greeting"]),
        ),
        ("tool_description", rules(), json!(["eval", ""])),
        ("prompt_block", rules(), json!(["eval", ""])),
        // The order is the rule: identity and the owner's classes are asked
        // before the radius, so no radius edit reaches either.
        (
            "identity",
            rules_without_text_slots(),
            json!(["refused", "identity_out_of_radius"]),
        ),
        (
            "persona",
            rules_without_text_slots(),
            json!(["proposed", "owner_approval_required"]),
        ),
        // No rules at all is the closed charter, not an open one.
        (
            "identity",
            json!([]),
            json!(["refused", "identity_out_of_radius"]),
        ),
        (
            "persona",
            json!([]),
            json!(["proposed", "owner_approval_required"]),
        ),
        (
            "tool_description",
            json!([]),
            json!(["refused", "outside_radius"]),
        ),
    ];
    for (class, rules, want) in table {
        let got = call(MUTATOR, "classify_slot", json!([text_change(class), rules]));
        assert_eq!(got, want, "class {class:?} under {rules}");
    }
}

#[test]
fn a_rule_row_that_is_missing_or_blank_is_closed() {
    if !can_run() {
        return;
    }
    assert_eq!(
        call(MUTATOR, "rule", json!([rules(), "eval_min_batch", "7"])),
        json!("30")
    );
    assert_eq!(
        call(MUTATOR, "rule", json!([[], "eval_min_batch", "7"])),
        json!("7")
    );
    assert_eq!(
        call(
            MUTATOR,
            "rule",
            json!([[{"kind": "never", "value": "  "}], "never", "identity"])
        ),
        json!("identity"),
        "an emptied row is not configured, and not configured is closed"
    );
    assert_eq!(
        call(
            MUTATOR,
            "rule_set",
            json!([[{"kind": "never", "value": " identity , ,secrets "}], "never", ""])
        ),
        json!(["identity", "secrets"])
    );
    assert_eq!(
        call(
            MUTATOR,
            "rule_set",
            json!([[], "owner_only", "persona,behaviour"])
        ),
        json!(["persona", "behaviour"])
    );
}

#[test]
fn a_text_revert_plan_restores_the_original_byte_for_byte() {
    if !can_run() {
        return;
    }
    let change = text_change("tool_description");
    let plan = text_plan();
    let table = [
        (change.clone(), plan.clone(), ""),
        (
            with(change.clone(), "target", json!("main/assistant/brain")),
            plan.clone(),
            "target_not_absolute",
        ),
        (
            with(change.clone(), "key", json!("")),
            plan.clone(),
            "slot_key_missing",
        ),
        (
            with(change.clone(), "to", json!("")),
            plan.clone(),
            "no_new_value",
        ),
        (
            with(change.clone(), "to", json!(7)),
            plan.clone(),
            "no_new_value",
        ),
        (change.clone(), json!({}), "no_revert_plan"),
        (
            change.clone(),
            with(plan.clone(), "target", json!("/main/other/brain")),
            "revert_plan_wrong_target",
        ),
        (
            change.clone(),
            with(plan.clone(), "kind", json!("model")),
            "revert_plan_wrong_kind",
        ),
        (
            change.clone(),
            with(plan.clone(), "key", json!("another_slot")),
            "revert_plan_wrong_key",
        ),
        (
            change.clone(),
            with(plan.clone(), "to", json!("")),
            "revert_plan_has_no_value",
        ),
        (
            change.clone(),
            with(plan.clone(), "to", json!(NEW)),
            "revert_plan_is_not_inverse",
        ),
        (
            change.clone(),
            with(plan.clone(), "to", json!("a third text")),
            "revert_plan_does_not_restore_the_original",
        ),
        // Byte for byte: a trailing space is another text.
        (
            with(change.clone(), "from", json!("from ")),
            with(plan.clone(), "to", json!("from")),
            "revert_plan_does_not_restore_the_original",
        ),
        // No `str()` on the way: a number is not the sentence "5".
        (
            with(change.clone(), "from", json!(5)),
            with(plan.clone(), "to", json!("5")),
            "revert_plan_does_not_restore_the_original",
        ),
        // A change with no `from` has no original to restore.
        (
            without(change.clone(), "from"),
            plan.clone(),
            "revert_plan_does_not_restore_the_original",
        ),
    ];
    for (change, plan, want) in table {
        let got = call(MUTATOR, "revert_ok_text", json!([change, plan, true]));
        assert_eq!(got, json!(want), "{change} / {plan}");
    }
    // A charter that requires no plan asks the change half only.
    assert_eq!(
        call(MUTATOR, "revert_ok_text", json!([change, {}, false])),
        json!("")
    );
    assert_eq!(
        call(
            MUTATOR,
            "revert_ok_text",
            json!([with(change.clone(), "to", json!("")), {}, false])
        ),
        json!("no_new_value")
    );
}

#[test]
fn the_batch_floor_is_the_larger_of_the_charter_and_the_goal() {
    if !can_run() {
        return;
    }
    let table = [
        (rules(), goal(), 30),
        (rules(), with(goal(), "min_samples", json!(45)), 45),
        (
            json!([{"kind": "eval_min_batch", "value": "50"}]),
            with(goal(), "min_samples", json!(45)),
            50,
        ),
        (json!([]), json!({}), 30),
        (
            json!([{"kind": "eval_min_batch", "value": " "}]),
            json!({}),
            30,
        ),
    ];
    for (rules, goal, want) in table {
        let got = call(MUTATOR, "batch_min_of", json!([rules, goal]));
        assert_eq!(got, json!(want), "{rules} / {goal}");
    }
}

#[test]
fn evidence_licenses_a_change_only_on_a_large_passing_batch_in_the_goals_direction() {
    if !can_run() {
        return;
    }
    let apply = json!(["apply", ""]);
    let below = json!(["discarded", "below_delta"]);
    let higher = with(goal(), "direction", json!("higher"));
    let table = [
        (
            evidence(10, "pass", json!(-25)),
            goal(),
            30,
            json!(["discarded", "no_evidence_10"]),
        ),
        // The batch size is asked before the verdict: a verdict on too small a
        // batch is noise whichever way it points.
        (
            evidence(10, "fail", json!(-25)),
            goal(),
            30,
            json!(["discarded", "no_evidence_10"]),
        ),
        (
            evidence(40, "fail", json!(-25)),
            goal(),
            30,
            json!(["discarded", "eval_failed"]),
        ),
        (evidence(40, "pass", json!(-25)), goal(), 30, apply.clone()),
        (evidence(40, "pass", json!(-10)), goal(), 30, apply.clone()),
        (evidence(40, "pass", json!(-5)), goal(), 30, below.clone()),
        (evidence(40, "pass", json!(25)), goal(), 30, below.clone()),
        (
            evidence(40, "pass", json!(25)),
            higher.clone(),
            30,
            apply.clone(),
        ),
        (evidence(40, "pass", json!(-25)), higher, 30, below.clone()),
        (
            evidence(40, "pass", json!("n/a")),
            goal(),
            30,
            below.clone(),
        ),
        (
            evidence(40, "pass", json!(-25)),
            with(goal(), "direction", json!("observe")),
            30,
            below,
        ),
        (
            evidence(40, "pass", json!(-25)),
            goal(),
            45,
            json!(["discarded", "no_evidence_40"]),
        ),
        // A count that arrives as text is read like a delta that does
        // (review finding M7).
        (
            with(evidence(40, "pass", json!(-25)), "n", json!("40")),
            goal(),
            30,
            apply.clone(),
        ),
        (json!({}), goal(), 30, json!(["discarded", "no_evidence_0"])),
    ];
    for (ev, goal, batch_min, want) in table {
        let got = call(MUTATOR, "evidence_ok", json!([ev, goal, batch_min]));
        assert_eq!(got, want, "{ev} / {goal} / batch {batch_min}");
    }
    // The JAZ threshold is exactly the larger of the two: a goal that asks for
    // 45 samples turns a batch of 40 away although the charter's floor is 30.
    let strict = with(goal(), "min_samples", json!(45));
    let floor = call(MUTATOR, "batch_min_of", json!([rules(), strict]));
    assert_eq!(floor, json!(45));
    assert_eq!(
        call(
            MUTATOR,
            "evidence_ok",
            json!([evidence(40, "pass", json!(-25)), strict, floor])
        ),
        json!(["discarded", "no_evidence_40"])
    );
}

// ---------------------------------------------------------------------------
// The decide path
// ---------------------------------------------------------------------------

#[test]
fn an_admissible_text_change_is_asked_about_rather_than_applied() {
    if !can_run() {
        return;
    }
    let out = run(
        MUTATOR,
        judge_answer(
            slot_decision(text_change("tool_description"), text_plan()),
            context_with(&rules()),
        ),
    );
    assert_the_declaration_admits(MUTATOR, &out);
    every_hop_carries_carry(&out);
    assert_eq!(routes(&out), ["rstore", "rstore", "eval"], "{out:?}");
    // The wait row goes out first, so it is in the store before any verdict
    // can ask for it.
    assert_eq!(args_of(&out[0])["table"], "waits", "{out:?}");
    assert_eq!(args_of(&out[1])["table"], "cycles", "{out:?}");

    let wait = store_call(&out, "insert", "waits").expect("a wait row")["row"].clone();
    let wid = wait["id"].as_str().expect("a wait id").to_string();
    assert!(wid.starts_with("wait:"), "{wait}");
    assert_eq!(wait["kind"], "eval", "{wait}");
    assert_eq!(wait["status"], "open", "{wait}");
    assert_eq!(
        wait["carry"],
        json!({"cycle_id": "cycle:1", "change": text_change("tool_description"),
               "revert_plan": text_plan(), "ar_goal_spec": goal(),
               "ar_rules": rules(), "batch_min": 30}),
        "the wait remembers everything the verdict's resumption needs"
    );

    let row = store_call(&out, "insert", "cycles").expect("a cycle row")["row"].clone();
    assert_eq!(row["status"], "open", "{row}");
    assert_eq!(row["outcome"], "eval_pending", "{row}");
    assert_eq!(row["reason_code"], "", "{row}");
    assert_eq!(row["role"], "assistant", "{row}");
    assert_eq!(row["hints"], json!(["hint:1"]), "{row}");
    assert_eq!(row["evidence"], json!({}), "{row}");

    let order = on_route(&out, "eval");
    assert_eq!(order.len(), 1, "{out:?}");
    let order = order[0];
    assert_eq!(order["header"]["eval_tag"], wid.as_str(), "{order}");
    assert_eq!(order["header"]["cycle_id"], "cycle:1", "{order}");
    assert_eq!(order["header"]["outcome"], "eval_pending", "{order}");
    assert_eq!(order["header"]["target"], TARGET, "{order}");
    assert_eq!(order["messages"], json!([]), "{order}");
    assert_eq!(
        order["eval"],
        json!({"cycle_id": "cycle:1", "target": TARGET, "key": "lookup",
               "from": OLD, "to": NEW, "batch_min": 30}),
        "{order}"
    );
    assert!(
        !order["header"].to_string().contains(NEW),
        "the texts travel in the body, never in the hop: {order}"
    );
    assert!(
        on_route(&out, "slot_update").is_empty(),
        "nothing is applied yet"
    );
}

#[test]
fn an_identity_change_leaves_nothing_but_its_receipt() {
    if !can_run() {
        return;
    }
    let out = run(
        MUTATOR,
        judge_answer(
            slot_decision(text_change("identity"), text_plan()),
            context_with(&rules()),
        ),
    );
    assert_the_declaration_admits(MUTATOR, &out);
    every_hop_carries_carry(&out);
    assert_eq!(out.len(), 1, "no order, no proposal, no text: {out:?}");
    let row =
        store_call(&out, "insert", "cycles").expect("the refusal is a receipt")["row"].clone();
    assert_eq!(row["status"], "closed", "{row}");
    assert_eq!(row["outcome"], "refused", "{row}");
    assert_eq!(row["reason_code"], "identity_out_of_radius", "{row}");
    assert_eq!(row["role"], "assistant", "{row}");
}

#[test]
fn a_persona_change_is_a_proposal_and_nothing_else() {
    if !can_run() {
        return;
    }
    let out = run(
        MUTATOR,
        judge_answer(
            slot_decision(text_change("persona"), text_plan()),
            context_with(&rules()),
        ),
    );
    assert_the_declaration_admits(MUTATOR, &out);
    every_hop_carries_carry(&out);
    assert_eq!(out.len(), 1, "no eval order and no slot update: {out:?}");
    let row =
        store_call(&out, "insert", "cycles").expect("the proposal is a receipt")["row"].clone();
    assert_eq!(row["status"], "closed", "{row}");
    assert_eq!(row["outcome"], "proposed", "{row}");
    assert_eq!(row["reason_code"], "owner_approval_required", "{row}");
}

#[test]
fn a_text_change_the_radius_or_its_plan_does_not_carry_is_refused_with_one_receipt() {
    if !can_run() {
        return;
    }
    let table = [
        // A plan that restores ALMOST the original.
        (
            text_change("tool_description"),
            with(text_plan(), "to", json!(format!("{OLD} "))),
            rules(),
            "revert_plan_does_not_restore_the_original",
        ),
        (
            text_change("greeting"),
            text_plan(),
            rules(),
            "key_outside_radius_greeting",
        ),
        (
            text_change("tool_description"),
            text_plan(),
            rules_without_text_slots(),
            "outside_radius",
        ),
        // A hop that brought no rules: the closed charter.
        (
            text_change("tool_description"),
            text_plan(),
            json!([]),
            "outside_radius",
        ),
    ];
    for (change, plan, rules, want) in table {
        let out = run(
            MUTATOR,
            judge_answer(slot_decision(change, plan), context_with(&rules)),
        );
        assert_the_declaration_admits(MUTATOR, &out);
        assert_eq!(out.len(), 1, "{want}: {out:?}");
        let row = store_call(&out, "insert", "cycles").expect("a receipt")["row"].clone();
        assert_eq!(row["outcome"], "refused", "{row}");
        assert_eq!(row["reason_code"], want, "{row}");
    }
}

// ---------------------------------------------------------------------------
// The verdict
// ---------------------------------------------------------------------------

#[test]
fn a_verdict_reads_its_wait_back_before_anything_happens() {
    if !can_run() {
        return;
    }
    let ev = evidence(40, "pass", json!(-25));
    let out = run(
        MUTATOR,
        json!({
            "messages": [], "eval": ev,
            "header": {"hop": {"route": "in_eval", "eval_tag": "wait:abc"},
                       "context": {"argus_origin": "mutator", "ar_phase": "", "ar_carry": ""}}
        }),
    );
    assert_the_declaration_admits(MUTATOR, &out);
    assert_eq!(out.len(), 1, "{out:?}");
    let m = &out[0];
    assert_eq!(m["header"]["route"], "rstore", "{m}");
    assert_eq!(m["header"]["phase"], "evalwait", "{m}");
    let carry: Value =
        serde_json::from_str(m["header"]["carry"].as_str().expect("carry is text")).expect("json");
    assert_eq!(carry, json!({"eval": ev}), "the verdict rides along: {m}");
    let select = store_call(&out, "select", "waits").expect("a read of the wait");
    assert_eq!(
        select["where"],
        json!({"id": "wait:abc", "status": "open"}),
        "{select}"
    );
    assert_eq!(select["limit"], 1, "{select}");
    assert_eq!(
        select["columns"],
        json!(["id", "at", "kind", "carry", "status"]),
        "{select}"
    );

    // A verdict that names no order is nobody's.
    let untagged = run(
        MUTATOR,
        json!({"messages": [], "eval": ev,
               "header": {"hop": {"route": "in_eval"}, "context": {}}}),
    );
    assert!(untagged.is_empty(), "{untagged:?}");
}

#[test]
fn a_passing_batch_applies_the_text_and_fires_the_probe() {
    if !can_run() {
        return;
    }
    let ev = evidence(40, "pass", json!(-25));
    let as_stored = wait_row(30);
    // The store may hand a `json` column back as an object or as its text.
    let as_text = with(wait_row(30), "carry", json!(as_stored["carry"].to_string()));
    for wait in [as_stored, as_text] {
        let out = run(MUTATOR, resumed(json!([wait]), &ev));
        assert_the_declaration_admits(MUTATOR, &out);
        every_hop_carries_carry(&out);
        assert_eq!(
            routes(&out),
            ["slot_update", "rstore", "probe", "rstore"],
            "{out:?}"
        );

        let update = on_route(&out, "slot_update")[0];
        assert_eq!(
            update["slot"],
            json!({"name": "lookup", "text": NEW}),
            "{update}"
        );
        assert_eq!(update["messages"], json!([]), "{update}");
        assert_eq!(update["header"]["target"], TARGET, "{update}");
        assert_eq!(
            update["header"]["cycle_id"], "cycle:1",
            "the probe finds the change at the target by the cycle's id: {update}"
        );
        assert_eq!(update["header"]["outcome"], "applied", "{update}");

        let cycle = store_call(&out, "update", "cycles").expect("the cycle is receipted");
        assert_eq!(
            without(cycle["set"].clone(), "at"),
            json!({"status": "applied", "outcome": "", "reason_code": "", "evidence": ev}),
            "{cycle}"
        );
        assert_eq!(cycle["where"], json!({"id": "cycle:1"}), "{cycle}");

        let probe = on_route(&out, "probe")[0];
        assert_eq!(
            args_of(probe),
            json!({"op": "probe", "cycle_id": "cycle:1", "target": TARGET}),
            "{probe}"
        );
        assert_eq!(probe["header"]["cycle_id"], "cycle:1", "{probe}");

        let done = store_call(&out, "update", "waits").expect("the wait is closed");
        assert_eq!(done["set"], json!({"status": "done"}), "{done}");
        assert_eq!(done["where"], json!({"id": "wait:abc"}), "{done}");
    }
}

#[test]
fn a_batch_that_does_not_license_the_change_is_discarded_with_its_reason() {
    if !can_run() {
        return;
    }
    let table = [
        (30, evidence(10, "pass", json!(-25)), "no_evidence_10"),
        (30, evidence(40, "fail", json!(-25)), "eval_failed"),
        (30, evidence(40, "pass", json!(-5)), "below_delta"),
        // The floor is the one the wait row remembers — an answer that names
        // a smaller one does not lower it.
        (
            45,
            with(evidence(40, "pass", json!(-25)), "batch_min", json!(10)),
            "no_evidence_40",
        ),
    ];
    for (batch_min, ev, want) in table {
        let out = run(MUTATOR, resumed(json!([wait_row(batch_min)]), &ev));
        assert_the_declaration_admits(MUTATOR, &out);
        every_hop_carries_carry(&out);
        assert_eq!(routes(&out), ["rstore", "rstore"], "{want}: {out:?}");
        let cycle = store_call(&out, "update", "cycles").expect("the cycle is receipted");
        assert_eq!(
            cycle["set"],
            json!({"status": "closed", "outcome": "discarded", "reason_code": want,
                   "evidence": ev}),
            "{cycle}"
        );
        let done = store_call(&out, "update", "waits").expect("the wait is closed");
        assert_eq!(done["set"], json!({"status": "done"}), "{done}");
    }
}

#[test]
fn a_verdict_whose_wait_is_gone_is_dropped_without_a_word() {
    if !can_run() {
        return;
    }
    let ev = evidence(40, "pass", json!(-25));
    let late = run(MUTATOR, resumed(json!([]), &ev));
    assert!(
        late.is_empty(),
        "the meter already closed this cycle: {late:?}"
    );
    let foreign = run(
        MUTATOR,
        resumed(json!([with(wait_row(30), "kind", json!("stats"))]), &ev),
    );
    assert!(
        foreign.is_empty(),
        "a wait of another kind is not this cell's: {foreign:?}"
    );
}

// ---------------------------------------------------------------------------
// The way back
// ---------------------------------------------------------------------------

#[test]
fn a_text_revert_outside_the_charters_radius_is_refused() {
    if !can_run() {
        return;
    }
    let out = run(
        MUTATOR,
        revert_order(text_plan(), Some(rules_without_text_slots())),
    );
    assert_the_declaration_admits(MUTATOR, &out);
    assert!(on_route(&out, "slot_update").is_empty(), "{out:?}");
    let cycle = store_call(&out, "update", "cycles").expect("the refusal is a receipt");
    assert_eq!(
        cycle["set"],
        json!({"status": "applied", "outcome": "revert_refused",
               "reason_code": "outside_radius"}),
        "the change is still standing, and the row says so: {cycle}"
    );
}

#[test]
fn a_text_revert_travels_the_slot_wire_back() {
    if !can_run() {
        return;
    }
    // The probe's revert carries no rules, the meter's carries the charter's;
    // `from` in a stored plan is dropped as for a number.
    for rules in [None, Some(rules())] {
        let out = run(
            MUTATOR,
            revert_order(with(text_plan(), "from", json!(NEW)), rules),
        );
        assert_the_declaration_admits(MUTATOR, &out);
        every_hop_carries_carry(&out);
        let back = on_route(&out, "slot_update");
        assert_eq!(back.len(), 1, "{out:?}");
        assert_eq!(
            back[0]["slot"],
            json!({"name": "lookup", "text": OLD}),
            "{}",
            back[0]
        );
        assert_eq!(back[0]["header"]["target"], TARGET, "{}", back[0]);
        assert_eq!(back[0]["header"]["outcome"], "reverted", "{}", back[0]);
        let cycle = store_call(&out, "update", "cycles").expect("a receipt");
        assert_eq!(
            cycle["set"],
            json!({"status": "closed", "outcome": "reverted"}),
            "{cycle}"
        );
    }
}

#[test]
fn a_degenerate_text_revert_plan_is_refused() {
    if !can_run() {
        return;
    }
    let table = [
        (
            with(text_plan(), "target", json!("assistant/brain")),
            "target_not_absolute",
        ),
        (with(text_plan(), "key", json!("")), "slot_key_missing"),
        (with(text_plan(), "to", json!("")), "no_new_value"),
        (with(text_plan(), "to", json!(4096)), "no_new_value"),
    ];
    for (plan, want) in table {
        let out = run(MUTATOR, revert_order(plan, None));
        assert_the_declaration_admits(MUTATOR, &out);
        assert!(on_route(&out, "slot_update").is_empty(), "{want}: {out:?}");
        let cycle = store_call(&out, "update", "cycles").expect("the refusal is a receipt");
        assert_eq!(cycle["set"]["outcome"], "revert_refused", "{cycle}");
        assert_eq!(cycle["set"]["reason_code"], want, "{cycle}");
        assert_eq!(cycle["set"]["status"], "applied", "{cycle}");
    }
}

// ---------------------------------------------------------------------------
// The half that did not move
// ---------------------------------------------------------------------------

#[test]
fn a_model_change_still_leaves_as_a_params_update_with_the_new_columns() {
    if !can_run() {
        return;
    }
    let decision = json!({
        "cycle_id": "cycle:1", "action": "change",
        "reasoning": "the smaller model carries this traffic at a third of the cost",
        "simulated": {"counterfactual_cost": 0.4},
        "change": {"target": TARGET, "kind": "model",
                   "from": "provider/model-large", "to": "provider/model-small"},
        "revert_plan": {"target": TARGET, "kind": "model", "to": "provider/model-large"}
    });
    let out = run(
        MUTATOR,
        judge_answer(decision.clone(), context_with(&rules())),
    );
    assert_the_declaration_admits(MUTATOR, &out);
    every_hop_carries_carry(&out);
    assert_eq!(
        routes(&out),
        ["mutate", "rstore", "probe"],
        "no evaluation for a model: {out:?}"
    );
    let change = on_route(&out, "mutate")[0];
    assert_eq!(
        change["params"],
        json!({"model": "provider/model-small"}),
        "{change}"
    );
    assert_eq!(change["system"], json!({}), "{change}");
    assert_eq!(change["header"]["target"], TARGET, "{change}");
    let row = store_call(&out, "insert", "cycles").expect("an applied row")["row"].clone();
    assert_eq!(row["status"], "applied", "{row}");
    assert_eq!(row["role"], "assistant", "{row}");
    assert_eq!(row["hints"], json!(["hint:1"]), "{row}");
    assert_eq!(row["evidence"], json!({}), "{row}");

    // A goal measured at the colony has no role, and a hop from a meter that
    // sends none of the new keys leaves the columns empty rather than missing.
    let mut colony = context_with(&rules());
    colony["ar_goal_spec"] = json!(with(goal(), "source", json!("colony")).to_string());
    let out = run(MUTATOR, judge_answer(decision.clone(), colony));
    let row = store_call(&out, "insert", "cycles").expect("a row")["row"].clone();
    assert_eq!(row["role"], "", "{row}");
    let bare = json!({"ar_cycle": "cycle:1", "ar_goal": "goal:llm-cost"});
    let out = run(MUTATOR, judge_answer(decision, bare));
    let row = store_call(&out, "insert", "cycles").expect("a row")["row"].clone();
    assert_eq!(row["role"], "", "{row}");
    assert_eq!(row["hints"], json!([]), "{row}");
    assert_eq!(row["evidence"], json!({}), "{row}");
}

#[test]
fn a_decision_to_do_nothing_carries_the_new_columns_too() {
    if !can_run() {
        return;
    }
    let out = run(
        MUTATOR,
        judge_answer(
            json!({"cycle_id": "cycle:1", "action": "none", "reasoning": "",
                   "simulated": {}, "change": {}, "revert_plan": {}}),
            context_with(&rules()),
        ),
    );
    assert_the_declaration_admits(MUTATOR, &out);
    let row = store_call(&out, "insert", "cycles").expect("a row")["row"].clone();
    assert_eq!(row["outcome"], "no_action", "{row}");
    assert_eq!(row["role"], "assistant", "{row}");
    assert_eq!(row["hints"], json!(["hint:1"]), "{row}");
    assert_eq!(row["evidence"], json!({}), "{row}");
}

// ---------------------------------------------------------------------------
// The lanes (review of GH #927, findings C1 and C2)
// ---------------------------------------------------------------------------

#[test]
fn a_revert_order_from_the_judge_writes_no_text() {
    if !can_run() {
        return;
    }
    let identity = json!({"target": TARGET, "kind": "text_slot", "key": "identity.name",
                          "to": "Somebody else"});
    let admissible = with(
        with(identity.clone(), "key", json!("lookup")),
        "class",
        json!("tool_description"),
    );
    for plan in [identity, admissible, text_plan()] {
        for answer in [
            judge_answer(judges_revert(plan.clone()), context_with(&rules())),
            judge_answer_flat(judges_revert(plan.clone()), context_with(&rules())),
        ] {
            let out = run(MUTATOR, answer);
            let row = the_only_emission_is_a_refusal(&out, "judge_cannot_revert");
            assert_eq!(
                row["id"], "cycle:1",
                "the row is this cycle's, never the one the order names: {row}"
            );
            assert_eq!(row["judged"]["action"], "revert", "{row}");
            assert!(!row.to_string().contains("Somebody else"), "{row}");
        }
    }
}

#[test]
fn a_revert_order_from_the_judge_swaps_no_model() {
    if !can_run() {
        return;
    }
    let plan = json!({"target": TARGET, "kind": "model", "to": "provider/model-other"});
    for answer in [
        judge_answer(judges_revert(plan.clone()), context_with(&rules())),
        judge_answer_flat(judges_revert(plan.clone()), context_with(&rules())),
    ] {
        let out = run(MUTATOR, answer);
        the_only_emission_is_a_refusal(&out, "judge_cannot_revert");
    }
    // An action beside the order does not make it a decision.
    let mut both = judges_revert(plan.clone());
    both["action"] = json!("change");
    both["change"] = json!({"target": TARGET, "kind": "model",
                            "from": "provider/model-other", "to": "provider/model-small"});
    both["revert_plan"] = plan;
    let out = run(MUTATOR, judge_answer(both, context_with(&rules())));
    the_only_emission_is_a_refusal(&out, "judge_cannot_revert");
}

#[test]
fn a_revert_order_on_its_lane_still_lands() {
    if !can_run() {
        return;
    }
    let out = run(
        MUTATOR,
        revert_order(
            json!({"target": TARGET, "kind": "model", "to": "provider/model-large"}),
            None,
        ),
    );
    assert_the_declaration_admits(MUTATOR, &out);
    let back = on_route(&out, "mutate");
    assert_eq!(back.len(), 1, "{out:?}");
    assert_eq!(
        back[0]["params"],
        json!({"model": "provider/model-large"}),
        "{}",
        back[0]
    );
    assert_eq!(back[0]["header"]["outcome"], "reverted", "{}", back[0]);
}

#[test]
fn the_revert_lane_carries_revert_orders_and_nothing_else() {
    if !can_run() {
        return;
    }
    let decisions = [
        slot_decision(text_change("tool_description"), text_plan()),
        json!({"cycle_id": "cycle:1", "action": "none", "reasoning": "",
               "simulated": {}, "change": {}, "revert_plan": {}}),
    ];
    for decision in decisions {
        let mut answer = judge_answer(decision, context_with(&rules()));
        answer["header"]["hop"] = json!({"route": "revert"});
        let out = run(MUTATOR, answer);
        assert!(
            out.is_empty(),
            "a decision on the revert lane is nobody's: {out:?}"
        );
    }
}

#[test]
fn the_verdict_lane_takes_a_tagged_verdict_and_nothing_else() {
    if !can_run() {
        return;
    }
    let identity = json!({"target": TARGET, "kind": "text_slot", "key": "identity.name",
                          "to": "Somebody else"});
    let model_decision = json!({
        "cycle_id": "cycle:1", "action": "change", "reasoning": "r", "simulated": {},
        "change": {"target": TARGET, "kind": "model",
                   "from": "provider/model-large", "to": "provider/model-small"},
        "revert_plan": {"target": TARGET, "kind": "model", "to": "provider/model-large"}
    });
    let ev = evidence(40, "pass", json!(-25));
    let table = [
        (
            "a revert order",
            json!({"messages": revert_order(identity, None)["messages"]}),
            "wait:abc",
        ),
        (
            "a model decision",
            json!({"messages": judge_answer(model_decision, context_with(&rules()))["messages"]}),
            "wait:abc",
        ),
        ("a verdict as a string", json!({"eval": "pass"}), "wait:abc"),
        (
            "a verdict as JSON text",
            json!({"eval": ev.to_string()}),
            "wait:abc",
        ),
        ("a verdict with an empty tag", json!({"eval": ev}), ""),
    ];
    for (what, body, tag) in table {
        let out = run(MUTATOR, on_the_verdict_lane(body, tag));
        assert!(
            out.is_empty(),
            "{what} on `in_eval` is dropped without a row: {out:?}"
        );
    }
}

#[test]
fn a_verdict_body_off_its_lane_is_dropped() {
    if !can_run() {
        return;
    }
    let ev = evidence(40, "pass", json!(-25));
    let mut answer = judge_answer(
        slot_decision(text_change("tool_description"), text_plan()),
        context_with(&rules()),
    );
    answer["eval"] = ev.clone();
    answer["header"]["hop"] = json!({"eval_tag": "wait:abc"});
    let table = [
        json!({"messages": [], "eval": ev,
               "header": {"hop": {"eval_tag": "wait:abc"}, "context": {}}}),
        json!({"messages": [], "eval": ev,
               "header": {"hop": {"route": "revert", "eval_tag": "wait:abc"}, "context": {}}}),
        answer,
    ];
    for m in table {
        let out = run(MUTATOR, m.clone());
        assert!(
            out.is_empty(),
            "a verdict is one by its lane, not by its body: {m} -> {out:?}"
        );
    }
}

#[test]
fn the_verdict_carries_its_contract_keys_and_nothing_else() {
    if !can_run() {
        return;
    }
    let ev = evidence(40, "pass", json!(-25));
    let mut noisy = ev.clone();
    noisy["transcript"] = Value::String("x".repeat(5000));
    noisy["batch_min"] = json!(5);
    noisy["notes"] = json!({"a": 1});
    let carry_of = |out: &[Value]| -> Value {
        assert_eq!(out.len(), 1, "{out:?}");
        serde_json::from_str(out[0]["header"]["carry"].as_str().expect("carry is text"))
            .expect("json")
    };
    let out = run(
        MUTATOR,
        on_the_verdict_lane(json!({"eval": noisy}), "wait:abc"),
    );
    assert_the_declaration_admits(MUTATOR, &out);
    assert_eq!(
        carry_of(&out),
        json!({"eval": ev}),
        "only the six keys of the contract ride on"
    );
    // A verdict missing a key carries what it has; nothing is invented.
    let part = without(ev.clone(), "baseline");
    let out = run(
        MUTATOR,
        on_the_verdict_lane(json!({"eval": part}), "wait:abc"),
    );
    assert_eq!(carry_of(&out), json!({"eval": part}));
}

// ---------------------------------------------------------------------------
// The charter before the answer (review of GH #927, findings I2, I3, M7)
// ---------------------------------------------------------------------------

#[test]
fn a_text_is_sorted_by_its_class_whatever_the_judge_called_its_answer() {
    if !can_run() {
        return;
    }
    let answer = |action: &str, class: &str| -> Value {
        json!({"cycle_id": "cycle:1", "action": action, "reasoning": "r", "simulated": {},
               "change": text_change(class), "revert_plan": text_plan()})
    };
    for action in ["propose", "none", "change"] {
        for (class, outcome, reason) in [
            ("identity", "refused", "identity_out_of_radius"),
            ("persona", "proposed", "owner_approval_required"),
            ("behaviour", "proposed", "owner_approval_required"),
        ] {
            let out = run(
                MUTATOR,
                judge_answer(answer(action, class), context_with(&rules())),
            );
            assert_the_declaration_admits(MUTATOR, &out);
            assert_eq!(
                out.len(),
                1,
                "{action}/{class}: one receipt, nothing else: {out:?}"
            );
            let row = store_call(&out, "insert", "cycles").expect("a receipt")["row"].clone();
            assert_eq!(row["status"], "closed", "{action}/{class}: {row}");
            assert_eq!(row["outcome"], outcome, "{action}/{class}: {row}");
            assert_eq!(row["reason_code"], reason, "{action}/{class}: {row}");
        }
    }
    // A class this loop may change keeps today's way for the other answers.
    for (action, outcome, reason) in [
        ("propose", "proposed", "judge_propose"),
        ("none", "no_action", "judge_none"),
    ] {
        let out = run(
            MUTATOR,
            judge_answer(answer(action, "tool_description"), context_with(&rules())),
        );
        let row = store_call(&out, "insert", "cycles").expect("a receipt")["row"].clone();
        assert_eq!(row["outcome"], outcome, "{action}: {row}");
        assert_eq!(row["reason_code"], reason, "{action}: {row}");
    }
}

/// The judge's charter, as the template seeds it.
const JUDGE_SEED: &str = "../../templates/argus/judge/seed/system.jsonl";

#[test]
fn the_judge_is_told_a_text_is_always_a_change() {
    if !can_run() {
        return;
    }
    let raw = std::fs::read_to_string(JUDGE_SEED).expect("the judge's seed");
    let texts: Vec<(String, String)> = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Value>(l).expect("a seed row"))
        .filter_map(|r| {
            Some((
                r["slot_path"].as_str()?.to_string(),
                r["value"]["text"].as_str()?.to_string(),
            ))
        })
        .collect();
    let answer = &texts
        .iter()
        .find(|(slot, _)| slot == "answer")
        .expect("an `answer` row")
        .1;
    assert!(
        answer
            .contains(r#"a text you would change, in whichever class, is "change" with its class"#),
        "{answer}"
    );
    assert!(answer.contains("you do not send in any form"), "{answer}");
    // `propose` is the quoted action value and nothing else: no row advises
    // it in prose, so nothing steers a text towards the one answer the
    // charter's classes used to be asked under only for `change`.
    for (slot, text) in &texts {
        let low = text.to_lowercase();
        for (i, _) in low.match_indices("propose") {
            let quoted = low[..i].ends_with('"') && low[i + "propose".len()..].starts_with('"');
            assert!(quoted, "`{slot}` advises `propose` in prose: {text}");
        }
    }
}

#[test]
fn an_applied_text_starts_its_window_when_it_lands() {
    if !can_run() {
        return;
    }
    let before = call(MUTATOR, "now", json!([]));
    let before = before.as_str().expect("a timestamp");
    let ev = evidence(40, "pass", json!(-25));
    let out = run(MUTATOR, resumed(json!([wait_row(30)]), &ev));
    let cycle = store_call(&out, "update", "cycles").expect("the cycle is receipted");
    let at = cycle["set"]["at"]
        .as_str()
        .expect("the applied update names its time");
    assert!(
        at >= before,
        "the meter measures from `at`, so it is the application ({before} or later): {cycle}"
    );
    assert_eq!(cycle["where"], json!({"id": "cycle:1"}), "{cycle}");
    // A discarded batch applied nothing, and moves no anchor.
    let out = run(
        MUTATOR,
        resumed(json!([wait_row(30)]), &evidence(40, "fail", json!(-25))),
    );
    let cycle = store_call(&out, "update", "cycles").expect("the cycle is receipted");
    assert!(cycle["set"].get("at").is_none(), "{cycle}");
}

#[test]
fn a_class_is_read_case_folded() {
    if !can_run() {
        return;
    }
    let table = [
        ("Identity", json!(["refused", "identity_out_of_radius"])),
        (" PERSONA ", json!(["proposed", "owner_approval_required"])),
        ("Tool_Description", json!(["eval", ""])),
        (
            "Greeting",
            json!(["refused", "key_outside_radius_greeting"]),
        ),
    ];
    for (class, want) in table {
        let got = call(
            MUTATOR,
            "classify_slot",
            json!([text_change(class), rules()]),
        );
        assert_eq!(got, want, "class {class:?}");
    }
    let out = run(
        MUTATOR,
        judge_answer(
            json!({"cycle_id": "cycle:1", "action": "propose", "reasoning": "r",
                   "simulated": {}, "change": text_change("IDENTITY"),
                   "revert_plan": text_plan()}),
            context_with(&rules()),
        ),
    );
    let row = store_call(&out, "insert", "cycles").expect("a receipt")["row"].clone();
    assert_eq!(row["reason_code"], "identity_out_of_radius", "{row}");
}

#[test]
fn a_rule_row_of_nothing_but_commas_is_a_missing_row() {
    if !can_run() {
        return;
    }
    assert_eq!(
        call(
            MUTATOR,
            "rule_set",
            json!([[{"kind": "never", "value": " , , "}], "never", "identity"])
        ),
        json!(["identity"])
    );
    let commas = |kind: &str| -> Value {
        let mut r = rules();
        for row in r.as_array_mut().expect("the rules are a list") {
            if row["kind"] == kind {
                row["value"] = json!(" , ,");
            }
        }
        r
    };
    let table = [
        (
            "never",
            "identity",
            json!(["refused", "identity_out_of_radius"]),
        ),
        (
            "owner_only",
            "persona",
            json!(["proposed", "owner_approval_required"]),
        ),
        (
            "radius",
            "tool_description",
            json!(["refused", "outside_radius"]),
        ),
        (
            "slot_classes",
            "tool_description",
            json!(["refused", "key_outside_radius_tool_description"]),
        ),
    ];
    for (kind, class, want) in table {
        let got = call(
            MUTATOR,
            "classify_slot",
            json!([text_change(class), commas(kind)]),
        );
        assert_eq!(got, want, "`{kind}` of commas, class {class}");
    }
}

#[test]
fn a_batch_size_is_read_like_the_delta() {
    if !can_run() {
        return;
    }
    let apply = json!(["apply", ""]);
    let none = json!(["discarded", "no_evidence_0"]);
    let table = [
        (json!(40.0), apply.clone()),
        (json!("40"), apply.clone()),
        (json!(" 40 "), apply.clone()),
        (json!("40.0"), apply.clone()),
        (json!(10.0), json!(["discarded", "no_evidence_10"])),
        (json!(40.5), none.clone()),
        (json!("forty"), none.clone()),
        (json!(true), none.clone()),
        (json!(null), none.clone()),
        (json!("nan"), none.clone()),
        (json!("inf"), none),
    ];
    for (n, want) in table {
        let ev = with(evidence(40, "pass", json!(-25)), "n", n.clone());
        let got = call(MUTATOR, "evidence_ok", json!([ev, goal(), 30]));
        assert_eq!(got, want, "n = {n}");
    }
    // The delta as text, the half that was already read this way.
    assert_eq!(
        call(
            MUTATOR,
            "evidence_ok",
            json!([evidence(40, "pass", json!("-25")), goal(), 30])
        ),
        apply
    );
}
