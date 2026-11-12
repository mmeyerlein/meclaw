//! D58 judge (GH #1480, contract points 1 and 7) -- a set's override of the
//! new params passes the door.
//!
//! WHY this file exists: a set may override a param only when the template
//! declares it under `params` (`check_override_params`, d53-3 K4 (1)). The
//! judges name the target of their view themselves, through the param
//! `view_target` of `talky/route` and `talky/screen_judge` (the curator holds
//! no built-in value per target, R-58-15), and a composition that runs its
//! judge on another model cell names that cell there. talky sets both to
//! `decide` -- one family for the judgement of the composite, where the
//! presenter's topics and the person's hints arrive (OR-d58-2.judge.13) --,
//! declared in the contract with that default, and a set's override of each
//! passes the door.
//!
//! The senders name the target of their contribution the same way (contract
//! point 8, OR-d58-2.judge.13): `part_target` of `presenter/stage` (the topic
//! table) and of `collector/assemble` (the person's hints on the curate),
//! each without a built-in value in the cell ("" = no contribution). The
//! composition `talky` sets its collector's to its judge's model cell
//! (`decide`); the presenter's is the set's, which wires the presenter.
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

/// `None` when the set's `over` on `template`/`cell` passes the door, else why not.
fn refused(template: &str, cell: &str, over: Value) -> Option<String> {
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
    check_override_params(&node, Some(cell), template, &over)
        .err()
        .map(|why| format!("a set's {over} on {template}/{cell} must pass the door: {why:?}"))
}

#[test]
fn gh_d58_judge_a_set_override_of_the_new_params_passes_the_door() {
    // The senders (contract point 8): no built-in target in the cell.
    for (template, cell) in [("presenter", "stage"), ("collector", "assemble")] {
        let cfg = read_json(&repo(&format!("templates/{template}/{cell}/config.json")));
        assert_eq!(
            cfg["params"]["part_target"],
            json!(""),
            "templates/{template}/{cell}/config.json declares part_target \"\""
        );
        let setting = &cfg["contract"]["settings"]["part_target"];
        assert_eq!(
            setting["default"],
            json!(""),
            "{template}/{cell}: {setting}"
        );
        assert!(
            setting["description"]
                .as_str()
                .is_some_and(|d| !d.is_empty()),
            "the setting part_target of {template}/{cell} says what it does: {setting}"
        );
        if let Some(why) = refused(template, cell, json!({"part_target": "decide_small"})) {
            panic!("{why}");
        }
    }
    let collector = read_json(&repo("templates/talky/collector/config.json"));
    assert_eq!(
        collector["override_params"]["assemble"]["part_target"],
        json!("decide"),
        "talky's collector hands the hints to the view of its judge: {collector}"
    );
    for (cell, default) in [("route", "decide"), ("screen_judge", "decide")] {
        let cfg = read_json(&repo(&format!("templates/talky/{cell}/config.json")));
        assert_eq!(
            cfg["params"]["view_target"],
            json!(default),
            "templates/talky/{cell}/config.json declares view_target {default:?}"
        );
        let setting = &cfg["contract"]["settings"]["view_target"];
        assert_eq!(setting["default"], json!(default), "{cell}: {setting}");
        assert!(
            setting["description"]
                .as_str()
                .is_some_and(|d| !d.is_empty()),
            "the setting view_target of {cell} says what it does: {setting}"
        );
    }
    let refusals: Vec<String> = [
        refused("talky", "route", json!({"view_target": "decide_small"})),
        refused(
            "talky",
            "screen_judge",
            json!({"view_target": "screen_small"}),
        ),
    ]
    .into_iter()
    .flatten()
    .collect();
    assert_eq!(refusals, Vec::<String>::new());
    // The control: the door does refuse a param no template declares.
    assert!(
        refused("talky", "route", json!({"view_never_declared": "1"})).is_some(),
        "check of the setup: an undeclared param is refused"
    );
}
