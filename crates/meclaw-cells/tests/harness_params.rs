//! P8 block 3 — birth configuration of the `harness` cell type.
//!
//! Two things are pinned here. First, every required key is rejected BY NAME,
//! because a cell that boots with a half-configured harness would spawn a real
//! agent process against the wrong workspace. Second, the containment defaults:
//! an empty `env_passthrough` in the config must not silently mean "inherit
//! everything".

use meclaw_cells::harness::{ApprovalMode, HarnessAdapter, HarnessParams};
use serde_json::json;

/// The smallest config that may boot: adapter, where to emit, where to work.
fn minimal(workspace_root: &std::path::Path) -> serde_json::Value {
    json!({
        "adapter": "claude-code",
        "emit_to": "/main/coordinator",
        "workspace_root": workspace_root.display().to_string(),
    })
}

#[test]
fn a_minimal_config_parses_with_containment_defaults() {
    let dir = tempfile::tempdir().expect("tempdir");
    let p = HarnessParams::parse(&minimal(dir.path())).expect("parse");

    assert!(matches!(p.adapter, HarnessAdapter::ClaudeCode));
    assert_eq!(p.command, "claude", "the adapter's default binary");
    assert_eq!(p.emit_to.as_str(), "/main/coordinator");

    // Containment (D7): the environment is a closed list by default, and the
    // approval channel is off until a topology asks for it (D6).
    assert_eq!(
        p.env_passthrough,
        vec![
            "PATH".to_string(),
            "HOME".to_string(),
            "USER".to_string(),
            "LANG".to_string(),
            "TERM".to_string()
        ],
        "the default passthrough must be a closed list, not the whole environment"
    );
    assert!(p.env.is_empty());
    assert!(matches!(p.approval, ApprovalMode::Off));

    // Timeouts (D8): A on startup and writes, generous grace, no task budget.
    assert_eq!(p.startup_timeout_ms, 60_000);
    assert_eq!(p.external_timeout_ms, 30_000);
    assert_eq!(p.query_timeout_ms, 5_000);
    assert_eq!(p.kill_grace_ms, 5_000);

    // Nothing is invented for the model — it comes from the config or not at
    // all (standing rule: models live in ${VAR}, never in code).
    assert_eq!(p.model, None);
    assert_eq!(p.max_turns, None);
    assert_eq!(p.permission_mode, None);
}

#[test]
fn every_required_key_is_rejected_by_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    for key in ["adapter", "emit_to", "workspace_root"] {
        let mut cfg = minimal(dir.path());
        cfg.as_object_mut().expect("object").remove(key);
        let err = HarnessParams::parse(&cfg).expect_err("must reject");
        assert!(err.contains(key), "the reject must name {key}, got: {err}");
    }
}

/// One adapter exists today. An unknown value is a loud reject rather than a
/// silent fallback, so a typo cannot start the wrong vendor binary.
#[test]
fn an_unknown_adapter_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut cfg = minimal(dir.path());
    cfg["adapter"] = json!("codex");
    let err = HarnessParams::parse(&cfg).expect_err("must reject");
    assert!(err.contains("adapter"), "got: {err}");
}

/// U11 discipline (`file`/`edit` precedent): the workspace root is checked
/// where it is parsed, so validate and spawn agree.
#[test]
fn a_workspace_root_that_is_missing_or_a_file_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut cfg = minimal(dir.path());
    cfg["workspace_root"] = json!("/nonexistent/definitely/not/here");
    let err = HarnessParams::parse(&cfg).expect_err("must reject a missing root");
    assert!(err.contains("workspace_root"), "got: {err}");

    let file = dir.path().join("a-file");
    std::fs::write(&file, b"x").expect("write");
    cfg["workspace_root"] = json!(file.display().to_string());
    let err = HarnessParams::parse(&cfg).expect_err("must reject a file");
    assert!(err.contains("workspace_root"), "got: {err}");
}

#[test]
fn the_workspace_root_is_canonicalised() {
    let dir = tempfile::tempdir().expect("tempdir");
    let indirect = dir.path().join("sub").join("..");
    std::fs::create_dir_all(dir.path().join("sub")).expect("mkdir");
    let mut cfg = minimal(dir.path());
    cfg["workspace_root"] = json!(indirect.display().to_string());

    let p = HarnessParams::parse(&cfg).expect("parse");
    assert_eq!(
        p.workspace_root,
        dir.path().canonicalize().expect("canonicalize"),
        "the root must be canonical, or the containment check below it is porous"
    );
}

