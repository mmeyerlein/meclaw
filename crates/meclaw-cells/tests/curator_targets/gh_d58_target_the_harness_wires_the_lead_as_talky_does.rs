//! D58 close (GH #1494; review 3 n2 of the target strand) -- the harness
//! wires the lead and the intake as talky does.
//!
//! WHY this file exists: the curator's harness (`support/curator_hive.rs`)
//! wires the lead with talky's knobs by hand (`LEAD_WIRING`, the intake's
//! `INTAKE_WIRING`), and every curator test that meets a target through the
//! lead runs on that copy. Nothing held the copy to talky's composition:
//! talky could change `bridge_wording` or a route, and the curator tests
//! would keep passing on the old values. Here each knob of the copy is
//! compared with talky's `override_params` -- the lead's on
//! `talky/lead/config.json`, the intake's on `talky/curator/config.json`.
//! The one difference is named: talky's `path_routes` ends with the
//! catch-all `*:brain`, which the harness copy leaves out (the lead's table
//! test `gh_d38_c1` appends it where it needs it).
//!
//! Reads the shipped tree; no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

#[test]
fn gh_d58_target_the_harness_wires_the_lead_as_talky_does() {
    if !shipped() || !repo("templates/talky/lead/config.json").is_file() {
        return;
    }
    let lead = &read_json(&repo("templates/talky/lead/config.json"))["override_params"]["lead"];
    let talky_keys: Vec<&String> = lead
        .as_object()
        .map(|o| o.keys().collect())
        .unwrap_or_default();
    let mut failures = Vec::new();
    for (key, value) in LEAD_WIRING {
        let want = if key == "path_routes" {
            json!(format!("{value},*:brain"))
        } else {
            json!(value)
        };
        if lead[key] != want {
            failures.push(format!(
                "lead {key}: the harness wires {want}, talky {}",
                lead[key]
            ));
        }
    }
    for key in &talky_keys {
        if !LEAD_WIRING.iter().any(|(k, _)| k == key) {
            failures.push(format!("lead {key}: talky sets it, the harness does not"));
        }
    }
    let intake =
        &read_json(&repo("templates/talky/curator/config.json"))["override_params"]["intake"];
    for (key, value) in INTAKE_WIRING {
        if intake[key] != json!(value) {
            failures.push(format!(
                "intake {key}: the harness wires {value}, talky {}",
                intake[key]
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
