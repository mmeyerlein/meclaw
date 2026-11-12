//! D58 lead (GH #1474, contract points 3 and 5) -- a set's override of the
//! lead params passes the door.
//!
//! WHY this file exists: a set may override a param only where the template
//! declares it under `params` (`check_override_params`, d53-3 K4 (1)). The
//! knobs of the switch move from `curator/lead` to the template `lead`, and
//! a set that renders them for a composite must still reach every one of
//! them at their new place; the curator's new knob that says a lead is
//! wired must be switchable from a set as well.
//!
//! Reads the shipped templates (`support/curator_hive.rs` for the paths).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_lead.rs"]
#[allow(unused_imports)]
use crate::curator_lead; // harness_fold: mod curator_lead;

use curator_hive::*;
use curator_lead::*;
use meclaw_colony::mutation::subtree::{check_override_params, parse_subtree};
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use meclaw_core::serde_json::{Map, Value, json};

fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn shipped_registry() -> TemplatesRegistry {
    let scanned = scan_templates_dir(&core_root().join("templates")).unwrap_or_default();
    TemplatesRegistry::from_entries(
        scanned
            .into_iter()
            .map(|s| TemplateEntry {
                template_id: format!("scan-{}", s.name),
                name: s.name,
                version: s.version,
                filesystem_path: s.filesystem_path,
            })
            .collect(),
    )
}

fn passes(template: &str, cell: &str, over: Value) {
    let parsed = parse_subtree(
        &core_root().join("templates").join(template),
        &shipped_registry(),
    )
    .unwrap_or_else(|e| panic!("the shipped {template} template parses: {e:?}"));
    let node = parsed
        .cells
        .into_iter()
        .find(|c| c.rel_path == cell)
        .unwrap_or_else(|| panic!("{template} has a {cell} cell"));
    if let Err(why) = check_override_params(&node, Some(cell), template, &over) {
        panic!("a set's {over} on {template}/{cell} must pass the door: {why:?}");
    }
}

/// A value a set would render for a knob of the lead: talky's for a knob
/// that names targets, else the shipped one.
fn set_value(knob: &str, shipped: &Value) -> Value {
    match knob {
        "path_routes" => json!("talk:brain,simple:quick,*:brain"),
        "departures" => json!("consult:10000"),
        "fallback" => json!("quick:brain"),
        "spec_target" => json!("quick_spec"),
        "bridge_target" => json!("*:quick"),
        _ => shipped.clone(),
    }
}

#[test]
fn gh_d58_lead_a_set_override_of_the_lead_params_passes_the_door() {
    if !shipped() {
        return;
    }
    let lead = lead_hive_cell("lead");
    let mut all = Map::new();
    for knob in CHOICE_KNOBS {
        let declared = lead["params"]
            .get(knob)
            .unwrap_or_else(|| panic!("{LEAD_TEMPLATE}/lead declares `{knob}`"));
        let value = set_value(knob, declared);
        passes("lead", "lead", json!({ knob: value.clone() }));
        all.insert(knob.to_string(), value);
    }
    // All of them at once, as a set renders them.
    passes("lead", "lead", Value::Object(all));
    // The curator's knob that says a lead is wired.
    passes("curator", "policy", json!({ LEAD_KNOB: "1" }));
}
