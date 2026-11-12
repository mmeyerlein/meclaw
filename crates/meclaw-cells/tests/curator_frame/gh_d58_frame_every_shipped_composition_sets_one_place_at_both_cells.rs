//! D58 frame (GH #1478, frame slot point 2) -- every shipped composition
//! sets one place at both cells.
//!
//! WHY this file exists: where and when the person is lives in two params,
//! `place` and `timezone`, once at `curator/policy` (the leaf `frame`) and
//! once at `curator/history` (the fact line of the seed and the judge's
//! context). Params are per cell, so two params stay; one value is what a
//! composition promises by setting the same value at both. A composition
//! that sets it at one cell only tells its model one place and its judge
//! another. The test reads every composition of the tree that wires a
//! curator (`override_params` of its curator node) and holds the two
//! values equal; an unset value is the cell's shipped one. A composition
//! built here that sets the place at one cell only proves the reader trips.
//!
//! In this tree the test holds trivially: no composition of core sets
//! `place` or `timezone` at either cell, so every value is the shipped empty
//! one at both. It counts for a set that renders a place into its
//! compositions (plan review R-H3); empty values are not written into the
//! compositions to make it bite here.
//!
//! Reads the shipped templates (`support/curator_hive.rs` for the paths).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const KNOBS: [&str; 2] = ["place", "timezone"];

/// The value `knob` has at `cell` under the overrides `over` of a curator
/// node: the override, else the cell's shipped param.
fn value_at(over: &Value, cell: &str, knob: &str) -> Value {
    match over.get(cell).and_then(|c| c.get(knob)) {
        Some(v) if !v.is_null() => v.clone(),
        _ => cell_config(cell)["params"][knob].clone(),
    }
}

/// Every knob of [`KNOBS`] whose value differs between the two cells.
fn findings(name: &str, over: &Value) -> Vec<String> {
    KNOBS
        .iter()
        .filter_map(|k| {
            let (p, h) = (value_at(over, "policy", k), value_at(over, "history", k));
            (p != h).then(|| format!("{name}: `{k}` is {p} at policy and {h} at history"))
        })
        .collect()
}

/// The curator nodes of the shipped compositions: every `config.json`
/// under `templates/` (outside the curator itself and the pinned copies)
/// whose cell is a reference to a curator template, with its overrides.
fn curator_nodes() -> Vec<(String, Value)> {
    let mut out = Vec::new();
    let mut stack = vec![repo("templates")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            let name = p.file_name().map(|n| n.to_string_lossy().to_string());
            if p.is_dir() {
                if name
                    .as_deref()
                    .is_some_and(|n| n.contains('@') || n == "curator")
                    && p.parent() == Some(repo("templates").as_path())
                {
                    continue;
                }
                stack.push(p);
            } else if name.as_deref() == Some("config.json") {
                let cfg = read_json(&p);
                let tpl = cfg["cell"]["template"].as_str().unwrap_or_default();
                if cfg["cell"]["type"] == json!("ref")
                    && (tpl == "curator" || tpl.starts_with("curator@"))
                {
                    let rel = p
                        .strip_prefix(repo(""))
                        .map(|r| r.display().to_string())
                        .unwrap_or_else(|_| p.display().to_string());
                    out.push((rel, cfg["override_params"].clone()));
                }
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[test]
fn gh_d58_frame_every_shipped_composition_sets_one_place_at_both_cells() {
    if !shipped() {
        return;
    }
    let nodes = curator_nodes();
    assert!(
        nodes.len() >= 3,
        "check of the setup: the curator nodes of talky, cogny and simple-agent: {nodes:?}"
    );
    let mut failures = Vec::new();
    for (name, over) in &nodes {
        failures.extend(findings(name, over));
    }
    // The reader trips on a composition that sets the place at one cell.
    let one_sided = json!({"policy": {"place": "Springfield"}});
    assert!(
        !findings("built here", &one_sided).is_empty(),
        "check of the setup: a place at the policy only is a finding"
    );
    let both = json!({"policy": {"place": "Springfield", "timezone": "Europe/Berlin"},
                      "history": {"place": "Springfield", "timezone": "Europe/Berlin"}});
    assert!(
        findings("built here", &both).is_empty(),
        "check of the setup: the same values at both cells are no finding"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
