//! D58 live (GH #1482, plan `u-live.md` contract point 5) -- a set override
//! of the strand's new params passes the door.
//!
//! WHY this file exists: a knob a script reads is not yet a knob a set can
//! turn. The door (`check_override_params`) lets an override through only
//! for a param the cell declares in its `params` block; an undeclared one
//! is refused when the set is applied, and the size of the start window
//! would stay 8192 in every colony while its tests ran green on a hand-set
//! hive. The strand's one new param is `seed_budget` of `curator/history`.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_colony::mutation::subtree::{check_override_params, parse_subtree};
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use meclaw_core::serde_json::json;

// The export audit (R2b) reads a bare path from the support helper as a
// reference to a tree the public subset lacks; the form of the sibling door
// pins (`gh_d38_k7_…`) names the shipped tree from the crate's manifest.
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
fn gh_d58_live_a_set_override_of_the_new_params_passes_the_door() {
    if !shipped() {
        return;
    }
    let parsed = parse_subtree(
        &core_root().join("templates").join("curator"),
        &shipped_registry(),
    )
    .unwrap_or_else(|e| panic!("the shipped curator template parses: {e:?}"));
    let cell = parsed
        .cells
        .into_iter()
        .find(|c| c.rel_path == "history")
        .expect("the curator has a history cell");
    for over in [json!({"seed_budget": 4096}), json!({"seed_budget": 8192})] {
        if let Err(why) = check_override_params(&cell, Some("history"), "curator", &over) {
            panic!("a set's {over} on curator/history must pass the door: {why:?}");
        }
    }
}
