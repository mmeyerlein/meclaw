//! P9 steps C1–C3 — birth configuration of the `subcolony` cell type.
//!
//! The line these tests defend is the containment boundary. What a `params`
//! MESSAGE may change is tuning; what the operator wrote the config to constrain
//! — which binary runs, on which tree, with which environment, and which context
//! keys are allowed to cross into the child — must not be reachable from a
//! message, or the constraint belongs to whoever can route to the cell.

use meclaw_cells::params_overlay::OverlayParams;
use meclaw_cells::subcolony::{SubcolonyOverlay, SubcolonyParams};
use serde_json::json;

/// A valid params object over a real directory.
fn valid(root: &std::path::Path) -> serde_json::Value {
    json!({"root": root.to_string_lossy()})
}

#[test]
fn the_child_root_is_required() {
    let err = SubcolonyParams::parse(&json!({})).expect_err("must reject");
    assert!(err.contains("root"), "must name the missing key: {err}");
}

#[test]
fn a_root_that_is_not_a_directory_is_rejected_at_parse_time() {
    // Parser invariant: validate_params and spawn_cell share this path, so a
    // config that validates cannot fail at spawn time.
    let err = SubcolonyParams::parse(&json!({"root": "/nonexistent/child/colony"}))
        .expect_err("must reject");
    assert!(err.contains("root"), "must name the key: {err}");
}

#[test]
fn the_defaults_are_the_documented_ones() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let p = SubcolonyParams::parse(&valid(td.path())).expect("must parse");
    assert_eq!(p.command, "meclaw", "the child is a meclaw binary");
    assert_eq!(p.boot_timeout_ms, 30_000);
    assert_eq!(p.request_timeout_ms, 120_000);
    assert_eq!(p.external_timeout_ms, 30_000);
    assert_eq!(p.kill_grace_ms, 5_000);
    assert!(p.emit_to.is_none(), "the origin lane is opt-in");
    assert!(
        p.context_in.is_empty(),
        "nothing crosses the boundary unless it is declared"
    );
}

#[test]
fn the_request_timeout_is_generous_enough_for_a_child_that_thinks() {
    // A child colony may contain an llm cell; the A-timeout on a request must
    // not be tighter than the work it is waiting for.
    let td = tempfile::TempDir::new().expect("tempdir");
    let p = SubcolonyParams::parse(&valid(td.path())).expect("must parse");
    assert!(
        p.request_timeout_ms > p.external_timeout_ms,
        "the request budget must exceed the per-write budget"
    );
}

#[test]
fn context_keys_cross_only_through_an_explicit_mapping() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let mut v = valid(td.path());
    v["context_in"] = json!({"user_id": "user_id", "chat_id": "conversation"});
    let p = SubcolonyParams::parse(&v).expect("must parse");
    let mapped: std::collections::HashMap<_, _> = p.context_in.iter().cloned().collect();
    assert_eq!(mapped.get("user_id").map(String::as_str), Some("user_id"));
    assert_eq!(
        mapped.get("chat_id").map(String::as_str),
        Some("conversation"),
        "a key may be renamed on the way in"
    );
}

#[test]
fn a_mapping_onto_turn_id_is_a_loud_reject() {
    // turn_id is the facade's correlation key. Letting a config write it would
    // let a parent message collide with the key the facade is waiting on.
    let td = tempfile::TempDir::new().expect("tempdir");
    let mut v = valid(td.path());
    v["context_in"] = json!({"anything": "turn_id"});
    let err = SubcolonyParams::parse(&v).expect_err("must reject");
    assert!(err.contains("turn_id"), "must name the reserved key: {err}");
}

#[test]
fn a_malformed_mapping_is_rejected_by_name() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let mut v = valid(td.path());
    v["context_in"] = json!(["user_id"]);
    let err = SubcolonyParams::parse(&v).expect_err("must reject");
    assert!(err.contains("context_in"), "must name the key: {err}");
}

#[test]
fn the_containment_keys_are_immutable_under_a_live_cell() {
    // Everything that decides WHAT runs and WHAT it can see stays fixed.
    for key in [
        "root",
        "command",
        "env",
        "env_passthrough",
        "context_in",
        "emit_to",
        "kill_grace_ms",
    ] {
        assert!(
            SubcolonyOverlay::IMMUTABLE_KEYS.contains(&key),
            "{key} must not be reachable from a params message"
        );
    }
}

#[test]
fn the_tunables_are_mutable_but_known() {
    for key in [
        "boot_timeout_ms",
        "request_timeout_ms",
        "external_timeout_ms",
        "query_timeout_ms",
    ] {
        assert!(SubcolonyOverlay::KNOWN_KEYS.contains(&key), "{key} known");
        assert!(
            !SubcolonyOverlay::IMMUTABLE_KEYS.contains(&key),
            "{key} is tuning and may change"
        );
    }
}
