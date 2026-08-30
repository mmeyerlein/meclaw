//! GH #425 — the builder drafts and never applies, and that is a fact about the
//! FILES rather than a promise in a README. R6: "Der Builder wendet NIE selbst
//! an — er bekommt KEINE Kante zur Mutations-Tür (Guardrail per Topologie)."
//!
//! A cell emission is routed over the SENDER's out-edges (scenario case B4); the
//! `target` on the emission is only a diagnostic. So a builder with no edge onto
//! `/colony/*` cannot address the mutation lane at all, whatever its script
//! does. And it cannot be given one later either: `/colony/mutations` is absent
//! from `MUTATION_DRAWABLE_VIRTUAL_ENDPOINTS`, on every scope, so the edge would
//! have to be birth topology — and birth topology is these files.
//!
//! This test asks the shipped tree that question, with no colony and no runtime.

use meclaw_core::serde_json::Value;
use std::path::{Path, PathBuf};

fn builder_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/builder")
}

/// Every `config.json` under the builder template, recursively.
fn every_config(dir: &Path, out: &mut Vec<(PathBuf, Value)>) {
    for e in std::fs::read_dir(dir).expect("builder template dir") {
        let p = e.expect("dir entry").path();
        if p.is_dir() {
            every_config(&p, out);
        } else if p.file_name().is_some_and(|n| n == "config.json") {
            let raw = std::fs::read_to_string(&p).expect("config readable");
            out.push((
                p,
                meclaw_core::serde_json::from_str(&raw).expect("config parses"),
            ));
        }
    }
}

fn configs() -> Vec<(PathBuf, Value)> {
    let mut out = Vec::new();
    every_config(&builder_root(), &mut out);
    assert!(
        !out.is_empty(),
        "the builder template has no config.json at all"
    );
    out
}

/// The endpoints THIS TEMPLATE may address. A strict subset of what a mutation
/// may ever draw (`crates/meclaw-colony/src/mutation/mod.rs`,
/// `MUTATION_DRAWABLE_VIRTUAL_ENDPOINTS`) and deliberately so: the list there is
/// what the substrate permits, this one is what the builder was given, and the
/// two are allowed to differ. `/colony/templates` is the case that made the
/// difference visible (GH #496) — the substrate permits it, and the builder
/// does not read it, because the template catalogue reaches the composer
/// through `builder-librarian` and nowhere else. Anything outside this list is
/// refused here, and `/colony/mutations` is outside it on purpose and for good.
const BUILDER_MAY_ADDRESS: &[&str] = &["/colony/graph", "/colony/registry", "/colony/ledger"];

#[test]
fn no_config_in_the_builder_draws_an_edge_onto_the_control_plane() {
    for (path, cfg) in &configs() {
        let edges = cfg.pointer("/params/graph/edges").and_then(Value::as_array);
        for edge in edges.into_iter().flatten() {
            for key in ["from", "to"] {
                let ep = edge.get(key).and_then(Value::as_str).unwrap_or("");
                if !ep.starts_with("/colony") {
                    continue;
                }
                assert!(
                    BUILDER_MAY_ADDRESS.contains(&ep),
                    "{}: edge {key} = {ep} — the builder reads the control \
                     plane and never writes it; {ep} is not one of {:?}",
                    path.display(),
                    BUILDER_MAY_ADDRESS
                );
            }
        }
    }
}

/// The half of the old assertion that did NOT weaken, said on its own so it
/// cannot be lost in the whitelist above: the mutation door stays unreachable,
/// and it cannot be added later either — `/colony/mutations` is absent from
/// `MUTATION_DRAWABLE_VIRTUAL_ENDPOINTS` on every scope, so such an edge would
/// have to be birth topology, and birth topology is these files.
#[test]
fn no_config_in_the_builder_draws_an_edge_onto_the_mutation_door() {
    for (path, cfg) in &configs() {
        let edges = cfg.pointer("/params/graph/edges").and_then(Value::as_array);
        for edge in edges.into_iter().flatten() {
            for key in ["from", "to"] {
                let ep = edge.get(key).and_then(Value::as_str).unwrap_or("");
                assert!(
                    !ep.starts_with("/colony/mutations"),
                    "{}: edge {key} = {ep} — drafting is not applying",
                    path.display()
                );
            }
        }
    }
}

/// The lane names, from both places a contract lives: `params.contract` on a
/// hive scope marker, and the top-level `contract` block on a cell.
fn declared_lanes(cfg: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for slot in ["/params/contract/accepts", "/params/contract/emits"] {
        for lane in cfg
            .pointer(slot)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(r) = lane.get("route").and_then(Value::as_str) {
                out.push(r.to_string());
            }
        }
    }
    for slot in ["/contract/emits/hop/route", "/contract/consumes/hop/route"] {
        for v in cfg
            .pointer(slot)
            .and_then(|r| r.get("values"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(r) = v.as_str() {
                out.push(r.to_string());
            }
        }
    }
    out
}

#[test]
fn no_lane_of_the_builder_is_named_after_applying() {
    for (path, cfg) in &configs() {
        for route in declared_lanes(cfg) {
            assert!(
                !matches!(route.as_str(), "mutate" | "rescan" | "apply" | "in_apply"),
                "{}: lane {route} — applying is not this template's business",
                path.display()
            );
        }
    }
}

/// The third assertion, and the one that survives a rename: whatever the lanes
/// are called, no script in this hive may write the mutation door's address.
/// A `code` cell cannot address anything without an edge, but a script that
/// SPELLS the path is a script that was written expecting one — and the day
/// somebody adds the edge "just for a test", the guardrail is gone quietly.
#[test]
fn no_script_in_the_builder_spells_the_mutation_door() {
    for (path, cfg) in &configs() {
        let raw = meclaw_core::serde_json::to_string(cfg).expect("re-serialise");
        assert!(
            !raw.contains("/colony/mutations"),
            "{}: the mutation door is spelled out in this file",
            path.display()
        );
        assert!(
            !raw.contains("/colony/templates/rescan"),
            "{}: the rescan door is spelled out in this file",
            path.display()
        );
    }
}
