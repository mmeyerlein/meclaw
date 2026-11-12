//! D58 feed (GH #1481, plan `u-feed.md` contract point 7) -- a set override
//! of `feed_in` passes the door.
//!
//! WHY this file exists: a knob a script reads is not yet a knob a set can
//! turn. The door (`check_override_params`) lets an override through only
//! for a param the cell declares; an undeclared one is refused when the set
//! is applied. `feed_in` is read by `curator/policy`, so it declares it,
//! shipped "0". talky is the composition that opens the lane: it hands
//! `in_feed` from its rim to its curator (`contract.accepts` and the edge
//! `. -> ./curator`) and sets the knob "1" on its curator's policy.

// harness_fold: #[path = "support/curator_feed.rs"]
#[allow(unused_imports)]
use crate::curator_feed; // harness_fold: mod curator_feed;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_feed::*;
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

#[test]
fn gh_d58_feed_a_set_override_of_feed_in_passes_the_door() {
    if !shipped() {
        return;
    }
    let parsed = parse_subtree(&core_root().join("templates/curator"), &shipped_registry())
        .unwrap_or_else(|e| panic!("the shipped curator template parses: {e:?}"));
    let cell = parsed
        .cells
        .iter()
        .find(|c| c.rel_path == "policy")
        .unwrap_or_else(|| panic!("the curator has a cell policy"));
    for value in ["1", "0"] {
        let over = json!({KNOB: value});
        if let Err(why) = check_override_params(cell, Some("policy"), "curator", &over) {
            panic!("a set's {over} on curator/policy must pass the door: {why:?}");
        }
    }
    let cfg = cell_config("policy");
    assert_eq!(
        cfg["params"][KNOB],
        json!("0"),
        "curator/policy ships it off"
    );
    let setting = &cfg["contract"]["settings"][KNOB];
    assert_eq!(setting["type"], json!("string"), "{setting}");
    assert_eq!(setting["default"], json!("0"), "{setting}");
    assert!(
        setting["description"]
            .as_str()
            .is_some_and(|d| !d.is_empty()),
        "the setting says what it does: {setting}"
    );

    // talky opens the lane: its curator's policy turns the knob on ...
    let over = read_json(&repo("templates/talky/curator/config.json"));
    assert_eq!(
        over["override_params"]["policy"][KNOB],
        json!("1"),
        "talky sets `{KNOB}` on its curator: {}",
        over["override_params"]
    );
    // ... and hands `in_feed` from its rim to the curator.
    let talky = read_json(&repo("templates/talky/config.json"));
    let accepts: Vec<Value> = talky["params"]["contract"]["accepts"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        accepts.iter().any(|l| l["route"] == json!(LANE)),
        "talky declares `{LANE}` in contract.accepts"
    );
    let edges = talky["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        edges.iter().any(|e| e["from"] == json!(".")
            && e["to"] == json!("./curator")
            && e["condition"]
                .as_str()
                .is_some_and(|c| c.contains(&format!("'{LANE}'")))),
        "talky's edge . -> ./curator carries `{LANE}`"
    );
}
