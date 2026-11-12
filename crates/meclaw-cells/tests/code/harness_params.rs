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
        session_idle_ms: 1_800_000,
        session_max_context_tokens: None,
        session_max_usd: None,
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
        session_idle_ms: 1_800_000,
        session_max_context_tokens: None,
        session_max_usd: None,
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

/// D57 V4 / OR-57-8: the app id and an explicit `ANTHROPIC_CUSTOM_HEADERS`
/// entry would both set the child's headers — one would silently lose, so the
/// pair is refused, naming both keys. The app id itself is immutable.
#[test]
fn an_app_id_next_to_an_explicit_custom_header_env_is_refused() {
    use meclaw_cells::harness::HarnessOverlay;
    use meclaw_cells::params_overlay::OverlayParams;

    let dir = tempfile::tempdir().expect("tempdir");
    let mut cfg = minimal(dir.path());
    cfg["x_title"] = json!("meclaw-lab harness");
    cfg["env"] = json!({"ANTHROPIC_CUSTOM_HEADERS": "X-Other: 1"});
    let err = HarnessParams::parse(&cfg).expect_err("must refuse");
    assert!(
        err.contains("x_title") && err.contains("ANTHROPIC_CUSTOM_HEADERS"),
        "{err}"
    );

    // Review d57 s1 Minor 6: the same variable passed through from the
    // colony's environment is a second source just the same.
    let mut cfg = minimal(dir.path());
    cfg["http_referer"] = json!("https://meclaw.ai");
    cfg["env_passthrough"] = json!(["PATH", "HOME", "ANTHROPIC_CUSTOM_HEADERS"]);
    let err = HarnessParams::parse(&cfg).expect_err("must refuse the passthrough too");
    assert!(
        err.contains("http_referer")
            && err.contains("env_passthrough")
            && err.contains("ANTHROPIC_CUSTOM_HEADERS"),
        "{err}"
    );
    let mut cfg = minimal(dir.path());
    cfg["env_passthrough"] = json!(["PATH", "ANTHROPIC_CUSTOM_HEADERS"]);
    HarnessParams::parse(&cfg).expect("the passthrough without an app id parses");

    let mut cfg = minimal(dir.path());
    cfg["http_referer"] = json!("https://meclaw.ai");
    let p = HarnessParams::parse(&cfg).expect("the app id alone parses");
    assert_eq!(p.http_referer.as_deref(), Some("https://meclaw.ai"));

    for key in ["x_title", "http_referer"] {
        assert!(HarnessOverlay::IMMUTABLE_KEYS.contains(&key), "{key}");
    }
}

/// D57 V4: the life cycle defaults to one child per task; `persistent` is the
/// other value, and anything else is refused by name. The session knobs have
/// their defaults (idle 30 min, no rotation limits).
#[test]
fn the_session_mode_defaults_to_per_task_and_rejects_unknown_values() {
    use meclaw_cells::harness::params::SessionMode;

    let dir = tempfile::tempdir().expect("tempdir");
    let p = HarnessParams::parse(&minimal(dir.path())).expect("parse");
    assert_eq!(p.session, SessionMode::PerTask);
    assert_eq!(p.session_idle_ms, 1_800_000);
    assert_eq!(p.session_max_context_tokens, None);
    assert_eq!(p.session_max_usd, None);

    for (value, mode) in [
        ("per_task", SessionMode::PerTask),
        ("persistent", SessionMode::Persistent),
    ] {
        let mut cfg = minimal(dir.path());
        cfg["session"] = json!(value);
        assert_eq!(HarnessParams::parse(&cfg).expect(value).session, mode);
    }

    let mut cfg = minimal(dir.path());
    cfg["session"] = json!("forever");
    cfg["session_idle_ms"] = json!(5);
    let err = HarnessParams::parse(&cfg).expect_err("must refuse");
    assert!(err.contains("session") && err.contains("forever"), "{err}");

    let mut cfg = minimal(dir.path());
    cfg["session"] = json!("persistent");
    cfg["session_idle_ms"] = json!(5);
    cfg["session_max_context_tokens"] = json!(100_000);
    cfg["session_max_usd"] = json!(2.5);
    let p = HarnessParams::parse(&cfg).expect("parse");
    assert_eq!(p.session_idle_ms, 5);
    assert_eq!(p.session_max_context_tokens, Some(100_000));
    assert_eq!(p.session_max_usd, Some(2.5));
}

