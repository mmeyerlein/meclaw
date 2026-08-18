//! The steward's loop, run against its real scripts (GitHub #155).
//!
//! A colony that can be mutated by an agent is not yet a colony that improves
//! itself. What is missing is the loop: measure, decide, act through the same
//! gates everybody else uses, verify, measure the effect, and then keep the
//! change or take it back — with a record of each step.
//!
//! The tests below hold up the parts of that claim that can be wrong quietly:
//!
//! - the mutation **radius** (model choice and numeric params, nothing else),
//! - the **charter rule** that a cycle without a pre-authored revert plan is
//!   invalid, and that the plan must actually restore the original,
//! - the **significance floor**, so noise never triggers action,
//! - the **probe**, which fails closed when it cannot look,
//! - and that every one of those outcomes is a receipt rather than a silence.

use std::io::Write;
use std::process::{Command, Stdio};

const MUTATOR: &str = "../../templates/steward/mutator/config.json";
const METER: &str = "../../templates/steward/meter/config.json";
const PROBE: &str = "../../templates/steward/probe/config.json";
const CHARTER: &str = "../../templates/steward/charter/config.json";
const HIVE: &str = "../../templates/steward/config.json";

fn resolve_vars(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail.find('}').expect("unterminated ${...}");
        if let Some((_, default)) = tail[..end].split_once(":-") {
            out.push_str(default);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn config(path: &str) -> serde_json::Value {
    let raw = std::fs::read_to_string(path).expect("template config");
    serde_json::from_str(&raw).expect("config json")
}

fn script(path: &str) -> String {
    resolve_vars(
        config(path)["params"]["script_inline"]
            .as_str()
            .expect("script"),
    )
}

fn emit(script: &str, doc: serde_json::Value) -> Vec<serde_json::Value> {
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(&meclaw_testing::code_stdin_bytes(&doc))
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "script exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("not JSON ({e}): {}", String::from_utf8_lossy(&out.stdout)))
}

/// A judge decision arriving at the mutator.
fn decision(args: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "messages": [{
            "origin": "assistant", "type": "tool_call", "id": "c1",
            "text": args.to_string()
        }],
        "header": {"hop": {}, "context": {"st_cycle": "cycle:1", "st_goal": "goal:llm-cost"}}
    })
}

/// The row a `store` insert would write, if there is one.
fn inserted(out: &[serde_json::Value], table: &str) -> Option<serde_json::Value> {
    out.iter().find_map(|m| {
        let text = m["messages"][0]["text"].as_str()?;
        let args: serde_json::Value = serde_json::from_str(text).ok()?;
        (args["operation"] == "insert" && args["table"] == table).then(|| args["row"].clone())
    })
}

/// The mutation body, if the cell emitted one.
fn mutation(out: &[serde_json::Value]) -> Option<&serde_json::Value> {
    out.iter().find(|m| m["header"]["msg_type"] == "mutation")
}

fn a_valid_change() -> serde_json::Value {
    serde_json::json!({
        "cycle_id": "cycle:1",
        "action": "change",
        "reasoning": "opus served 40 calls at 2.1M prompt tokens; sonnet at the same counts costs 0.41 EUR against 1.90 EUR, and the quality gate held last window.",
        "simulated": {"counterfactual_eur": 0.41, "actual_eur": 1.90},
        "change": {"target": "/main/talky/brain", "kind": "model",
                   "from": "anthropic/claude-opus-4", "to": "anthropic/claude-sonnet-4"},
        "revert_plan": {"target": "/main/talky/brain", "kind": "model",
                        "to": "anthropic/claude-opus-4"}
    })
}

// ---------------------------------------------------------------------------
// The charter rule
// ---------------------------------------------------------------------------

