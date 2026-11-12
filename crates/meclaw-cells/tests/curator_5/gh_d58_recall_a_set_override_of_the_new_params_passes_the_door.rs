//! GH #1466 (D58 recall) -- a set's override of the new params passes the door.
//!
//! WHY this file exists: a set may override a param only when the template
//! declares it under `params` (`check_override_params`, d53-3 K4 (1)); a knob
//! that lives in the script alone is not switchable from a set. The rule
//! ships off and is turned on by the set whose recall bench measured the
//! bound, and that set sends three overrides: `curator/policy`
//! `recall_skip_sure` "1" and `recall_skip_max_yes` (a number), and
//! `talky/route` `stamp_needs_yes` "1" (the stamp then carries the yes-values
//! the rule reads). Every one must pass, and every one ships with the default
//! that leaves the bytes of before.
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
fn gh_d58_recall_a_set_override_of_the_new_params_passes_the_door() {
    // Declared, with the defaults that leave the bytes of before.
    let policy = cell_config("policy");
    assert_eq!(
        policy["params"]["recall_skip_sure"],
        json!("0"),
        "templates/curator/policy/config.json declares recall_skip_sure \"0\""
    );
    assert_eq!(
        policy["params"]["recall_skip_max_yes"],
        json!(0.2),
        "templates/curator/policy/config.json declares recall_skip_max_yes 0.2"
    );
    for knob in ["recall_skip_sure", "recall_skip_max_yes"] {
        let setting = &policy["contract"]["settings"][knob];
        assert_eq!(
            setting["default"], policy["params"][knob],
            "the setting {knob} names the shipped default: {setting}"
        );
        assert!(
            setting["description"]
                .as_str()
                .is_some_and(|d| !d.is_empty()),
            "the setting {knob} says what it does: {setting}"
        );
    }
    let route = read_json(&repo("templates/talky/route/config.json"));
    assert_eq!(
        route["params"]["stamp_needs_yes"],
        json!("0"),
        "templates/talky/route/config.json declares stamp_needs_yes \"0\""
    );
    assert_eq!(
        route["contract"]["settings"]["stamp_needs_yes"]["default"],
        json!("0"),
        "the route's setting says the same"
    );

    // And a set's override of each passes the door; all three are checked
    // before the verdict, so one refusal does not hide the next.
    let refusals: Vec<String> = [
        refused("curator", "policy", json!({"recall_skip_sure": "1"})),
        refused("curator", "policy", json!({"recall_skip_max_yes": 0.35})),
        refused("talky", "route", json!({"stamp_needs_yes": "1"})),
    ]
    .into_iter()
    .flatten()
    .collect();
    assert_eq!(refusals, Vec::<String>::new());
    // The control: the door does refuse a param no template declares.
    assert!(
        refused(
            "curator",
            "policy",
            json!({"recall_skip_never_declared": "1"})
        )
        .is_some(),
        "check of the setup: an undeclared param is refused"
    );
}
