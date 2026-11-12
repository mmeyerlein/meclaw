//! GH #1473 (D58 sections) -- a set override of `sections` passes the door,
//! and the knobs it replaces are gone.
//!
//! WHY this file exists: a knob a script reads is not yet a knob a set can
//! turn. The door (`check_override_params`) lets an override through only for
//! a param the cell declares; an undeclared one is refused when the set is
//! applied. `sections` replaces `pass_sections` and `things_section` without
//! an alias, so those two (and `things_instruction`) must be refused now, and
//! the compositions that registered `memory` through `pass_sections` (talky,
//! simple-agent) register it through `sections`, on `sidecar` with its `topic`
//! mark, and their markers pass the door whole.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_colony::mutation::subtree::{
    check_override_params, check_ref_marker_overrides, parse_subtree,
};
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
fn a_set_override_of_sections_passes_the_door() {
    if !shipped() {
        return;
    }
    let parsed = parse_subtree(&core_root().join("templates/curator"), &shipped_registry())
        .unwrap_or_else(|e| panic!("the shipped curator template parses: {e:?}"));
    let cell = |name: &str| {
        parsed
            .cells
            .iter()
            .find(|c| c.rel_path == name)
            .unwrap_or_else(|| panic!("the curator has a cell {name}"))
    };
    let (intake, schemas) = (cell("intake"), cell("schemas"));

    let over = json!({"sections": {
        "memory": {"lane": "sidecar", "mark": "topic"},
        "things": {"lane": "thing_seen", "cap_share": 0.1}
    }});
    if let Err(why) = check_override_params(intake, Some("intake"), "curator", &over) {
        panic!("a set's {over} on curator/intake must pass the door: {why:?}");
    }

    // The setting is declared, shipped empty.
    let cfg = cell_config("intake");
    assert_eq!(cfg["params"]["sections"], json!({}), "shipped empty");
    let setting = &cfg["contract"]["settings"]["sections"];
    assert_eq!(setting["type"], json!("object"), "{setting}");
    assert_eq!(setting["default"], json!({}), "{setting}");
    assert!(
        setting["description"]
            .as_str()
            .is_some_and(|d| !d.is_empty()),
        "the setting says what it does: {setting}"
    );

    // The replaced knobs are gone: no alias.
    for (c, name, knob) in [
        (intake, "intake", "pass_sections"),
        (intake, "intake", "things_section"),
        (schemas, "schemas", "things_section"),
        (schemas, "schemas", "things_instruction"),
    ] {
        let over = json!({knob: "1"});
        assert!(
            check_override_params(c, Some(name), "curator", &over).is_err(),
            "curator/{name} `{knob}` is gone and refused at the door (GH #1473)"
        );
    }

    // The compositions register `memory` through `sections`.
    for marker in [
        "templates/talky/curator/config.json",
        "templates/simple-agent/curator/config.json",
    ] {
        let m = read_json(&repo(marker));
        let over = &m["override_params"];
        let refused = check_ref_marker_overrides(&parsed, "curator", over);
        assert!(refused.is_empty(), "{marker} passes the door: {refused:?}");
        assert!(
            over["intake"].get("pass_sections").is_none(),
            "{marker}: no pass_sections"
        );
        let memory = &over["intake"]["sections"]["memory"];
        assert_eq!(
            (&memory["lane"], &memory["mark"]),
            (&json!("sidecar"), &json!("topic")),
            "{marker} registers memory on sidecar with its topic mark"
        );
        // D58 followup (GH #1479): a talky's `memory` also registers the
        // class its `fact` marks keep -- the curator names none itself.
        let classes = if marker.starts_with("templates/talky/") {
            json!(["wellbeing"])
        } else {
            Value::Null
        };
        assert_eq!(memory["classes"], classes, "{marker}: {memory}");
    }
}
