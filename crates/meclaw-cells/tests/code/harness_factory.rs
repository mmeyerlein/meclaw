//! P8 block 3 — the factory, and the parser invariant it has to hold.
//!
//! `CellFactory`'s docs make this a hard requirement: `validate_params` and
//! `spawn_cell` must route through the SAME parse path, because
//! `apply_bootstrap_plan` calls `.expect("validated in plan-phase")` on the
//! strength of it. A divergence turns a config error into a boot-time panic.

use meclaw_cells::harness::HarnessCellFactory;
use meclaw_colony::CellFactory;
use serde_json::json;
use std::sync::Arc;

fn minimal(root: &std::path::Path) -> serde_json::Value {
    json!({
        "adapter": "claude-code",
        "emit_to": "/main/coord",
        "workspace_root": root.display().to_string(),
    })
}

#[test]
fn validate_accepts_a_minimal_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    let f = Arc::new(HarnessCellFactory);
    f.validate_params(&minimal(dir.path()))
        .expect("must accept");
}

#[test]
fn validate_rejects_an_empty_config_by_name() {
    let f = Arc::new(HarnessCellFactory);
    let err = f.validate_params(&json!({})).expect_err("must reject");
    assert!(err.contains("adapter"), "got: {err}");
}

/// The parser invariant, checked where it actually bites: a filesystem fact
/// that `validate_params` does not look at would sail through validation and
/// blow up during boot. Everything `spawn_cell` needs is therefore decided in
/// the shared parse path.
#[test]
fn validate_rejects_what_spawn_would_choke_on() {
    let f = Arc::new(HarnessCellFactory);
    let err = f
        .validate_params(&json!({
            "adapter": "claude-code",
            "emit_to": "/main/coord",
            "workspace_root": "/nonexistent/definitely/not/here",
        }))
        .expect_err("a missing workspace root must be a validation error");
    assert!(err.contains("workspace_root"), "got: {err}");
}
