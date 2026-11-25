//! Wave d54s, strand `specaus` (GH #1584, R-54-9, Nr. 48 a/c) -- the early
//! answer of the quick voice (`spec_ahead`, GH #1275) is gone from the three
//! scripts that carried it and from the two hives that wired its lanes.
//!
//! WHY this file exists: the set had the speculation switched off since 0.72
//! (`spec_ahead` "0", `path_routes` `simple:brain`), and about 500 script
//! lines -- the lead's `quick_spec v1`/`spec automaton v1`, the collector's
//! `spec_fire`, the policy's `quick_spec v1` -- ran on every turn without
//! ever deciding one (`~/kb` reader 48 a/c). R-54-8: switched-off code is
//! taken out, not locked. This lock keeps it out: no cell reads a `spec_*`
//! param or `delta_speculation`, and no hive names a lane of the
//! speculation. The early interrupt stays: its delta stamp reaches the lead
//! as a note (`lead_note` kind `delta_stamp`).
//!
//! Reads the shipped templates; no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value};

/// The cells that carried the speculation, by their path under `templates`.
const CELLS: [&str; 3] = ["lead/lead", "collector/assemble", "curator/policy"];
/// The hives that wired its lanes.
const HIVES: [&str; 2] = ["lead", "curator"];
/// The lanes of the speculation (routes in and out).
const LANES: [&str; 5] = [
    "lead_spec",
    "in_spec_llm",
    "in_spec_partial",
    "spec_release",
    "spec_tap",
];

fn config(rel: &str) -> Value {
    read_json(&repo("templates").join(rel).join("config.json"))
}

/// GH #49: every config this file reads travelled into this tree.
fn shipped() -> bool {
    CELLS
        .iter()
        .chain(HIVES.iter())
        .all(|rel| repo("templates").join(rel).join("config.json").is_file())
}

fn spec_knob(key: &str) -> bool {
    key.starts_with("spec_") || key == "delta_speculation"
}

#[test]
fn no_cell_reads_a_spec_knob() {
    if !shipped() {
        return;
    }
    let mut found = Vec::new();
    for rel in CELLS {
        let cfg = config(rel);
        let params = cfg["params"].as_object().cloned().unwrap_or_default();
        let settings = cfg["contract"]["settings"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        for key in params.keys().chain(settings.keys()) {
            if spec_knob(key) {
                found.push(format!("{rel}: knob {key}"));
            }
        }
        let script = cfg["params"]["script_inline"].as_str().unwrap_or_default();
        for needle in ["\"spec_", "delta_speculation"] {
            if script.contains(needle) {
                found.push(format!("{rel}: script names {needle}"));
            }
        }
    }
    assert!(found.is_empty(), "the speculation is still read: {found:?}");
}

#[test]
fn no_hive_wires_a_lane_of_the_speculation() {
    if !shipped() {
        return;
    }
    let mut found = Vec::new();
    for rel in HIVES {
        let text = sj::to_string(&config(rel)["params"]).expect("params as text");
        for lane in LANES {
            if text.contains(&format!("'{lane}'")) || text.contains(&format!("\"{lane}\"")) {
                found.push(format!("{rel}: {lane}"));
            }
        }
    }
    assert!(
        found.is_empty(),
        "a lane of the speculation is still wired: {found:?}"
    );
}