/// D57 V4 β line: idle and rotation limits are tuning and move at runtime;
/// the life cycle itself and the app id do not.
#[test]
fn the_session_knobs_are_tunable_and_the_mode_and_app_id_are_not() {
    use meclaw_cells::harness::HarnessOverlay;
    use meclaw_cells::params_overlay::apply_update;

    let current = HarnessOverlay {
        model: None,
        max_turns: None,
        max_budget_usd: None,
        startup_timeout_ms: 60_000,
        external_timeout_ms: 30_000,
        query_timeout_ms: 5_000,
        session_idle_ms: 1_800_000,
        session_max_context_tokens: None,
        session_max_usd: None,
    };
    let mut update = serde_json::Map::new();
    update.insert("session_idle_ms".to_string(), json!(60_000));
    update.insert("session_max_context_tokens".to_string(), json!(150_000));
    update.insert("session_max_usd".to_string(), json!(1.5));
    let (new_ov, _) = apply_update(&current, &update).expect("tuning update must apply");
    assert_eq!(new_ov.session_idle_ms, 60_000);
    assert_eq!(new_ov.session_max_context_tokens, Some(150_000));
    assert_eq!(new_ov.session_max_usd, Some(1.5));

    for key in ["session", "x_title", "http_referer"] {
        let mut update = serde_json::Map::new();
        update.insert(key.to_string(), json!("persistent"));
        let err = apply_update(&current, &update).expect_err("must reject");
        assert!(err.detail().contains(key), "{key}: {}", err.detail());
    }
}

/// Review d57 s1 Minor 3 (a): every mutable key travels between the params
/// and the overlay through ONE pair of functions — the runtime params update
/// (`cell.rs`) and the β restore (`factory.rs`) both use it, so no copy kept
/// by hand can forget a key. The update below names every mutable key; a new
/// one fails here until it is carried.
#[test]
fn every_tunable_reaches_the_params_through_the_one_overlay_path() {
    use meclaw_cells::harness::HarnessOverlay;
    use meclaw_cells::params_overlay::{OverlayParams, apply_update};

    let dir = tempfile::tempdir().expect("tempdir");
    let mut params = HarnessParams::parse(&minimal(dir.path())).expect("parse");
    let update = json!({
        "model": "m-2",
        "max_turns": 7,
        "max_budget_usd": 2.5,
        "startup_timeout_ms": 61_000,
        "external_timeout_ms": 31_000,
        "query_timeout_ms": 6_000,
        "session_idle_ms": 5_000,
        "session_max_context_tokens": 1_500,
        "session_max_usd": 0.5,
    });
    let update = update.as_object().expect("object").clone();
    let mut named: Vec<&str> = update.keys().map(|k| k.as_str()).collect();
    let mut mutable: Vec<&str> = HarnessOverlay::KNOWN_KEYS
        .iter()
        .copied()
        .filter(|k| !HarnessOverlay::IMMUTABLE_KEYS.contains(k))
        .collect();
    named.sort_unstable();
    mutable.sort_unstable();
    assert_eq!(named, mutable, "the update must name every mutable key");

    let (overlay, _) = apply_update(&HarnessOverlay::of(&params), &update).expect("tuning applies");
    overlay.apply_to(&mut params);
    assert_eq!(params.model.as_deref(), Some("m-2"));
    assert_eq!(params.max_turns, Some(7));
    assert_eq!(params.max_budget_usd, Some(2.5));
    assert_eq!(params.startup_timeout_ms, 61_000);
    assert_eq!(params.external_timeout_ms, 31_000);
    assert_eq!(params.query_timeout_ms, 6_000);
    assert_eq!(params.session_idle_ms, 5_000);
    assert_eq!(params.session_max_context_tokens, Some(1_500));
    assert_eq!(params.session_max_usd, Some(0.5));
    // And back: the overlay of the tuned params is exactly the update.
    assert_eq!(
        serde_json::to_value(HarnessOverlay::of(&params)).expect("serialize"),
        serde_json::Value::Object(update)
    );
}
