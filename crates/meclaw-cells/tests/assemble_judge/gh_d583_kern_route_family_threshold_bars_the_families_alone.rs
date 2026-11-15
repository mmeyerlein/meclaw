//! D58-3 (OR-583-70) -- `family_threshold` bars the families alone.
//!
//! WHY this file exists: the route stamp's `families` scope the tools of a
//! quick round, its `needs` give the tool duty, and until now both counted at
//! the one bar `yes_threshold`. A set that wants the families named more (or
//! less) readily than a tool duty had no knob for it. `family_threshold` is
//! that knob: a number bars the families, the needs stay at `yes_threshold`.
//! Shipped it is "" -- not a number, so the families follow `yes_threshold`
//! and the stamp is the stamp of before, also when a set moves
//! `yes_threshold`. A set may override it only when the template declares it
//! under `params` (`check_override_params`, d53-3 K4 (1)).
//!
//! In the harness of the route pins (`assemble_judge`, rule
//! `^gh_d583_kern_route`), not `curator_sections`: the runner of the shipped
//! `talky/route` script, `support/assemble_cell.rs`, is folded here.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::*;
use meclaw_colony::mutation::subtree::{check_override_params, parse_subtree};
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use serde_json::{Value, json};

const ROUTE: &str = "templates/talky/route/config.json";

fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A judged answer of the decider for turn `c-1`: a sure path, and yes-values
/// that sit between the bars the cases below move.
fn judged() -> Value {
    let answers = json!({
        "path": {"choice": "simple", "p": 0.9},
        "intent": {"choice": "question", "p": 0.8},
        "needs.tool": {"yes": 0.55}, "needs.reasoning": {"yes": 0.3},
        "needs.research": {"yes": 0.0},
        "families.web": {"yes": 0.55}, "families.history": {"yes": 0.3}
    });
    let hop = json!({"finish_reason": "stop", "asked_at": now_ms().to_string()});
    let mut doc = lane("in_decision", hop, json!({"turn_id": "c-1"}), json!([]));
    doc["decision"] = json!({"answers": answers, "missing": [], "model": "d"});
    doc
}

/// The stamp's `(needs, families)` under the overrides `over`.
fn verdict(over: &[(&str, Value)]) -> (Value, Value) {
    let out = run_cell(ROUTE, over, judged()).0;
    assert_eq!(out.len(), 1, "expected one emission, got {out:?}");
    let m = &out[0];
    assert_eq!(m["header"]["route"], "stamp", "{m}");
    assert!(m["header"].get("fallback").is_none(), "a judged stamp: {m}");
    (m["stamp"]["needs"].clone(), m["stamp"]["families"].clone())
}

/// `None` when a set's `over` on `talky/route` passes the door, else why not.
fn refused(over: Value) -> Option<String> {
    let scanned = scan_templates_dir(&core_root().join("templates")).unwrap_or_default();
    let registry = TemplatesRegistry::from_entries(
        scanned
            .into_iter()
            .map(|s| TemplateEntry {
                template_id: format!("scan-{}", s.name),
                name: s.name,
                version: s.version,
                filesystem_path: s.filesystem_path,
            })
            .collect(),
    );
    let parsed = parse_subtree(&core_root().join("templates/talky"), &registry)
        .unwrap_or_else(|e| panic!("the shipped talky template parses: {e:?}"));
    let node = parsed
        .cells
        .into_iter()
        .find(|c| c.rel_path == "route")
        .expect("talky has a route cell");
    check_override_params(&node, Some("route"), "talky", &over)
        .err()
        .map(|why| format!("a set's {over} on talky/route must pass the door: {why:?}"))
}

#[test]
fn gh_d583_kern_route_family_threshold_bars_the_families_alone() {
    // Declared, shipped "" (not a number: the bar of before).
    let cfg: Value = serde_json::from_str(
        &std::fs::read_to_string(core_root().join(ROUTE)).expect("route config"),
    )
    .expect("route config is JSON");
    assert_eq!(
        cfg["params"]["family_threshold"],
        json!(""),
        "templates/talky/route/config.json declares family_threshold \"\""
    );
    let setting = &cfg["contract"]["settings"]["family_threshold"];
    assert_eq!(
        setting["default"], cfg["params"]["family_threshold"],
        "the route's setting names the shipped default: {setting}"
    );
    assert!(
        setting["description"]
            .as_str()
            .is_some_and(|d| !d.is_empty()),
        "the setting says what it does: {setting}"
    );

    // Shipped: the families count at `yes_threshold` (0.5), as before.
    let shipped = verdict(&[]);
    assert_eq!(shipped, (json!(["tool"]), json!(["web"])), "shipped stamp");
    // Shipped and a set's `yes_threshold`: the families still follow it.
    assert_eq!(
        verdict(&[("yes_threshold", json!(0.6))]),
        (json!([]), json!([])),
        "with family_threshold unset the families follow yes_threshold"
    );
    assert_eq!(
        verdict(&[("family_threshold", json!(0.5))]),
        shipped,
        "family_threshold at the value of yes_threshold: the shipped stamp"
    );

    // A number bars the families alone, up and down; the needs stay at
    // `yes_threshold`.
    assert_eq!(
        verdict(&[("family_threshold", json!(0.7))]),
        (json!(["tool"]), json!([])),
        "a higher family bar drops web (0.55), keeps the need tool (0.55)"
    );
    assert_eq!(
        verdict(&[("family_threshold", json!(0.25))]),
        (json!(["tool"]), json!(["history", "web"])),
        "a lower family bar adds history (0.3), not the need reasoning (0.3)"
    );

    // A set's override passes the door; an undeclared param does not.
    assert_eq!(refused(json!({"family_threshold": 0.7})), None);
    assert!(
        refused(json!({"family_threshold_never_declared": 0.7})).is_some(),
        "check of the setup: an undeclared param is refused"
    );
}
