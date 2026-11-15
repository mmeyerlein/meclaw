//! d58-3 tag (GH #1527) -- a set override of `day_floor` passes the door.
//!
//! WHY this file exists: a switch a script reads is not yet a switch a set
//! can turn. The door (`check_override_params`) lets an override through
//! only for a param the cell declares in its `params` block; an undeclared
//! one is refused when the set is applied, and the floor would stay off in
//! every colony while its tests ran green on a hand-set hive (the kit's
//! duty pin for every new param). Declared, with its preset "0" in the
//! params and in the contract's settings, a set turns it, both ways.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_colony::mutation::subtree::{check_override_params, parse_subtree};
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use meclaw_core::serde_json::json;

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

#[test]
fn a_set_override_of_day_floor_is_taken() {
    let policy = cell_config("policy");
    assert_eq!(
        policy["params"]["day_floor"],
        json!("0"),
        "templates/curator/policy/config.json declares day_floor, shipped \"0\""
    );
    let setting = &policy["contract"]["settings"]["day_floor"];
    assert_eq!(
        setting["default"],
        json!("0"),
        "the setting's default is the param"
    );
    assert_eq!(setting["type"], json!("string"), "{setting}");
    assert!(
        setting["description"]
            .as_str()
            .is_some_and(|d| !d.is_empty()),
        "the setting says what it does: {setting}"
    );
    let parsed = parse_subtree(&core_root().join("templates/curator"), &shipped_registry())
        .unwrap_or_else(|e| panic!("the shipped curator template parses: {e:?}"));
    let cell = parsed
        .cells
        .into_iter()
        .find(|c| c.rel_path == "policy")
        .expect("the curator has a policy cell");
    for over in [json!({"day_floor": "1"}), json!({"day_floor": "0"})] {
        if let Err(why) = check_override_params(&cell, Some("policy"), "curator", &over) {
            panic!("a set's {over} on curator/policy must pass the door: {why:?}");
        }
    }
}