#[test]
fn a_cycle_without_a_revert_plan_is_invalid() {
    let mut args = a_valid_change();
    args["revert_plan"] = serde_json::json!({});
    let out = emit(&script(MUTATOR), decision(args));
    assert!(
        mutation(&out).is_none(),
        "nothing may be changed without a way back: {out:?}"
    );
    let row = inserted(&out, "cycles").expect("the refusal is a receipt");
    assert_eq!(row["outcome"], "refused");
    assert_eq!(row["reason_code"], "no_revert_plan");
}

#[test]
fn a_revert_plan_that_does_not_lead_back_is_refused() {
    // Structurally perfect, semantically useless: it "reverts" to the value we
    // are moving TO. Every field check would pass.
    let mut args = a_valid_change();
    args["revert_plan"]["to"] = serde_json::json!("anthropic/claude-sonnet-4");
    let out = emit(&script(MUTATOR), decision(args));
    assert!(mutation(&out).is_none());
    let row = inserted(&out, "cycles").expect("a receipt");
    assert_eq!(row["reason_code"], "revert_plan_is_not_inverse");
}

#[test]
fn a_revert_plan_pointing_at_another_cell_is_refused() {
    let mut args = a_valid_change();
    args["revert_plan"]["target"] = serde_json::json!("/main/cogny/brain");
    let out = emit(&script(MUTATOR), decision(args));
    assert!(mutation(&out).is_none());
    assert_eq!(
        inserted(&out, "cycles").unwrap()["reason_code"],
        "revert_plan_wrong_target"
    );
}

#[test]
fn a_revert_plan_that_restores_something_else_is_refused() {
    let mut args = a_valid_change();
    args["revert_plan"]["to"] = serde_json::json!("anthropic/claude-haiku-3");
    let out = emit(&script(MUTATOR), decision(args));
    assert!(mutation(&out).is_none());
    assert_eq!(
        inserted(&out, "cycles").unwrap()["reason_code"],
        "revert_plan_does_not_restore_the_original"
    );
}

// ---------------------------------------------------------------------------
// The radius
// ---------------------------------------------------------------------------

#[test]
fn a_topology_change_is_refused_however_well_argued() {
    let mut args = a_valid_change();
    args["change"] = serde_json::json!({
        "target": "/main/talky/brain", "kind": "topology",
        "from": "", "to": "rewire the collector"
    });
    args["revert_plan"] = serde_json::json!({
        "target": "/main/talky/brain", "kind": "topology", "to": "rewire it back"
    });
    let out = emit(&script(MUTATOR), decision(args));
    assert!(mutation(&out).is_none(), "radius v1 executes no topology");
    assert_eq!(
        inserted(&out, "cycles").unwrap()["reason_code"],
        "outside_radius"
    );
}

#[test]
fn a_proposal_is_recorded_rather_than_executed() {
    // The judge's legitimate way to raise a topology idea. It has to be
    // findable later — a human who cannot find the proposal has not been sent
    // one.
    let out = emit(
        &script(MUTATOR),
        decision(serde_json::json!({
            "cycle_id": "cycle:1",
            "action": "propose",
            "reasoning": "the real win is moving the summariser off the hot path",
            "simulated": {},
            "change": {},
            "revert_plan": {}
        })),
    );
    assert!(mutation(&out).is_none());
    let row = inserted(&out, "cycles").expect("a receipt");
    assert_eq!(row["outcome"], "proposed");
    assert!(
        row["judged"]["reasoning"]
            .as_str()
            .unwrap()
            .contains("summariser"),
        "the proposal's content survives into the record: {row}"
    );
}

#[test]
fn a_numeric_step_beyond_the_limit_is_refused() {
    let mut args = a_valid_change();
    args["change"] = serde_json::json!({
        "target": "/main/talky/collector", "kind": "numeric_param",
        "key": "max_iter", "from": 4, "to": 40
    });
    args["revert_plan"] = serde_json::json!({
        "target": "/main/talky/collector", "kind": "numeric_param",
        "key": "max_iter", "to": 4
    });
    let out = emit(&script(MUTATOR), decision(args));
    assert!(mutation(&out).is_none());
    assert!(
        inserted(&out, "cycles").unwrap()["reason_code"]
            .as_str()
            .unwrap()
            .starts_with("step_too_large"),
        "one cycle's mistake has to stay small enough to measure out of"
    );
}

