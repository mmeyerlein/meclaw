//! GH #96 — the `mcp` child reads `params.sandbox`, and the `subcolony` child
//! deliberately does not.
//!
//! Three spawn sites in this tree start a foreign process. `bash`, `code` and
//! `harness` came under a declared profile in #35 and #85. The shared stdio
//! child core grew a `sandbox` field for the third, and it was left `None` for
//! two consumers. This closes the file on both — one by wiring it, one by
//! ruling it out in writing.
//!
//! **`mcp`: wired.** An MCP server is a third-party binary an operator
//! configured, and of the three it is the one least likely to have been written
//! by whoever runs the colony. It now takes the SAME `params.sandbox` schema as
//! the other three: one profile shape, one parser, one set of mistakes an
//! operator can make.
//!
//! **`subcolony`: ruled out, and the reason is in the code.** A child colony is
//! a colony, not a cell running foreign code; its cells carry their own
//! profiles, so a profile here would be a second boundary over the same
//! processes, and the two would disagree the first time somebody tightened one.
//! The half that looks most useful does not survive contact either: a
//! filesystem cut cannot be scoped, because the child needs its own root plus
//! every cell directory below it.
//!
//! Every proof here has a control — a declaration that is *absent* must behave
//! exactly as it did before, or "the field is read" would be indistinguishable
//! from "something changed for everybody".

use meclaw_cells::mcp::McpParams;
use meclaw_core::serde_json::json;

/// The stdio child spec of an `mcp` params block, or a panic naming why not.
fn stdio_spec(params: meclaw_core::serde_json::Value) -> meclaw_cells::stdio_child::ChildSpec {
    match McpParams::parse(&params).expect("params parse").transport {
        meclaw_cells::mcp::McpTransport::Stdio { spec } => spec,
        other => panic!("expected a stdio transport, got {other:?}"),
    }
}

#[test]
fn an_mcp_child_without_a_declaration_keeps_the_rights_it_always_had() {
    // The control, and the reason this is opt-in: every `mcp` cell on disk
    // today declares nothing. If absence changed behaviour, this release would
    // break them all.
    let spec = stdio_spec(json!({"command": "server-bin", "args": ["--stdio"]}));
    assert!(
        spec.sandbox.is_none(),
        "no declaration means no profile — the historical behaviour, unchanged"
    );
}

#[test]
fn a_declared_profile_reaches_the_mcp_child_spec() {
    let spec = stdio_spec(json!({
        "command": "server-bin",
        "sandbox": {"trust": "restricted", "network": "deny",
                    "filesystem": {"read": ["/usr"]}}
    }));
    let profile = spec
        .sandbox
        .as_ref()
        .expect("a declared profile reaches the child spec");
    assert!(
        matches!(
            **profile,
            meclaw_cells::sandbox::SandboxProfile::Restricted { .. }
        ),
        "`restricted` stays restricted all the way to the spawn"
    );
}

#[test]
fn the_mcp_profile_is_parsed_by_the_same_reader_as_the_other_cells() {
    // Not "a similar schema" — the same function, so an operator who learned
    // the shape on a `code` cell has learned it here. The receipt is that the
    // same malformed block produces a parse error rather than being ignored.
    let err = McpParams::parse(&json!({
        "command": "server-bin",
        "sandbox": {"trust": "restricted", "frobnicate": true}
    }))
    .expect_err("an unknown sandbox key must not be silently accepted");
    assert!(
        err.contains("frobnicate"),
        "the error names the offending key: {err}"
    );

    let err = McpParams::parse(&json!({
        "command": "server-bin",
        "sandbox": {"trust": "trusted", "network": "deny"}
    }))
    .expect_err("`trusted` plus an enforcement key is the contradiction the parser rejects");
    assert!(err.contains("trusted"), "{err}");
}

#[test]
fn sandbox_is_immutable_at_runtime() {
    // A containment a runtime params update could switch off is not a
    // containment. Same argument as the store's `write_surface`.
    use meclaw_cells::params_overlay::OverlayParams as _;
    type O = meclaw_cells::mcp::McpOverlay;
    assert!(
        O::IMMUTABLE_KEYS.contains(&"sandbox"),
        "sandbox must be immutable: {:?}",
        O::IMMUTABLE_KEYS
    );
    assert!(
        O::KNOWN_KEYS.contains(&"sandbox"),
        "and known, so touching it is an `Immutable` reject rather than a vaguer `Unknown`"
    );
}

#[test]
fn a_subcolony_child_carries_no_profile_and_says_why() {
    // The ruling, pinned. If somebody later wires a profile here, this test
    // fails and points at the paragraph that argued against it — which is the
    // only way a deliberate omission survives contact with a later reader.
    let td = tempfile::TempDir::new().expect("tempdir");
    let params = meclaw_cells::subcolony::SubcolonyParams::parse(&json!({
        "root": td.path().to_string_lossy(),
    }))
    .expect("a minimal subcolony params block parses");
    let spec = meclaw_cells::subcolony::io::child_spec_for_test(&params);
    assert!(
        spec.sandbox.is_none(),
        "a subcolony gets no profile of its own (GH #96 ruling)"
    );
    // What DOES contain it, and is not nothing:
    assert!(
        spec.process_group,
        "the child runs in its own process group — no orphan survives the parent"
    );
    assert!(
        spec.env_clear,
        "and sees the passthrough list, not this colony's environment"
    );
}
