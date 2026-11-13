//! GH #1430 (D38 K3) -- the collection's knobs: declared, preset, switchable.
//!
//! WHY this file exists: a set may override a knob of the policy only when
//! the template declares it under `params` (d53-3 K4, `check_override_params`)
//! -- a knob that lives in `ROLES` alone is not switchable from a set. So
//! `gc` and `gc_scaffold` stand in the `params` block (null = the preset),
//! and the presets are: `gc` "1" for talky alone, "0" for every other role
//! (byte for byte the behaviour of before); `gc_scaffold` "1" everywhere (the
//! set's brake of A4); `keep_rounds` 1 for talky (was 2; R-38-2, RK-38-2),
//! every other role as it was. The constants resolve like every knob.
//!
//! Runs the pure half of the shipped policy (`support/curator_hive.rs`).

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

/// Both knobs are declared on the policy cell with a null default, so a
/// set's override of them passes the mutation door.
#[test]
fn the_knobs_are_declared_and_a_set_override_passes_the_door() {
    let params = cell_config("policy")["params"].clone();
    for k in ["gc", "gc_scaffold"] {
        assert!(
            params.as_object().unwrap().contains_key(k),
            "templates/curator/policy/config.json declares {k}"
        );
        assert_eq!(params[k], Value::Null, "{k}: null = the preset");
    }
    let parsed = parse_subtree(&core_root().join("templates/curator"), &shipped_registry())
        .unwrap_or_else(|e| panic!("the shipped curator template parses: {e:?}"));
    let cell = parsed
        .cells
        .into_iter()
        .find(|c| c.rel_path == "policy")
        .expect("the curator has a policy cell");
    if let Err(why) = check_override_params(
        &cell,
        Some("policy"),
        "curator",
        &json!({"gc": "1", "gc_scaffold": "0"}),
    ) {
        panic!("a set's gc/gc_scaffold on curator/policy must pass the door: {why:?}");
    }
}

/// The values per example set (the former presets per role, R-58-15: they
/// live in the profiles now), as the curator reads them from a profile, and
/// `keep_rounds` 1 for talky alone.
#[test]
fn talky_collects_with_keep_rounds_1_and_the_other_roles_stay() {
    let (got, err) = policy_scope(
        json!({}),
        "dict((r, [knob_of(profile_params(p, bool(p)), k) \
         for k in ('gc', 'gc_scaffold', 'keep_rounds')]) for r, p in ARGS.items())",
        curator_profiles::profiles_by_set(),
    );
    assert_eq!(
        got,
        json!({"": ["0", "1", 0], "talky": ["1", "1", 1], "consult": ["0", "1", 2],
               "coding": ["0", "1", 3], "research": ["0", "1", 1], "quick": ["0", "1", 1]}),
        "{err}"
    );
}

/// The constants the knobs resolve to: the preset, an instance's own switch
/// (JSON true, "1", "true", "on" are on -- the rule of every switch), null
/// for the preset.
#[test]
fn the_switches_resolve_like_every_switch() {
    for (over, want_gc, want_scaffold, want_rounds) in [
        (
            curator_profiles::set_knobs("talky", json!({})),
            true,
            true,
            1,
        ),
        (
            curator_profiles::set_knobs("talky", json!({"gc": null})),
            true,
            true,
            1,
        ),
        (
            curator_profiles::set_knobs("talky", json!({"gc": "0"})),
            false,
            true,
            1,
        ),
        (
            curator_profiles::set_knobs("talky", json!({"gc_scaffold": "0"})),
            true,
            false,
            1,
        ),
        (
            curator_profiles::set_knobs("talky", json!({"gc_scaffold": false})),
            true,
            false,
            1,
        ),
        (
            curator_profiles::set_knobs("talky", json!({"keep_rounds": 3})),
            true,
            true,
            3,
        ),
        (
            curator_profiles::set_knobs("consult", json!({})),
            false,
            true,
            2,
        ),
        (
            curator_profiles::set_knobs("consult", json!({"gc": true})),
            true,
            true,
            2,
        ),
        (
            curator_profiles::set_knobs("consult", json!({"gc": "on"})),
            true,
            true,
            2,
        ),
        (json!({}), false, true, 0),
    ] {
        let (got, err) = policy_scope(over.clone(), "[GC, GC_SCAFFOLD, KEEP_ROUNDS]", Value::Null);
        assert_eq!(
            got,
            json!([want_gc, want_scaffold, want_rounds]),
            "{over}: {err}"
        );
    }
}
