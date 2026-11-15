//! d58-3 namen (GH #1525, contract points 1, 2 and 5) -- a set override of
//! the new params passes the door.
//!
//! WHY this file exists: a knob a script reads is not yet a knob a set can
//! turn. The door (`check_override_params`) lets an override through only
//! for a param the cell declares (kit trap of the stage). The names the
//! curator and the lead lost come back as knobs -- `recall_needs` at the
//! policy, the lead and the collector's `assemble`, `state_needs` and
//! `pending_note` at the policy -- shipped neutral (`[]`, the neutral note),
//! so the compositions and the sets of soul2me must be able to set them.
//!
//! Parses the shipped templates (`meclaw_colony::mutation::subtree`); no
//! model.

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

fn door(template: &str, cell: &str, over: &Value) -> Result<(), String> {
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
    check_override_params(&node, Some(cell), template, over).map_err(|e| format!("{e:?}"))
}

/// `(template, cell, knob, shipped default, a value a set renders)`.
fn new_params() -> Vec<(&'static str, &'static str, &'static str, Value, Value)> {
    vec![
        (
            "curator",
            "policy",
            "recall_needs",
            json!([]),
            json!(["memory"]),
        ),
        (
            "curator",
            "policy",
            "state_needs",
            json!([]),
            json!(["memory", "orga"]),
        ),
        (
            "curator",
            "policy",
            "pending_note",
            json!("An answer from another model is pending; do not ask for it again."),
            json!("An answer is on its way."),
        ),
        // Bauer 3b (N9b): the state block's entry of a pending ask.
        (
            "curator",
            "policy",
            "pending_state",
            json!("an answer from another model"),
            json!("a consult"),
        ),
        ("lead", "lead", "recall_needs", json!([]), json!(["memory"])),
        // Bauer 3a: the interrupt's feed section, the speaking surface's word.
        (
            "lead",
            "lead",
            "interrupt_section",
            json!(""),
            json!("interrupt"),
        ),
        (
            "collector",
            "assemble",
            "recall_needs",
            json!([]),
            json!(["memory"]),
        ),
        // Fix round (review M2): the empty-state sentence of the recalling
        // hive, the composition's word.
        (
            "curator",
            "push",
            "nothing_prefix",
            json!(""),
            json!("Nothing in this memory answers"),
        ),
        // Fix round (review M1): the renamed knobs of the lead.
        ("lead", "lead", "recall_to_fallback", json!("0"), json!("1")),
        (
            "lead",
            "lead",
            "ask_pending_ms",
            json!(120000),
            json!(86400000),
        ),
    ]
}

#[test]
fn gh_d583_namen_a_set_override_of_the_new_params_passes_the_door() {
    if !shipped() || !lead_shipped() {
        return;
    }
    let mut wrong = Vec::new();
    for (template, cell, knob, default, set) in new_params() {
        let cfg = read_json(&repo(&format!("templates/{template}/{cell}/config.json")));
        if cfg["params"].get(knob) != Some(&default) {
            wrong.push(format!(
                "{template}/{cell} ships `{knob}` {} -- want {default}",
                cfg["params"].get(knob).cloned().unwrap_or(Value::Null)
            ));
        }
        let setting = &cfg["contract"]["settings"][knob];
        if !setting["description"]
            .as_str()
            .is_some_and(|d| !d.is_empty())
        {
            wrong.push(format!(
                "{template}/{cell} declares no `{knob}` that says what it does: {setting}"
            ));
        }
        if let Err(why) = door(template, cell, &json!({ knob: set })) {
            wrong.push(format!(
                "a set's `{knob}` on {template}/{cell} must pass the door: {why}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} new param(s) a set cannot turn:\n  {}",
        wrong.len(),
        wrong.join("\n  ")
    );
}

/// Fix round (review M2): the push reads no wording of another hive ab Werk
/// -- a bundle is empty by its structural signals only -- and a set's
/// `nothing_prefix` is the sentence it reads as empty.
#[test]
fn gh_d583_namen_the_push_reads_the_empty_sentence_from_its_param() {
    if !shipped() {
        return;
    }
    let body = json!({"messages": [{"text": "Nothing in this memory answers this question."}]});
    let (got, err) = cell_scope(
        "push",
        json!({}),
        "bundle_find(ARGS[0], ARGS[1])",
        json!([body, {}]),
    );
    assert_eq!(
        got,
        json!("Nothing in this memory answers this question."),
        "shipped: no wording is read, the text is a find: {err}"
    );
    let (got, err) = cell_scope(
        "push",
        json!({}),
        "bundle_find(ARGS[0], ARGS[1])",
        json!([body, {"recall_empty": "1"}]),
    );
    assert_eq!(
        got,
        json!(""),
        "the structural signal still marks it empty: {err}"
    );
    let (got, err) = cell_scope(
        "push",
        json!({"nothing_prefix": "Nothing in this memory answers"}),
        "bundle_find(ARGS[0], ARGS[1])",
        json!([body, {}]),
    );
    assert_eq!(got, json!(""), "the set's sentence is no find: {err}");
}