// ---------------------------------------------------------------------------
// What a good cycle does
// ---------------------------------------------------------------------------

#[test]
fn an_accepted_change_travels_the_normal_mutation_lane() {
    let out = emit(&script(MUTATOR), decision(a_valid_change()));
    let m = mutation(&out).expect("a mutation leaves the cell");
    assert_eq!(m["scope"], "/main/talky");
    assert_eq!(
        m["diff"]["swap_nodes"][0],
        serde_json::json!({"name": "brain", "params": {"model": "anthropic/claude-sonnet-4"}})
    );
    // The ordinary shape, with no operator flag anywhere near it.
    let dump = serde_json::to_string(m).unwrap();
    assert!(!dump.contains("operator"), "no operator lane: {dump}");
    assert!(!dump.contains("force"), "no override: {dump}");
}

#[test]
fn an_applied_cycle_stays_open_and_fires_the_probe() {
    let out = emit(&script(MUTATOR), decision(a_valid_change()));
    let row = inserted(&out, "cycles").expect("a receipt");
    assert_eq!(
        row["status"], "applied",
        "open until its effect is measured"
    );
    assert_eq!(row["outcome"], "");
    assert_eq!(row["revert_plan"]["to"], "anthropic/claude-opus-4");
    assert!(
        row["simulated"]["counterfactual_eur"].is_number(),
        "what it simulated is part of the record: {row}"
    );

    let probe = out
        .iter()
        .find(|m| m["header"]["route"] == "probe")
        .expect("the health check is fired at once, not on the next tick");
    let args: serde_json::Value =
        serde_json::from_str(probe["messages"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(args["target"], "/main/talky/brain");
}

#[test]
fn a_numeric_param_within_the_limit_goes_through_as_a_params_swap() {
    let mut args = a_valid_change();
    args["change"] = serde_json::json!({
        "target": "/main/talky/collector", "kind": "numeric_param",
        "key": "max_iter", "from": 4, "to": 5
    });
    args["revert_plan"] = serde_json::json!({
        "target": "/main/talky/collector", "kind": "numeric_param",
        "key": "max_iter", "to": 4
    });
    let out = emit(&script(MUTATOR), decision(args));
    let m = mutation(&out).expect("a mutation");
    assert_eq!(
        m["diff"]["swap_nodes"][0]["params"]["max_iter"],
        serde_json::json!(5),
        "an integer stays an integer on the wire"
    );
}

// ---------------------------------------------------------------------------
// The revert
// ---------------------------------------------------------------------------

#[test]
fn a_revert_uses_the_plan_that_was_authored_beforehand() {
    let out = emit(
        &script(MUTATOR),
        serde_json::json!({
            "messages": [{"origin":"assistant","type":"tool_call","id":"c1","text":
                serde_json::json!({
                    "op": "revert",
                    "cycle_id": "cycle:1",
                    "plan": {"target": "/main/talky/brain", "kind": "model",
                             "to": "anthropic/claude-opus-4"}
                }).to_string()}],
            "header": {"hop": {}, "context": {}}
        }),
    );
    let m = mutation(&out).expect("the inverse mutation");
    assert_eq!(
        m["diff"]["swap_nodes"][0]["params"]["model"],
        "anthropic/claude-opus-4"
    );
    assert_eq!(m["header"]["outcome"], "reverted");
}

#[test]
fn a_revert_without_a_plan_is_recorded_rather_than_improvised() {
    let out = emit(
        &script(MUTATOR),
        serde_json::json!({
            "messages": [{"origin":"assistant","type":"tool_call","id":"c1","text":
                serde_json::json!({"op": "revert", "cycle_id": "cycle:1", "plan": {}}).to_string()}],
            "header": {"hop": {}, "context": {}}
        }),
    );
    assert!(mutation(&out).is_none(), "nothing is invented here");
    let text = out[0]["messages"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("revert_plan_missing_at_revert_time"),
        "{text}"
    );
}

// ---------------------------------------------------------------------------
// The measurement side
// ---------------------------------------------------------------------------

#[test]
fn an_inert_charter_makes_the_steward_do_nothing_at_all() {
    // The resting state of a freshly grown steward: every goal disabled. It
    // must be silent, not an error — nobody has said what to pursue yet.
    let out = emit(
        &script(METER),
        serde_json::json!({
            "messages": [{"origin":"tool","type":"tool_result","id":"c1","text":"[]"}],
            "header": {"hop": {"operation": "select"},
                       "context": {"st_phase": "goals", "st_carry": "{}"}}
        }),
    );
    assert!(out.is_empty(), "an inert charter emits nothing: {out:?}");
}

#[test]
fn a_tick_reads_the_charter_before_anything_else() {
    let out = emit(
        &script(METER),
        serde_json::json!({
            "messages": [],
            "header": {"hop": {"schedule_name": "steward-cycle"}, "context": {}}
        }),
    );
    let args: serde_json::Value =
        serde_json::from_str(out[0]["messages"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(args["table"], "goals");
    assert_eq!(args["where"]["enabled"], 1, "only enabled goals");
    assert_eq!(out[0]["header"]["route"], "cstore");
}

#[test]
fn too_little_traffic_closes_the_cycle_as_skipped_with_the_count() {
    // "We did not look" and "we looked and saw nothing" are different facts.
    let carry = serde_json::json!({
        "goals": [{"id": "goal:llm-cost", "metric": "llm_cost", "direction": "lower",
                   "window_minutes": 60, "min_samples": 30, "min_delta_pct": 10,
                   "quality_gate": "answer_quality", "enabled": 1}],
        "rules": []
    });
    let out = emit(
        &script(METER),
        serde_json::json!({
            "messages": [{"origin":"tool","type":"tool_result","id":"c1","text":"[]"}],
            "header": {"hop": {"operation": "select"},
                       "context": {"st_phase": "open", "st_carry": carry.to_string()}}
        }),
    );
    let row = inserted(&out, "cycles").expect("a receipt even when nothing happened");
    assert_eq!(row["outcome"], "skipped");
    assert!(
        row["reason_code"]
            .as_str()
            .unwrap()
            .starts_with("below_min_samples_"),
        "the count is in the reason: {row}"
    );
    assert_eq!(row["status"], "closed");
}

#[test]
fn an_observe_only_goal_never_proposes_anything() {
    let carry = serde_json::json!({
        "goals": [{"id": "goal:dlq-watch", "metric": "dlq_rate", "direction": "observe",
                   "window_minutes": 60, "min_samples": 0, "min_delta_pct": 0,
                   "quality_gate": "", "enabled": 1}],
        "rules": []
    });
    let out = emit(
        &script(METER),
        serde_json::json!({
            "messages": [{"origin":"tool","type":"tool_result","id":"c1","text":"[]"}],
            "header": {"hop": {"operation": "select"},
                       "context": {"st_phase": "open", "st_carry": carry.to_string()}}
        }),
    );
    assert!(
        !out.iter().any(|m| m["header"]["route"] == "judge"),
        "an observe-only goal must not reach the judge: {out:?}"
    );
    assert_eq!(inserted(&out, "cycles").unwrap()["outcome"], "observed");
}

#[test]
fn no_model_is_reachable_from_the_measuring_path() {
    // The property that makes the numbers trustworthy: the meter and the probe
    // are code cells, and nothing in them talks to a provider.
    for path in [METER, PROBE] {
        let cfg = config(path);
        assert_eq!(cfg["cell"]["type"], "code", "{path}");
        // A `code` cell cannot call a provider by construction — it has no
        // api_key, no base_url and no model param, and there is no param that
        // would give it one.
        let params = cfg["params"].as_object().expect("params");
        for forbidden in ["api_key", "base_url", "provider", "model", "tools"] {
            assert!(
                !params.contains_key(forbidden),
                "{path} carries {forbidden}, which is llm-cell shaped"
            );
        }
        // …and the script itself reaches nothing over the network. Checked on
        // the imports rather than on the prose, so a comment mentioning a
        // provider does not fail a test about capability.
        let script = params["script_inline"].as_str().expect("script");
        for line in script.lines() {
            let l = line.trim_start();
            if l.starts_with("import ") || l.starts_with("from ") {
                for net in ["http", "urllib", "socket", "requests"] {
                    assert!(
                        !l.contains(net),
                        "{path} imports {net}: the measuring path must reach nothing"
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The probe
// ---------------------------------------------------------------------------

#[test]
fn a_probe_that_cannot_look_reports_unhealthy_rather_than_fine() {
    // Fail closed: "found nothing" and "found it healthy" must never read the
    // same. The colony.db path resolves to nothing in this working directory.
    let out = emit(
        &script(PROBE),
        serde_json::json!({
            "messages": [{"origin":"assistant","type":"tool_call","id":"c1","text":
                serde_json::json!({"op":"probe","cycle_id":"cycle:1",
                                   "target":"/main/talky/brain"}).to_string()}],
            "header": {"hop": {}, "context": {}}
        }),
    );
    let update = out
        .iter()
        .find_map(|m| {
            let t = m["messages"][0]["text"].as_str()?;
            let a: serde_json::Value = serde_json::from_str(t).ok()?;
            (a["operation"] == "update").then_some(a)
        })
        .expect("the verdict is written to the receipt");
    assert_eq!(update["set"]["verified"]["verdict"], "unhealthy");
    assert_eq!(update["set"]["verified"]["reason"], "probe_unavailable");

    // …and it goes looking for the revert plan rather than inventing one.
    let select = out
        .iter()
        .find_map(|m| {
            let t = m["messages"][0]["text"].as_str()?;
            let a: serde_json::Value = serde_json::from_str(t).ok()?;
            (a["operation"] == "select").then_some(a)
        })
        .expect("it fetches the plan that was authored beforehand");
    assert_eq!(select["columns"][1], "revert_plan");
}

#[test]
fn an_unhealthy_cycle_without_a_stored_plan_closes_for_a_human() {
    let out = emit(
        &script(PROBE),
        serde_json::json!({
            "messages": [{"origin":"tool","type":"tool_result","id":"c1",
                          "text": "[{\"id\":\"cycle:1\",\"revert_plan\":{}}]"}],
            "header": {"hop": {"operation": "select"},
                       "context": {"st_phase": "plan", "st_cycle": "cycle:1",
                                   "st_reason": "unhealthy"}}
        }),
    );
    let args: serde_json::Value =
        serde_json::from_str(out[0]["messages"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(args["set"]["outcome"], "unhealthy_no_plan");
    assert_eq!(args["set"]["status"], "closed");
}

#[test]
fn an_unhealthy_cycle_with_a_plan_reverts_at_once() {
    let out = emit(
        &script(PROBE),
        serde_json::json!({
            "messages": [{"origin":"tool","type":"tool_result","id":"c1",
                "text": serde_json::json!([{
                    "id": "cycle:1",
                    "revert_plan": {"target":"/main/talky/brain","kind":"model",
                                    "to":"anthropic/claude-opus-4"}
                }]).to_string()}],
            "header": {"hop": {"operation": "select"},
                       "context": {"st_phase": "plan", "st_cycle": "cycle:1",
                                   "st_reason": "errors_3"}}
        }),
    );
    assert_eq!(out[0]["header"]["route"], "revert");
    let args: serde_json::Value =
        serde_json::from_str(out[0]["messages"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(args["op"], "revert");
    assert_eq!(args["plan"]["to"], "anthropic/claude-opus-4");
}

// ---------------------------------------------------------------------------
// The shape of the hive itself
// ---------------------------------------------------------------------------

#[test]
fn a_fresh_charter_pursues_nothing() {
    let seed = std::fs::read_to_string("../../templates/steward/charter/seed/goals.jsonl")
        .expect("the goals seed ships with the template");
    for line in seed.lines().skip(1).filter(|l| !l.trim().is_empty()) {
        let row: serde_json::Value = serde_json::from_str(line).expect("seed row");
        assert_eq!(
            row["enabled"], 0,
            "a freshly grown steward must change nothing until somebody means it: {row}"
        );
    }
}

#[test]
fn the_charter_carries_the_radius_and_the_revert_rule_as_data() {
    let seed = std::fs::read_to_string("../../templates/steward/charter/seed/rules.jsonl")
        .expect("the rules seed ships with the template");
    let rows: Vec<serde_json::Value> = seed
        .lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("seed row"))
        .collect();
    let kinds: Vec<&str> = rows.iter().map(|r| r["kind"].as_str().unwrap()).collect();
    for required in [
        "radius",
        "require_revert_plan",
        "budget_eur_per_cycle",
        "quality_floor_pct",
    ] {
        assert!(
            kinds.contains(&required),
            "the charter is missing {required}"
        );
    }
    let radius = rows.iter().find(|r| r["kind"] == "radius").unwrap();
    assert_eq!(
        radius["value"], "model,numeric_params",
        "the radius widens by editing this row, never by editing code"
    );
}

#[test]
fn the_vault_of_this_hive_is_its_charter_and_its_stores_are_internal() {
    // Both stores declare an internal write surface: nothing outside the hive
    // writes the charter it is governed by, or the receipts it is judged on.
    for path in [CHARTER, "../../templates/steward/receipts/config.json"] {
        assert_eq!(
            config(path)["params"]["write_surface"],
            "internal",
            "{path} must not be writable from outside the hive"
        );
    }
}

#[test]
fn the_hive_is_sealed_to_its_own_path_and_states_its_lanes() {
    // GH #197: this used to pin `ports == ["meter", "mutator"]`, which spelled
    // out two CELL names — exactly what the boundary ruling of 2026-08-18 took
    // away. What it was protecting is the property below: nothing outside can
    // name anything inside, and what a caller may ask for is said in lanes.
    let hive = config(HIVE);
    let ports = hive["params"]["ports"]
        .as_array()
        .expect("the hive declares a port list");
    assert!(
        ports.is_empty(),
        "the hive path is the address and the lane is the port: {ports:?}"
    );

    let contract = hive["params"]["contract"]
        .as_object()
        .expect("a sealed hive owes a contract");
    let cells = [
        "charter", "clock", "judge", "meter", "mutator", "probe", "receipts",
    ];
    let mut lanes = 0usize;
    for side in ["accepts", "emits"] {
        for lane in contract[side].as_array().expect("accepts/emits is a list") {
            let route = lane["route"].as_str().expect("a lane names a route");
            assert!(
                !cells.contains(&route),
                "'{route}' is a cell of this hive — a lane says what a caller wants, never where \
                 it lands"
            );
            lanes += 1;
        }
    }
    assert!(
        lanes >= 3,
        "the contract says almost nothing: {lanes} lanes"
    );
}

#[test]
fn every_edge_of_the_hive_stays_inside_it() {
    let hive = config(HIVE);
    for edge in hive["params"]["graph"]["edges"].as_array().unwrap() {
        for role in ["from", "to"] {
            let ep = edge[role].as_str().unwrap();
            // `.` is the hive itself — the door and the exit of the sealed form
            // (GH #197), and the one endpoint that is still inside this subtree
            // without being below it. Everything else must be a child.
            assert!(
                ep == "." || ep.starts_with("./"),
                "a template has no edges leaving its own subtree: {ep}"
            );
        }
    }
}
