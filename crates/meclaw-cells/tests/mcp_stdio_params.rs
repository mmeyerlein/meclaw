//! P7 block 3 — the `mcp` transport cut: stdio arrives NEXT TO http, and every
//! existing http config stays valid byte for byte (ratified D5).

use meclaw_cells::mcp::params::{McpParams, McpTransport};
use serde_json::json;

#[test]
fn a_config_without_transport_is_http_as_before() {
    let p = McpParams::parse(&json!({"endpoint": "https://x.example/rpc"})).expect("parse");
    match p.transport {
        McpTransport::Http { endpoint, bearer } => {
            assert_eq!(endpoint, "https://x.example/rpc");
            assert_eq!(bearer, None);
        }
        other => panic!("default transport must be http, got {other:?}"),
    }
    assert_eq!(p.external_timeout_ms, 30_000);
    assert_eq!(p.query_timeout_ms, 5_000);
}

#[test]
fn an_explicit_http_transport_parses_like_the_implicit_one() {
    let p = McpParams::parse(&json!({
        "transport": "http",
        "endpoint": "https://x.example/rpc",
        "auth": {"bearer": "tok"}
    }))
    .expect("parse");
    match p.transport {
        McpTransport::Http { endpoint, bearer } => {
            assert_eq!(endpoint, "https://x.example/rpc");
            assert_eq!(bearer.as_deref(), Some("tok"));
        }
        other => panic!("expected http, got {other:?}"),
    }
}

#[test]
fn a_stdio_transport_carries_the_full_child_spec() {
    let p = McpParams::parse(&json!({
        "transport": "stdio",
        "command": "/usr/bin/mcp-server",
        "args": ["--stdio", "-v"],
        "env": {"TOKEN": "abc"},
        "cwd": "/srv/work",
        "kill_grace_ms": 750,
        "external_timeout_ms": 1234
    }))
    .expect("parse");
    match p.transport {
        McpTransport::Stdio { spec } => {
            assert_eq!(spec.program, "/usr/bin/mcp-server");
            assert_eq!(spec.args, vec!["--stdio".to_string(), "-v".to_string()]);
            assert_eq!(spec.env, vec![("TOKEN".to_string(), "abc".to_string())]);
            assert_eq!(spec.cwd.as_deref(), Some(std::path::Path::new("/srv/work")));
            assert_eq!(spec.kill_grace_ms, 750);
        }
        other => panic!("expected stdio, got {other:?}"),
    }
    assert_eq!(p.external_timeout_ms, 1234);
}

#[test]
fn stdio_defaults_are_empty_args_and_a_two_second_grace() {
    let p = McpParams::parse(&json!({"transport": "stdio", "command": "srv"})).expect("parse");
    match p.transport {
        McpTransport::Stdio { spec } => {
            assert!(spec.args.is_empty());
            assert!(spec.env.is_empty());
            assert_eq!(spec.cwd, None);
            assert_eq!(spec.kill_grace_ms, 2_000);
        }
        other => panic!("expected stdio, got {other:?}"),
    }
}

/// P8 regression: the child-spec grew two containment switches for the
/// `harness` cell type. `mcp` must keep the pre-P8 behaviour — inherited
/// environment, no process group — or its stdio transport would silently change
/// under an unrelated package.
#[test]
fn the_mcp_transport_leaves_the_p8_containment_switches_off() {
    let p = McpParams::parse(&json!({"transport": "stdio", "command": "srv"})).expect("parse");
    match p.transport {
        McpTransport::Stdio { spec } => {
            assert!(!spec.process_group, "mcp must not open a process group");
            assert!(!spec.env_clear, "mcp must keep inheriting its environment");
        }
        other => panic!("expected stdio, got {other:?}"),
    }
}

#[test]
fn stdio_without_command_is_rejected() {
    let err = McpParams::parse(&json!({"transport": "stdio"})).expect_err("must reject");
    assert!(err.contains("command"), "got: {err}");
}

#[test]
fn http_without_endpoint_is_rejected() {
    let err = McpParams::parse(&json!({"transport": "http"})).expect_err("must reject");
    assert!(err.contains("endpoint"), "got: {err}");
}

#[test]
fn mixing_endpoint_and_command_is_a_loud_reject() {
    let err = McpParams::parse(&json!({"endpoint": "https://x/rpc", "command": "srv"}))
        .expect_err("must reject");
    assert!(
        err.contains("endpoint") && err.contains("command"),
        "the reject must name both keys, got: {err}"
    );
}

#[test]
fn an_unknown_transport_value_is_rejected() {
    let err = McpParams::parse(&json!({"transport": "sse", "endpoint": "https://x/rpc"}))
        .expect_err("must reject");
    assert!(err.contains("transport"), "got: {err}");
}

/// The stdio identity keys are immutable at runtime, like endpoint and auth:
/// swapping the child process under a live cell is an identity change.
#[test]
fn stdio_identity_keys_are_immutable_in_a_params_update() {
    use meclaw_cells::mcp::params::McpOverlay;
    use meclaw_cells::params_overlay::apply_update;

    let current = McpOverlay {
        external_timeout_ms: 30_000,
        query_timeout_ms: 5_000,
    };
    for key in [
        "transport",
        "command",
        "args",
        "env",
        "cwd",
        "kill_grace_ms",
    ] {
        let mut update = serde_json::Map::new();
        update.insert(key.to_string(), json!("whatever"));
        let err = apply_update(&current, &update).expect_err("must reject {key}");
        assert!(
            err.detail().contains(key),
            "reject for {key} must name the key, got: {}",
            err.detail()
        );
    }

    // The two mutable timeouts still apply.
    let mut update = serde_json::Map::new();
    update.insert("external_timeout_ms".to_string(), json!(1234));
    let (new_ov, _) = apply_update(&current, &update).expect("timeout update must apply");
    assert_eq!(new_ov.external_timeout_ms, 1234);
}
