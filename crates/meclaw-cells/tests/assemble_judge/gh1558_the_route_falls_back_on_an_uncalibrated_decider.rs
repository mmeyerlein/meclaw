//! GH #1558 -- a decider without a calibration answers
//! `decision_uncalibrated`, and the route takes its fallback for it the way it
//! does on any failed decide call: one fallback stamp (`fallback: error`, no
//! yes-values, no needs) and one stderr line that names the code.
//!
//! WHY this file exists: the llm cell refuses to answer on a raw scale
//! instead of guessing a default, so the consumer's existing fallback is the
//! whole behaviour of a colony whose catalogue row has no curve. This pin
//! keeps that fallback holding for the new code by name.
//!
//! Runs the shipped `talky/route` script (`support/assemble_cell.rs`).

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::*;
use serde_json::json;

const ROUTE: &str = "templates/talky/route/config.json";

#[test]
fn gh1558_the_route_falls_back_on_an_uncalibrated_decider() {
    let hop = json!({"finish_reason": "error", "error_code": "decision_uncalibrated",
                     "asked_at": (now_ms() + 3_600_000).to_string()});
    let mut doc = lane("in_decision", hop, json!({"turn_id": "c-1"}), json!([]));
    doc["meta"] = json!({"provider": "decisions",
                         "error": {"source": "calibration",
                                   "detail": "model 'vendor/decider' has no decision_calibration"}});
    let (out, stderr) = run_cell(ROUTE, &[], doc);
    assert_eq!(out.len(), 1, "one stamp: {out:?}");
    let m = &out[0];
    assert_eq!(m["header"]["route"], "stamp", "{m}");
    assert_eq!(m["header"]["fallback"], "error", "{m}");
    assert!(m["stamp"].get("needs_yes").is_none(), "{m}");
    assert!(
        stderr.contains("decision_uncalibrated"),
        "one line names the code: {stderr}"
    );
}
