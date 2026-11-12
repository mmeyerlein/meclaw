//! GH #1464 (D58 coldgc, contract point 1) -- a set override of `gc_on_cold`
//! passes the door.
//!
//! WHY this file exists: a knob a script reads is not yet a knob a set can
//! turn. The door (`check_override_params`) lets an override through only
//! for a param the cell declares in its `params` block; an undeclared one is
//! refused when the set is applied, and the knob would stay off in every
//! colony while its tests ran green on a hand-set hive. Declared, with its
//! preset "0" in the params and in the contract's settings, a set turns it.
//! No preset of a role sets it: which role collects on a cold cache is the
//! set's word alone. The same holds for `cold_to` (addendum 10.11.), the
//! share of the role's `window_target` a rebuild after an expired cache
//! aims at: declared empty -- the inherited aim of before --, no preset.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

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
fn the_knob_is_declared_with_its_shipped_default() {
    let policy = cell_config("policy");
    assert_eq!(
        policy["params"]["gc_on_cold"],
        json!("0"),
        "templates/curator/policy/config.json declares gc_on_cold, shipped \"0\""
    );
    let setting = &policy["contract"]["settings"]["gc_on_cold"];
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
    assert_eq!(
        policy["params"]["cold_to"],
        json!(""),
        "templates/curator/policy/config.json declares cold_to, shipped empty"
    );
    let setting = &policy["contract"]["settings"]["cold_to"];
    assert_eq!(
        setting["default"],
        json!(""),
        "the setting's default is the param"
    );
    assert_eq!(setting["type"], json!("string"), "{setting}");
    assert!(
        setting["description"]
            .as_str()
            .is_some_and(|d| !d.is_empty()),
        "the setting says what it does: {setting}"
    );
}

/// The knob is no value of any profile (R-58-15: the former presets of the
/// roles are profile values now): a profile cannot carry it and no example
/// set names it, so every target reads the shipped "0" until a set says
/// otherwise.
#[test]
fn no_role_presets_the_knob() {
    if !shipped() {
        return;
    }
    let (got, err) = policy_scope(
        json!({}),
        "sorted(k for k in ('gc_on_cold', 'cold_to') if k in ROLE_KEYS)",
        Value::Null,
    );
    assert!(!err.contains("Traceback"), "{err}");
    assert_eq!(got, json!([]), "no profile carries either knob: {got}");
    for name in curator_profiles::VALUE_SETS {
        let set = curator_profiles::value_set(name);
        assert!(
            set.get("gc_on_cold").is_none() && set.get("cold_to").is_none(),
            "{name}: {set}"
        );
    }
}

#[test]
fn gh_d58_cold_a_set_override_of_gc_on_cold_passes_the_door() {
    let parsed = parse_subtree(&core_root().join("templates/curator"), &shipped_registry())
        .unwrap_or_else(|e| panic!("the shipped curator template parses: {e:?}"));
    let cell = parsed
        .cells
        .into_iter()
        .find(|c| c.rel_path == "policy")
        .expect("the curator has a policy cell");
    for over in [
        json!({"gc_on_cold": "1"}),
        json!({"gc_on_cold": "0"}),
        json!({"cold_to": "0.45"}),
        json!({"cold_to": ""}),
    ] {
        if let Err(why) = check_override_params(&cell, Some("policy"), "curator", &over) {
            panic!("a set's {over} on curator/policy must pass the door: {why:?}");
        }
    }
}
