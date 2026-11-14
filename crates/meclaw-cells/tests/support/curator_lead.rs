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

// ack
// D54g ack (GH #1504): the three knobs this strand gives the lead and the
// lead's pure decision under them, written once here for the eight pins of
// `gh_d54g_ack_lead`.

/// The knob that lists the conditions the lead orders a side call for
/// (contract point 1): `must_call` and `external`, shipped both.
pub const SIDE_WHEN: &str = "side_when";

/// The knob that says what the turn of a running session gets (contract
/// point 4): `call` (shipped, as before) or `listen`.
pub const LIVE_TURN: &str = "live_turn";

/// The knob that names the target of an event round by its kind (contract
/// point 6): `kind:target` pairs, shipped empty.
pub const EVENT_ROUTES: &str = "event_routes";

/// The handle of the running session the pins' turns belong to.
pub const ACK_LIVE: &str = "call-live-9";

/// The round key of an event round (`<label>~<deadline_ms>~<hex8>`).
pub const ACK_EVENT_TURN: &str = "voice-turn-1~4102444800000~ab12cd34";

/// The keys of a verdict as the lead gives it before this strand: with
/// fresh knobs it gains none (contract point 8).
pub const ACK_VERDICT_KEYS: [&str; 16] = [
    "deny",
    "departs",
    "escalate",
    "fallback",
    "feed",
    "from",
    "pending",
    "pending_end",
    "reason",
    "side",
    "speak_check",
    "spec",
    "spec_held",
    "spec_row",
    "target",
    "wait",
];

/// The shipped value of a knob of the lead: declared under `params` and
/// under `contract.settings`, with the same default at both places.
pub fn ack_shipped(knob: &str) -> Value {
    let lead = lead_hive_cell("lead");
    let declared = lead["params"]
        .get(knob)
        .unwrap_or_else(|| panic!("{LEAD_TEMPLATE}/lead declares `{knob}` under params"))
        .clone();
    let setting = &lead["contract"]["settings"][knob];
    assert!(
        setting.is_object(),
        "{LEAD_TEMPLATE}/lead declares `{knob}` under contract.settings"
    );
    assert_eq!(
        setting["default"], declared,
        "`{knob}`: the default of contract.settings is the value of params"
    );
    declared
}

/// The names of a list knob: a comma separated string or a JSON list.
pub fn ack_names(v: &Value) -> Vec<String> {
    let mut out: Vec<String> = match v {
        Value::String(s) => s
            .split(',')
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .collect(),
        Value::Array(a) => a
            .iter()
            .filter_map(|n| n.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    };
    out.sort();
    out
}

/// talky's wiring of the lead (`LEAD_WIRING`) with a target for every
/// other turn (`*:brain`, as a set renders it), `extra` on top.
pub fn ack_knobs(extra: Value) -> Value {
    let mut k = meclaw_core::serde_json::Map::new();
    for (key, value) in LEAD_WIRING {
        k.insert(key.to_string(), json!(value));
    }
    k.insert(
        "path_routes".to_string(),
        json!("talk:brain,simple:quick,complex:consult,research:research,*:brain"),
    );
    for (key, value) in obj(extra) {
        k.insert(key, value);
    }
    Value::Object(k)
}

/// The head of a call's question to the lead: a first iteration, no handle,
/// no speculation, no event; `extra` on top.
pub fn ack_head(extra: Value) -> Value {
    let mut h = json!({"channel": "test", "live": "", "session": "s1", "turn": "t1",
                       "call": "c-t1", "iter": "0", "consult_id": "", "consult_class": "",
                       "spec": "", "escalate": "", "at": "2026-11-12T10:00:00.000000Z"});
    for (k, v) in obj(extra) {
        h[k.as_str()] = v;
    }
    h
}

/// A route stamp.
pub fn ack_stamp(path: &str, needs: &[&str], families: &[&str]) -> Value {
    json!({"path": path, "needs": needs, "families": families})
}

/// `lead_verdict` of the shipped lead under `knobs`, for a session that has
/// decided nothing yet.
pub fn ack_verdict(knobs: Value, stamp: Value, head: Value) -> Value {
    assert!(lead_shipped(), "the lead cell is shipped");
    let (v, err) = cell_scope(
        "lead",
        knobs,
        "lead_verdict(ARGS[0], ARGS[1], ARGS[2])",
        json!([stamp, head,
               {"escalated": "", "consult_pending": {}, "spec": {}, "spec_held": {}}]),
    );
    assert!(v.is_object(), "a verdict: {v} {err}");
    v
}

/// The entries of a verdict's `side` under the condition `when`.
pub fn ack_sides(v: &Value, when: &str) -> Vec<Value> {
    v["side"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e["when"] == json!(when))
        .collect()
}

/// A verdict without the field `key`.
pub fn ack_without(v: &Value, key: &str) -> Value {
    let mut o = obj(v.clone());
    o.remove(key);
    Value::Object(o)
}

/// The keys of a verdict, sorted.
pub fn ack_keys(v: &Value) -> Vec<String> {
    let mut k: Vec<String> = obj(v.clone()).keys().cloned().collect();
    k.sort();
    k
}
