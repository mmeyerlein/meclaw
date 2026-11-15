//! GH #1526 (d58-3 grenzen, OR-583.grenzen.12) -- a set's override of
//! `keep_recent_share` and `window_target_share` passes the door.
//!
//! WHY this file exists: the two shares replace the token numbers a set used
//! to write (`keep_recent_tokens`, `window_target`). A set sets them on the
//! curator's instance (`curator/policy`, a declared param) or in the
//! `window_profile` of a model cell of its composite; the door
//! (`check_override_params`) lets an override through only for a declared
//! param, so both must be declared, shipped `null` (off: the bytes of
//! before), with a setting that says what they do. The token knobs stay
//! declared and shipped `null` as well -- the override for tests and
//! instances.

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
fn gh_d583_grenzen_a_set_override_of_the_shares_is_accepted() {
    if !shipped() {
        return;
    }
    let policy = cell_config("policy");
    for knob in [
        "keep_recent_share",
        "window_target_share",
        "keep_recent_tokens",
        "window_target",
    ] {
        assert_eq!(
            policy["params"][knob],
            Value::Null,
            "templates/curator/policy/config.json declares {knob} null"
        );
        let setting = &policy["contract"]["settings"][knob];
        assert_eq!(
            setting["default"],
            Value::Null,
            "the setting {knob} names the shipped default: {setting}"
        );
        assert!(
            setting["description"]
                .as_str()
                .is_some_and(|d| !d.is_empty()),
            "the setting {knob} says what it does: {setting}"
        );
    }
    let mut profile =
        read_json(&repo("templates/talky/brain/config.json"))["params"]["window_profile"].clone();
    profile["keep_recent_share"] = json!(0.25);
    profile["window_target_share"] = json!(0.5);
    let refusals: Vec<String> = [
        refused("curator", "policy", json!({"keep_recent_share": 0.25})),
        refused("curator", "policy", json!({"window_target_share": 0.5})),
        refused("talky", "brain", json!({"window_profile": profile})),
    ]
    .into_iter()
    .flatten()
    .collect();
    assert_eq!(refusals, Vec::<String>::new());
    assert!(
        refused(
            "curator",
            "policy",
            json!({"keep_recent_share_never_declared": 0.25})
        )
        .is_some(),
        "check of the setup: an undeclared param is refused"
    );
}
