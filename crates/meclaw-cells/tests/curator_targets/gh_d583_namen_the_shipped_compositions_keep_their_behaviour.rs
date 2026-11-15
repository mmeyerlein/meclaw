//! d58-3 namen (GH #1525, contract points 1, 2, 4 and 5; R-583-1 form B)
//! -- the shipped compositions keep their behaviour.
//!
//! WHY this file exists: the names left the defaults of the curator and the
//! lead (`errand_slot`, `agent_sources`, `side_slot`, `bridge_places` ship
//! empty; `recall_needs`, `state_needs` ship `[]`; the pending note ships a
//! neutral text). A composition that relied on a default would change its
//! behaviour silently -- no errand, no source, no side call, no recall. So
//! each shipped composition sets, in its own `override_params`, exactly the
//! value its hive shipped before this strand; the corpus `d38_c0` and the
//! goldens gh277 stay byte-equal but for these overrides (their own tests).
//! The values below are the shipped defaults of the base of this strand
//! (`d58-3/basis` f3a32c12ca), copied by hand: they are the behaviour that
//! must not move.
//!
//! Reads the shipped tree and runs the pure half of the shipped policy
//! (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The compositions that ship a curator, and whether they ship a lead.
const COMPOSITIONS: [(&str, bool); 2] = [("talky", true), ("cogny", false)];

/// The pending note the policy wrote before this strand for the errand `c1`
/// pending since `t0` (`pending_text`).
const TODAY_PENDING: &str = "A consult is pending (id c1, since t0): the backend works on this \
     request and its answer comes on its own. Do not consult again; say it is being looked into \
     if asked.";

/// `(marker, cell, knob, today's shipped value)`: the overrides every
/// composition with a curator sets.
fn curator_values() -> Vec<(&'static str, &'static str, &'static str, Value)> {
    vec![
        ("curator", "policy", "recall_needs", json!(["memory"])),
        (
            "curator",
            "policy",
            "state_needs",
            json!(["memory", "orga"]),
        ),
        ("curator", "policy", "errand_slot", json!("consult")),
        // Bauer 3b (N9b): the state block's entry of a pending ask.
        ("curator", "policy", "pending_state", json!("a consult")),
        ("curator", "intake", "errand_slot", json!("consult")),
        (
            "curator",
            "intake",
            "agent_sources",
            json!("advice:cogny,delegation:voice"),
        ),
        ("collector", "assemble", "recall_needs", json!(["memory"])),
        // Fix round (review M2).
        (
            "curator",
            "push",
            "nothing_prefix",
            json!("Nothing in this memory answers"),
        ),
    ]
}

/// The overrides a composition with a lead sets on it.
fn lead_values() -> Vec<(&'static str, &'static str, &'static str, Value)> {
    vec![
        ("lead", "lead", "recall_needs", json!(["memory"])),
        ("lead", "lead", "side_slot", json!("bridge")),
        (
            "lead",
            "lead",
            "bridge_places",
            json!(
                "{\"places\": {\"weather\": \"at the weather service\", \"web\": \"on the web\"}, \
                 \"rest\": \"in other places\", \"before\": \", \", \"join\": \" and \"}"
            ),
        ),
        // Bauer 3a: the side texts name the speaking surface, the shipped
        // ones no longer; the interrupt's section is the surface's word.
        ("lead", "lead", "bridge_texts", json!(NAMEN_BRIDGE_TEXTS)),
        ("lead", "lead", "interrupt_section", json!("interrupt")),
        // Fix round (review M1): the renamed knobs at today's values.
        ("lead", "lead", "recall_to_fallback", json!("0")),
        ("lead", "lead", "ask_pending_ms", json!(120000)),
    ]
}

/// The `override_params` of the marker `<comp>/<marker>/config.json`.
fn overrides(comp: &str, marker: &str) -> Value {
    let p = repo(&format!("templates/{comp}/{marker}/config.json"));
    if !p.is_file() {
        return json!({});
    }
    read_json(&p)["override_params"].clone()
}

/// The shipped config of a cell of a hive (`curator`, `lead`, `collector`).
fn hive_cell(hive: &str, cell: &str) -> Value {
    read_json(&repo(&format!("templates/{hive}/{cell}/config.json")))
}

