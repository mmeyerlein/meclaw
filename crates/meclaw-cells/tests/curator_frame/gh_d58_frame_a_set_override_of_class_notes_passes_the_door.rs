//! D58 frame (GH #1478, note per stamp class point 1) -- a set override of
//! `class_notes` passes the door.
//!
//! WHY this file exists: a param a cell reads is not yet a param a set can
//! turn. The door (`check_override_params`) lets an override through only
//! for a param the cell declares in `params` and `contract.settings`. The
//! curator's policy declares `class_notes`, shipped empty (the curator names
//! no class of its own), and a set's entry for a class passes. The knob
//! `clarify_note` fell with the fixed class: it is declared nowhere, and a
//! set that still sends it is refused at the door instead of being ignored.
//!
//! Reads the shipped templates (`support/curator_hive.rs` for the paths).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_colony::mutation::subtree::{check_override_params, parse_subtree};
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use meclaw_core::serde_json::{Value, json};

fn shipped_registry() -> TemplatesRegistry {
    let scanned = scan_templates_dir(&repo("templates")).unwrap_or_default();
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

/// `None` when the set's `over` on the curator's policy passes the door,
/// else why not.
fn refused(over: Value) -> Option<String> {
    let parsed = parse_subtree(&repo("templates/curator"), &shipped_registry())
        .unwrap_or_else(|e| panic!("the shipped curator template parses: {e:?}"));
    let node = parsed
        .cells
        .into_iter()
        .find(|c| c.rel_path == "policy")
        .unwrap_or_else(|| panic!("the curator has a policy cell"));
    check_override_params(&node, Some("policy"), "curator", &over)
        .err()
        .map(|why| format!("{why:?}"))
}

#[test]
fn gh_d58_frame_a_set_override_of_class_notes_passes_the_door() {
    if !shipped() {
        return;
    }
    let mut failures = Vec::new();
    let policy = cell_config("policy");
    if policy["params"]["class_notes"] != json!({}) {
        failures.push(format!(
            "the policy ships `class_notes` empty: {}",
            policy["params"]["class_notes"]
        ));
    }
    let setting = &policy["contract"]["settings"]["class_notes"];
    if setting["default"] != json!({}) {
        failures.push(format!(
            "the setting `class_notes` names the shipped default: {setting}"
        ));
    }
    if !setting["description"]
        .as_str()
        .is_some_and(|d| !d.is_empty())
    {
        failures.push(format!(
            "the setting `class_notes` says what it does: {setting}"
        ));
    }
    for (place, v) in [
        ("params", &policy["params"]["clarify_note"]),
        (
            "contract.settings",
            &policy["contract"]["settings"]["clarify_note"],
        ),
    ] {
        if !v.is_null() {
            failures.push(format!("`clarify_note` is still declared in {place}"));
        }
    }
    for over in [
        json!({"class_notes": {"clarify": {"text": "Ask one question back.", "tools": "none",
                                           "pair": "mx.hints.clarify"}}}),
        json!({"class_notes": {}}),
    ] {
        if let Some(why) = refused(over.clone()) {
            failures.push(format!("a set's {over} must pass the door: {why}"));
        }
    }
    if refused(json!({"clarify_note": "Ask one question back."})).is_none() {
        failures.push("a set's `clarify_note` passes the door after the knob fell".into());
    }
    assert!(
        refused(json!({"class_notes_never_declared": {}})).is_some(),
        "check of the setup: an undeclared param is refused"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
