//! GH #1439 (D38 Z1) -- the state block's knobs: declared, preset, switchable.
//!
//! WHY this file exists: a set may override a param only when the template
//! declares it under `params` (`check_override_params`, d53-3 K4 (1)); a knob
//! that lives in the script or in `ROLES` alone is not switchable from a set.
//! The set that turns the block on sends `state_block` "1" and may move
//! `state_lines`; both must pass. `state_block` ships "0" (the window of
//! before), `state_lines` ships null = the preset of the role: talky 8, every
//! other role 0 (R-IG-1: the number is a knob with a preset).
//!
//! Reads the shipped templates (`support/curator_hive.rs` for the paths).

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
fn the_knobs_are_declared_with_their_shipped_defaults() {
    let policy = cell_config("policy");
    assert_eq!(
        policy["params"]["state_block"],
        json!("0"),
        "state_block ships \"0\""
    );
    assert!(
        policy["params"]
            .as_object()
            .expect("params")
            .contains_key("state_lines"),
        "templates/curator/policy/config.json declares state_lines"
    );
    assert_eq!(
        policy["params"]["state_lines"],
        Value::Null,
        "null = the preset"
    );
    let settings = &policy["contract"]["settings"];
    assert_eq!(settings["state_block"]["default"], json!("0"));
    assert_eq!(settings["state_lines"]["default"], Value::Null);
}

#[test]
fn state_lines_is_preset_per_role() {
    // R-58-15: the values per example set live in the profiles; read as the
    // curator reads a profile.
    let (got, err) = policy_scope(
        json!({}),
        "dict((r, knob_of(profile_params(p, bool(p)), 'state_lines')) for r, p in ARGS.items())",
        curator_profiles::profiles_by_set(),
    );
    assert_eq!(
        got,
        json!({"": 0, "talky": 8, "consult": 0, "coding": 0, "research": 0, "quick": 0}),
        "{err}"
    );
    for (over, want_on, want_lines) in [
        (curator_profiles::set_knobs("talky", json!({})), false, 8),
        (
            curator_profiles::set_knobs("talky", json!({"state_block": "1"})),
            true,
            8,
        ),
        (
            curator_profiles::set_knobs("talky", json!({"state_block": true})),
            true,
            8,
        ),
        (
            curator_profiles::set_knobs("talky", json!({"state_block": "1", "state_lines": 3})),
            true,
            3,
        ),
        (
            curator_profiles::set_knobs("talky", json!({"state_lines": null})),
            false,
            8,
        ),
        (
            curator_profiles::set_knobs("consult", json!({"state_block": "on"})),
            true,
            0,
        ),
    ] {
        let (got, err) = policy_scope(over.clone(), "[STATE_BLOCK, STATE_LINES]", Value::Null);
        assert_eq!(got, json!([want_on, want_lines]), "{over}: {err}");
    }
}

#[test]
fn a_set_override_of_state_block_passes_the_door() {
    let parsed = parse_subtree(&core_root().join("templates/curator"), &shipped_registry())
        .unwrap_or_else(|e| panic!("the shipped curator template parses: {e:?}"));
    let cell = parsed
        .cells
        .into_iter()
        .find(|c| c.rel_path == "policy")
        .expect("the curator has a policy cell");
    let over = json!({"state_block": "1", "state_lines": 6});
    if let Err(why) = check_override_params(&cell, Some("policy"), "curator", &over) {
        panic!("a set's {over} on curator/policy must pass the door: {why:?}");
    }
}
