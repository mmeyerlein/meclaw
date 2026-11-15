//! d58-3 namen (GH #1525, contract point 3, N7) -- the fallback row is the
//! summarizer's own.
//! OR-583.stapel.5, raten F.
//!
//! WHY this file exists: `input_soft_fallback_row` named one model of one
//! provider as the default of the policy, the push and the handover, and
//! `input_soft_fallback` the window of that row -- a model name in the
//! shared hive, and a window that is right only for compositions whose
//! summarizer runs on that model. N7 keeps no model name as a default in the
//! curator. Since the raten ruling F (GH #1532) the row param is gone and the
//! fallback is ONE catalogue fallback window for every model: a producer asked
//! before any window travels falls back to it and says so on stderr. So no
//! shipped composition names a fallback row or overrides the fallback window
//! at the curator: every composition falls back to the curator's own uniform
//! window. That the one value is the same in every core cell is pinned in
//! `misc_1/gh_d583_raten_fallback_one_window_without_a_catalog_row`; here only
//! the curator and the compositions over it.
//!
//! Reads the shipped tree; no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The cells that ship a fallback window.
const PRODUCERS: [&str; 3] = ["policy", "push", "handover"];

/// The one catalogue fallback window (raten F, GH #1532).
const FALLBACK: u64 = 17500;

/// Every shipped composition with a curator cell (frozen versions
/// `<name>@<version>` are history and not read).
fn compositions() -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(repo("templates"))
        .expect("the templates")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| !n.contains('@') && n != "curator")
        .filter(|n| repo(&format!("templates/{n}/curator/config.json")).is_file())
        .collect();
    out.sort();
    out
}

#[test]
fn gh_d583_namen_the_fallback_row_is_the_summarizers_own() {
    if !shipped() {
        return;
    }
    let mut wrong = Vec::new();
    for cell in PRODUCERS {
        let cfg = cell_config(cell);
        if cfg["params"].get("input_soft_fallback_row").is_some()
            || cfg["contract"]["settings"]
                .get("input_soft_fallback_row")
                .is_some()
        {
            wrong.push(format!(
                "curator/{cell} still carries `input_soft_fallback_row` -- the curator names no row"
            ));
        }
        if cfg["params"].get("input_soft_fallback") != Some(&json!(FALLBACK)) {
            wrong.push(format!(
                "curator/{cell} ships `input_soft_fallback` {} -- the catalogue fallback window is {FALLBACK}",
                cfg["params"]["input_soft_fallback"]
            ));
        }
    }
    let comps = compositions();
    assert!(
        comps.iter().any(|c| c == "talky") && comps.iter().any(|c| c == "cogny"),
        "check of the setup: talky and cogny ship a curator, found {comps:?}"
    );
    for comp in &comps {
        let over: Value =
            read_json(&repo(&format!("templates/{comp}/curator/config.json")))["override_params"]
                .clone();
        for cell in PRODUCERS {
            for knob in ["input_soft_fallback_row", "input_soft_fallback"] {
                if let Some(v) = over[cell].get(knob) {
                    wrong.push(format!(
                        "{comp}/curator overrides `{cell}.{knob}` with {v} -- the composition \
                         names no fallback row, the curator falls back to its one window"
                    ));
                }
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{} place(s) where a fallback row or window is set past the curator's one window:\n  {}",
        wrong.len(),
        wrong.join("\n  ")
    );
}
