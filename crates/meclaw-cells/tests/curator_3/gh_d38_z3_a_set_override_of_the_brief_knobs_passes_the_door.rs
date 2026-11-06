//! GH #1440 (D38 Z3) -- the three knobs of the brief are declared, ship off
//! and pass a set's override.
//!
//! WHY this file exists: a set may override a param only when the template
//! declares it under `params` (`check_override_params`, d53-3 K4 (1)); a knob
//! that lives in the script alone cannot be switched from a set, and S10/S6
//! switch these from Satz 0.56. `summary_form` ships "prose" (the prose
//! summary of before), `summary_state_cond` and `summarizer_self` ship "0".
//! The script reads them the way every flat switch is read (`switch_on`), and
//! `summary_form` knows its two values only: anything else is the default.
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

const KNOBS: [(&str, &str); 3] = [
    ("summary_form", "prose"),
    ("summary_state_cond", "0"),
    ("summarizer_self", "0"),
];

#[test]
fn the_knobs_are_declared_with_their_shipped_defaults() {
    if !shipped() {
        return;
    }
    let policy = cell_config("policy");
    for (knob, off) in KNOBS {
        assert_eq!(policy["params"][knob], json!(off), "{knob} ships {off:?}");
        let s = &policy["contract"]["settings"][knob];
        assert_eq!(s["default"], json!(off), "{knob}");
        assert_eq!(s["type"], json!("string"), "{knob}");
        assert!(
            s["description"]
                .as_str()
                .is_some_and(|d| d.contains("#1440")),
            "{knob}: the setting names its issue"
        );
    }
}

#[test]
fn the_script_reads_the_knobs() {
    if !shipped() {
        return;
    }
    for (over, want) in [
        (json!({}), json!(["prose", false, false])),
        (
            json!({"summary_form": "brief", "summary_state_cond": "1", "summarizer_self": "1"}),
            json!(["brief", true, true]),
        ),
        (
            json!({"summary_form": "chronicle"}),
            json!(["prose", false, false]),
        ),
        (
            json!({"summary_state_cond": true, "summarizer_self": "on"}),
            json!(["prose", true, true]),
        ),
    ] {
        let (got, err) = policy_scope(
            over.clone(),
            "[SUMMARY_FORM, SUMMARY_STATE_COND, SUMMARIZER_SELF]",
            Value::Null,
        );
        assert_eq!(got, want, "{over}: {err}");
    }
}

#[test]
fn a_set_override_of_the_brief_knobs_passes_the_door() {
    if !shipped() {
        return;
    }
    let parsed = parse_subtree(&core_root().join("templates/curator"), &shipped_registry())
        .unwrap_or_else(|e| panic!("the shipped curator template parses: {e:?}"));
    let cell = parsed
        .cells
        .into_iter()
        .find(|c| c.rel_path == "policy")
        .expect("the curator has a policy cell");
    let over = json!({"summary_form": "brief", "summary_state_cond": "1",
                      "summarizer_self": "1"});
    if let Err(why) = check_override_params(&cell, Some("policy"), "curator", &over) {
        panic!("a set's {over} on curator/policy must pass the door: {why:?}");
    }
}