#[test]
fn the_approval_mode_parses_and_rejects_unknown_values() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut cfg = minimal(dir.path());

    cfg["approval"] = json!("channel");
    let p = HarnessParams::parse(&cfg).expect("parse");
    assert!(matches!(p.approval, ApprovalMode::Channel));

    cfg["approval"] = json!("ask-marcus");
    let err = HarnessParams::parse(&cfg).expect_err("must reject");
    assert!(err.contains("approval"), "got: {err}");
}

#[test]
fn the_optional_run_settings_are_carried_through() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut cfg = minimal(dir.path());
    cfg["command"] = json!("/opt/bin/claude");
    cfg["model"] = json!("some-model-id");
    cfg["permission_mode"] = json!("acceptEdits");
    cfg["max_turns"] = json!(12);
    cfg["max_budget_usd"] = json!(1.5);
    cfg["allowed_tools"] = json!(["Bash", "Read"]);
    cfg["extra_args"] = json!(["--strict-mcp-config"]);
    cfg["env"] = json!({"FOO": "bar"});
    cfg["env_passthrough"] = json!(["PATH", "HOME"]);
    cfg["startup_timeout_ms"] = json!(1234);
    cfg["kill_grace_ms"] = json!(999);

    let p = HarnessParams::parse(&cfg).expect("parse");
    assert_eq!(p.command, "/opt/bin/claude");
    assert_eq!(p.model.as_deref(), Some("some-model-id"));
    assert_eq!(p.permission_mode.as_deref(), Some("acceptEdits"));
    assert_eq!(p.max_turns, Some(12));
    assert_eq!(p.max_budget_usd, Some(1.5));
    assert_eq!(
        p.allowed_tools,
        vec!["Bash".to_string(), "Read".to_string()]
    );
    assert_eq!(p.extra_args, vec!["--strict-mcp-config".to_string()]);
    assert_eq!(p.env, vec![("FOO".to_string(), "bar".to_string())]);
    assert_eq!(
        p.env_passthrough,
        vec!["PATH".to_string(), "HOME".to_string()]
    );
    assert_eq!(p.startup_timeout_ms, 1234);
    assert_eq!(p.kill_grace_ms, 999);
}

/// β overlay: what may change under a running cell, and what may not.
///
/// The immutable set is the containment boundary. Swapping the binary, the
/// environment, the workspace root or the permission mode at runtime would let
/// a params message do what the config was written to prevent — so those are
/// identity, not tuning.
#[test]
fn the_containment_keys_are_immutable_in_a_params_update() {
    use meclaw_cells::harness::HarnessOverlay;
    use meclaw_cells::params_overlay::apply_update;

    let current = HarnessOverlay {
        model: None,
        max_turns: None,
        max_budget_usd: None,
        startup_timeout_ms: 60_000,
        external_timeout_ms: 30_000,
        query_timeout_ms: 5_000,
    };
    for key in [
        "adapter",
        "command",
        "emit_to",
        "workspace_root",
        "env",
        "env_passthrough",
        "permission_mode",
        "allowed_tools",
        "extra_args",
        "approval",
        "kill_grace_ms",
    ] {
        let mut update = serde_json::Map::new();
        update.insert(key.to_string(), json!("whatever"));
        let err = apply_update(&current, &update).expect_err("must reject");
        assert!(
            err.detail().contains(key),
            "reject for {key} must name the key, got: {}",
            err.detail()
        );
    }
}

#[test]
fn the_run_budget_and_timeouts_stay_tunable() {
    use meclaw_cells::harness::HarnessOverlay;
    use meclaw_cells::params_overlay::apply_update;

    let current = HarnessOverlay {
        model: None,
        max_turns: None,
        max_budget_usd: None,
        startup_timeout_ms: 60_000,
        external_timeout_ms: 30_000,
        query_timeout_ms: 5_000,
    };
    let mut update = serde_json::Map::new();
    update.insert("max_turns".to_string(), json!(3));
    update.insert("external_timeout_ms".to_string(), json!(1234));
    update.insert("model".to_string(), json!("another-model"));

    let (new_ov, _) = apply_update(&current, &update).expect("tuning update must apply");
    assert_eq!(new_ov.max_turns, Some(3));
    assert_eq!(new_ov.external_timeout_ms, 1234);
    assert_eq!(new_ov.model.as_deref(), Some("another-model"));
}

/// A non-object params slot is a config error, not a panic.
#[test]
fn a_non_object_params_slot_is_rejected() {
    let err = HarnessParams::parse(&json!("nope")).expect_err("must reject");
    assert!(err.contains("params"), "got: {err}");
}
