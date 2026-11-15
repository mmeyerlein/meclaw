//! D58-3 kern (GH #1524, contract point 1, K.8) -- a set override of
//! `profile_fallback` is taken.
//!
//! WHY this file exists: a param a script reads is not yet a param a set can
//! turn. The door (`check_override_params`) lets an override through only for
//! a param the cell declares. `curator/policy` ships `profile_fallback` as
//! `{}` -- no fallback, the curator of before -- and declares it with a word
//! on what it does, so that a set that renders the profiles of its targets
//! into it passes the door.
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

fn door(template: &str, cell: &str, over: &Value) -> Result<(), String> {
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
    check_override_params(&node, Some(cell), template, over).map_err(|e| format!("{e:?}"))
}

#[test]
fn gh_d583_kern_a_set_override_of_profile_fallback_is_taken() {
    if !shipped() {
        return;
    }
    let policy = cell_config("policy");
    assert_eq!(
        policy["params"]["profile_fallback"],
        json!({}),
        "`profile_fallback` ships empty: the curator of before, byte for byte"
    );
    let setting = &policy["contract"]["settings"]["profile_fallback"];
    assert!(
        setting["description"]
            .as_str()
            .is_some_and(|d| !d.is_empty()),
        "curator/policy declares `profile_fallback` and says what it does: {setting}"
    );
    let over = json!({"profile_fallback": {
        "spec": {"window": "wall", "system": "full", "tools": "all",
                 "instructions": {"slot": "spec", "text": "", "only": true}}}});
    if let Err(why) = door("curator", "policy", &over) {
        panic!("a set's {over} on curator/policy must pass the door: {why}");
    }
}
