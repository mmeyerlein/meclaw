//! D58 note (GH #1477, contract point 8) -- a set override of `note_once`
//! passes the door on the three cells that read it.
//!
//! WHY this file exists: a knob a script reads is not yet a knob a set can
//! turn. The door (`check_override_params`) lets an override through only for
//! a param the cell declares; an undeclared one is refused when the set is
//! applied. `note_once` is read by `curator/intake` (the scope and the
//! column), `curator/policy` (the window and the receipt) and
//! `curator/history` (the start window), so each declares it, shipped "0".

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/note_once.rs"]
#[allow(unused_imports)]
use crate::note_once; // harness_fold: mod note_once;

use curator_hive::*;
use meclaw_colony::mutation::subtree::{check_override_params, parse_subtree};
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use meclaw_core::serde_json::json;
use note_once::*;

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
fn a_set_override_of_note_once_passes_the_door() {
    if !shipped() {
        return;
    }
    let parsed = parse_subtree(&core_root().join("templates/curator"), &shipped_registry())
        .unwrap_or_else(|e| panic!("the shipped curator template parses: {e:?}"));
    for name in KNOB_CELLS {
        let cell = parsed
            .cells
            .iter()
            .find(|c| c.rel_path == name)
            .unwrap_or_else(|| panic!("the curator has a cell {name}"));
        for value in ["1", "0"] {
            let over = json!({KNOB: value});
            if let Err(why) = check_override_params(cell, Some(name), "curator", &over) {
                panic!("a set's {over} on curator/{name} must pass the door: {why:?}");
            }
        }
        let cfg = cell_config(name);
        assert_eq!(
            cfg["params"][KNOB],
            json!("0"),
            "curator/{name} ships it off"
        );
        let setting = &cfg["contract"]["settings"][KNOB];
        assert_eq!(setting["type"], json!("string"), "{name}: {setting}");
        assert_eq!(setting["default"], json!("0"), "{name}: {setting}");
        assert!(
            setting["description"]
                .as_str()
                .is_some_and(|d| !d.is_empty()),
            "{name}: the setting says what it does: {setting}"
        );
    }
}
