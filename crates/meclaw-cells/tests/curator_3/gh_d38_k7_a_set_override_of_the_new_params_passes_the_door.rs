//! GH #1435 (D38 K7) -- a set's override of the three K7 params passes the door.
//!
//! WHY this file exists: a set may override a param only when the template
//! declares it under `params` (`check_override_params`, d53-3 K4 (1)); a knob
//! that lives in the script alone is not switchable from a set. The set that
//! turns the memory way on sends three overrides -- `talky/route` `needs` as
//! an object of four entries, `collector/assemble` `needs_wait_recall` "1",
//! `curator/policy` `memory_to_brain` "1" -- and every one must pass.
//!
//! Reads the shipped templates (`support/curator_hive.rs` for the paths).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_colony::mutation::subtree::{check_override_params, parse_subtree};
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use meclaw_core::serde_json::{Value, json};

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

#[test]
fn the_new_params_are_declared_with_their_shipped_defaults() {
    let route = read_json(&repo("templates/talky/route/config.json"));
    assert!(
        route["params"]["needs"].is_object(),
        "templates/talky/route/config.json declares needs as an object"
    );
    let assemble = read_json(&repo("templates/collector/assemble/config.json"));
    assert_eq!(
        assemble["params"]["needs_wait_recall"],
        json!("0"),
        "templates/collector/assemble/config.json declares needs_wait_recall \"0\""
    );
    let policy = cell_config("policy");
    assert_eq!(
        policy["params"]["memory_to_brain"],
        json!("0"),
        "templates/curator/policy/config.json declares memory_to_brain \"0\""
    );
}

#[test]
fn a_set_override_of_the_new_params_passes_the_door() {
    passes(
        "talky",
        "route",
        json!({"needs": {"tool": "a tool call",
                         "reasoning": "careful step-by-step reasoning",
                         "research": "a deep search over many sources",
                         "memory": "something the person said earlier or before today -- their people, their life, their past"}}),
    );
    passes("collector", "assemble", json!({"needs_wait_recall": "1"}));
    passes("curator", "policy", json!({"memory_to_brain": "1"}));
}
