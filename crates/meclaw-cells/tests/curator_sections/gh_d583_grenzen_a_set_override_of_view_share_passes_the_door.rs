//! GH #1526 (d58-3 grenzen, N6) -- a set override of `view_share` passes the
//! door on `curator/history`, the one cell that reads it.
//!
//! WHY this file exists: a knob a script reads is not yet a knob a set can
//! turn. The door (`check_override_params`) lets an override through only for
//! a param the cell declares; an undeclared one is refused when the set is
//! applied. `view_share` sizes every view `./history` answers (a share of the
//! asked target's window), shipped 0.1875 = 1 500 / 8 000: the 1 500 tokens
//! the shipped judge's view had, of the 8 000 its shipped model's row holds.

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
fn a_set_override_of_view_share_passes_the_door() {
    if !shipped() {
        return;
    }
    let parsed = parse_subtree(&core_root().join("templates/curator"), &shipped_registry())
        .unwrap_or_else(|e| panic!("the shipped curator template parses: {e:?}"));
    let cell = parsed
        .cells
        .iter()
        .find(|c| c.rel_path == "history")
        .unwrap_or_else(|| panic!("the curator has a cell history"));
    for value in [json!(0.25), json!(0.1875)] {
        let over = json!({"view_share": value});
        if let Err(why) = check_override_params(cell, Some("history"), "curator", &over) {
            panic!("a set's {over} on curator/history must pass the door: {why:?}");
        }
    }
    let cfg = cell_config("history");
    assert_eq!(
        cfg["params"]["view_share"],
        json!(0.1875),
        "shipped 1500 / 8000"
    );
    let setting = &cfg["contract"]["settings"]["view_share"];
    assert_eq!(setting["type"], json!("number"), "{setting}");
    assert_eq!(setting["default"], json!(0.1875), "{setting}");
    assert!(
        setting["description"]
            .as_str()
            .is_some_and(|d| !d.is_empty()),
        "the setting says what it does: {setting}"
    );
}
