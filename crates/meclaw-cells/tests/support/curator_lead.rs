//! D58 lead (GH #1474) -- the switch is a hive of its own.
//!
//! WHY this file exists: the cells `curator/lead` and `curator/lead_state`
//! leave the curator and become the template `lead`. The curator holds no
//! choice of a model any more; a composite that chooses between targets
//! wires a `lead` hive beside its curator and says so with one knob of the
//! curator's policy, and a composite without one names its target itself
//! (`hop.target`). The pins of the strand share the paths, the knob and the
//! lanes from here.
//!
//! Shared by the `gh_d58_lead_*` pins; no model.

#![allow(dead_code)]

use crate::curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The knob of the curator instance (`./policy`) that says a lead is wired
/// beside it: `"1"` wired, anything else (shipped: `"0"`) none
/// (OR-d58-2.lead.1, the name is this strand's).
pub const LEAD_KNOB: &str = "lead_wired";

/// The template of the switch, a hive of its own.
pub const LEAD_TEMPLATE: &str = "templates/lead";

/// The two cells of the switch inside its hive: the lead and its store.
pub const LEAD_HIVE_CELLS: [&str; 2] = ["lead", "state"];

/// The three lanes from the curator to the lead.
pub const TO_LEAD: [&str; 3] = ["lead_ask", "lead_spec", "lead_note"];

/// The lane the lead's verdict comes home on.
pub const FROM_LEAD: &str = "in_lead";

/// D58 moveout (GH #1476, contract point 8): the lane the lead's orders
/// reach the curator on -- the curator's own `in_feed`, passed on unchanged.
pub const ORDERS_FROM_LEAD: &str = "in_feed";

/// The knobs that make the choice of a model: they live in the lead and in
/// no cell of the curator.
pub const CHOICE_KNOBS: [&str; 15] = [
    "path_routes",
    "departures",
    "fallback",
    "spec_target",
    "bridge_target",
    "errand_family",
    "memory_to_brain",
    "quick_max_iter",
    "quick_speculative",
    "spec_tools",
    "spec_called_turns",
    "consult_pending_ms",
    "delta_speculation",
    "interrupt_on_delta",
    "interrupt_questions",
];

/// The knobs that name a target or an errand class: shipped empty.
pub const TARGET_KNOBS: [&str; 5] = [
    "path_routes",
    "departures",
    "fallback",
    "spec_target",
    "bridge_target",
];

/// The config of a cell of the lead hive (`lead` or `state`).
pub fn lead_hive_cell(name: &str) -> Value {
    let p = repo(&format!("{LEAD_TEMPLATE}/{name}/config.json"));
    assert!(
        p.is_file(),
        "{} is missing: the switch is not a hive of its own yet",
        p.display()
    );
    read_json(&p)
}

/// The hive config of the lead template.
pub fn lead_hive() -> Value {
    let p = repo(&format!("{LEAD_TEMPLATE}/config.json"));
    assert!(
        p.is_file(),
        "{} is missing: the switch is not a hive of its own yet",
        p.display()
    );
    read_json(&p)
}

/// The curator without a lead: the knob says none is wired.
pub fn bare(over: &[(&str, &str, Value)]) -> Hive {
    let mut all = vec![("policy", LEAD_KNOB, json!("0"))];
    all.extend_from_slice(over);
    Hive::with(&all)
}

/// The curator with a lead wired the way talky wires it.
pub fn wired(over: &[(&str, &str, Value)]) -> Hive {
    let mut all = vec![("policy", LEAD_KNOB, json!("1"))];
    all.extend_from_slice(over);
    Hive::with(&all)
}

/// Every message on a lane to the lead that left the curator.
pub fn to_lead(h: &Hive) -> Vec<Msg> {
    h.out
        .iter()
        .filter(|m| TO_LEAD.contains(&m.route()))
        .cloned()
        .collect()
}

/// The edges of a template's hive config (`params.graph.edges`).
pub fn edges_of(cfg: &Value) -> Vec<Value> {
    cfg["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The routes a hive contract lists under `key` (`accepts` or `emits`).
pub fn contract_routes(cfg: &Value, key: &str) -> Vec<String> {
    cfg["params"]["contract"][key]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|e| {
            e.get("route")
                .and_then(Value::as_str)
                .or_else(|| e.as_str())
                .map(str::to_string)
        })
        .collect()
}