#[test]
fn gh_d583_namen_the_shipped_compositions_keep_their_behaviour() {
    if !shipped() || !lead_shipped() {
        return;
    }
    let mut wrong = Vec::new();
    // The defaults name no composition: empty, or an empty list.
    for (hive, cell, knob, empty) in [
        ("curator", "policy", "errand_slot", json!("")),
        ("curator", "intake", "errand_slot", json!("")),
        ("curator", "intake", "agent_sources", json!("")),
        ("lead", "lead", "side_slot", json!("")),
        ("lead", "lead", "bridge_places", json!("")),
        ("curator", "policy", "recall_needs", json!([])),
        ("curator", "policy", "state_needs", json!([])),
        ("lead", "lead", "recall_needs", json!([])),
        ("collector", "assemble", "recall_needs", json!([])),
        ("curator", "push", "nothing_prefix", json!("")),
    ] {
        let cfg = hive_cell(hive, cell);
        if cfg["params"].get(knob) != Some(&empty) {
            wrong.push(format!(
                "{hive}/{cell} ships `{knob}` {} -- the default is {empty}",
                cfg["params"].get(knob).cloned().unwrap_or(Value::Null)
            ));
        }
        if let Some(d) = cfg["contract"]["settings"][knob].get("default")
            && *d != empty
        {
            wrong.push(format!(
                "{hive}/{cell} declares `{knob}` with the default {d} -- not {empty}"
            ));
        }
    }
    // Bauer 3b (N9b): the shipped state entry of a pending ask names no
    // composition's word.
    let (open, err) = policy_scope(
        json!({}),
        "state_of_rows([], [], {}, ARGS, None, 's1', 1000, '2026-11-14')['open']",
        json!({"errand:s1": "{\"until\": 2000}"}),
    );
    if open != json!(["an answer from another model"]) {
        wrong.push(format!(
            "the shipped policy lists a pending ask as {open} -- the neutral entry is \
             [\"an answer from another model\"] {err}"
        ));
    }
    // Each composition sets today's value itself.
    for (comp, with_lead) in COMPOSITIONS {
        let mut wanted = curator_values();
        if with_lead {
            wanted.extend(lead_values());
        }
        for (marker, cell, knob, today) in wanted {
            let got = overrides(comp, marker)[cell].get(knob).cloned();
            if got.as_ref() != Some(&today) {
                wrong.push(format!(
                    "{comp}/{marker} override `{cell}.{knob}`: {} -- today's value is {today}",
                    got.unwrap_or(Value::Null)
                ));
            }
        }
        // The pending note the speaking model reads under the
        // composition's overrides is today's, word for word.
        let (got, err) = policy_scope(
            overrides(comp, "curator")["policy"].clone(),
            "pending_text(ARGS)",
            json!({"ask_id": "c1", "consult_id": "c1", "at": "t0"}),
        );
        if got != json!(TODAY_PENDING) {
            wrong.push(format!(
                "{comp}: the pending note under its overrides is {got} -- today's is \
                 {TODAY_PENDING:?} {err}"
            ));
        }
        // Bauer 3b (N9b): the state block lists a pending ask in today's
        // words under the composition's overrides.
        let (open, err) = policy_scope(
            overrides(comp, "curator")["policy"].clone(),
            "state_of_rows([], [], {}, ARGS, None, 's1', 1000, '2026-11-14')['open']",
            json!({"errand:s1": "{\"until\": 2000}"}),
        );
        if open != json!(["a consult"]) {
            wrong.push(format!(
                "{comp}: a pending ask stands in the state block as {open} under its \
                 overrides -- today's is [\"a consult\"] {err}"
            ));
        }
        if overrides(comp, "curator")["policy"]
            .get("pending_note")
            .is_none()
        {
            wrong.push(format!(
                "{comp}/curator override `policy.pending_note` is not set -- the neutral \
                 default would change what its speaking model reads"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} place(s) where a composition would change its behaviour:\n  {}",
        wrong.len(),
        wrong.join("\n  ")
    );
}
