//! S4 (GH #35) -- the `params.sandbox` schema, parsed and validated.
//!
//! Pure parsing: no syscalls, no filesystem. Everything here is deterministic
//! and runs on any platform. The isolation properties themselves live in
//! `sandbox_isolation.rs`.
//!
//! The schema is a security boundary, so its key sets are CLOSED: an unknown
//! key is an error, never a silently ignored typo. Design in
//! `plans/s4-sandbox/design.md`.

use meclaw_cells::sandbox::{NetworkPolicy, SandboxProfile};
use meclaw_core::serde_json::json;

// ---- absence and the two trust levels ------------------------------------

#[test]
fn absent_sandbox_key_parses_to_none() {
    let p = SandboxProfile::parse(&json!({"runner": "python3"})).unwrap();
    assert!(
        p.is_none(),
        "no sandbox key means no profile, not a default one"
    );
}

#[test]
fn trusted_is_the_escape_hatch_and_carries_nothing() {
    let p = SandboxProfile::parse(&json!({"sandbox": {"trust": "trusted"}}))
        .unwrap()
        .expect("profile");
    assert!(matches!(p, SandboxProfile::Trusted));
}

#[test]
fn restricted_requires_a_filesystem_block() {
    let e = SandboxProfile::parse(&json!({"sandbox": {"trust": "restricted"}})).unwrap_err();
    assert!(
        e.contains("params.sandbox.filesystem"),
        "the error must name the missing key: {e}"
    );
}

#[test]
fn restricted_parses_read_write_and_defaults_network_to_deny() {
    let p = SandboxProfile::parse(&json!({
        "sandbox": {
            "trust": "restricted",
            "filesystem": {"read": ["/srv/data"], "write": ["/srv/work"]}
        }
    }))
    .unwrap()
    .expect("profile");
    match p {
        SandboxProfile::Restricted {
            network,
            filesystem,
            ..
        } => {
            assert_eq!(
                network,
                NetworkPolicy::Deny,
                "a restricted profile without a network key is default-deny"
            );
            assert_eq!(filesystem.read, vec![std::path::PathBuf::from("/srv/data")]);
            assert_eq!(
                filesystem.write,
                vec![std::path::PathBuf::from("/srv/work")]
            );
            assert!(filesystem.runtime, "runtime defaults to true");
        }
        SandboxProfile::Trusted => panic!("expected restricted"),
    }
}

#[test]
fn restricted_honours_an_explicit_network_allow_and_runtime_false() {
    let p = SandboxProfile::parse(&json!({
        "sandbox": {
            "trust": "restricted",
            "network": "allow",
            "filesystem": {"read": ["/srv/data"], "runtime": false}
        }
    }))
    .unwrap()
    .expect("profile");
    match p {
        SandboxProfile::Restricted {
            network,
            filesystem,
            ..
        } => {
            assert_eq!(network, NetworkPolicy::Allow);
            assert!(!filesystem.runtime);
            assert!(filesystem.write.is_empty());
        }
        SandboxProfile::Trusted => panic!("expected restricted"),
    }
}

// ---- closed key sets ------------------------------------------------------

#[test]
fn unknown_sandbox_key_is_rejected() {
    // The typo case that must never pass as "no value, so default".
    let e = SandboxProfile::parse(&json!({
        "sandbox": {"trust": "trusted", "netwrok": "deny"}
    }))
    .unwrap_err();
    assert!(e.contains("netwrok"), "the error must name the key: {e}");
}

#[test]
fn unknown_filesystem_key_is_rejected() {
    let e = SandboxProfile::parse(&json!({
        "sandbox": {
            "trust": "restricted",
            "filesystem": {"read": ["/srv"], "exec": ["/usr"]}
        }
    }))
    .unwrap_err();
    assert!(e.contains("exec"), "the error must name the key: {e}");
}

#[test]
fn missing_trust_is_rejected() {
    let e = SandboxProfile::parse(&json!({"sandbox": {"network": "deny"}})).unwrap_err();
    assert!(e.contains("params.sandbox.trust"), "{e}");
}

#[test]
fn unknown_trust_value_is_rejected() {
    let e = SandboxProfile::parse(&json!({"sandbox": {"trust": "maybe"}})).unwrap_err();
    assert!(e.contains("maybe"), "the error must quote the value: {e}");
}

#[test]
fn unknown_network_value_is_rejected() {
    let e = SandboxProfile::parse(&json!({
        "sandbox": {"trust": "restricted", "network": "off", "filesystem": {"read": ["/srv"]}}
    }))
    .unwrap_err();
    assert!(e.contains("off"), "{e}");
}

#[test]
fn sandbox_must_be_an_object() {
    let e = SandboxProfile::parse(&json!({"sandbox": "restricted"})).unwrap_err();
    assert!(e.contains("params.sandbox"), "{e}");
}

// ---- the contradiction: trusted plus restrictions -------------------------

#[test]
fn trusted_with_restriction_fields_is_rejected() {
    for extra in ["network", "filesystem"] {
        let mut sb = meclaw_core::serde_json::Map::new();
        sb.insert("trust".into(), json!("trusted"));
        sb.insert(
            extra.into(),
            if extra == "network" {
                json!("deny")
            } else {
                json!({"read": ["/srv"]})
            },
        );
        let e = SandboxProfile::parse(&json!({"sandbox": sb})).unwrap_err();
        assert!(
            e.contains("trusted") && e.contains(extra),
            "trusted plus {extra} must be rejected as contradictory: {e}"
        );
    }
}

// ---- phase 2 fields: limits (GH #85) --------------------------------------

#[test]
fn limits_parses_all_three_caps() {
    let p = SandboxProfile::parse(&json!({
        "sandbox": {
            "trust": "restricted",
            "filesystem": {"read": ["/srv"]},
            "limits": {"memory_max_bytes": 268435456, "pids_max": 64, "cpu_max_percent": 50}
        }
    }))
    .unwrap()
    .expect("profile");
    match p {
        SandboxProfile::Restricted { limits, .. } => {
            let l = limits.expect("limits parsed");
            assert_eq!(l.memory_max_bytes, Some(268_435_456));
            assert_eq!(l.pids_max, Some(64));
            assert_eq!(l.cpu_max_percent, Some(50));
        }
        SandboxProfile::Trusted => panic!("expected restricted"),
    }
}

#[test]
fn a_single_cap_is_enough_and_the_others_stay_unset() {
    let p = SandboxProfile::parse(&json!({
        "sandbox": {
            "trust": "restricted",
            "filesystem": {"read": ["/srv"]},
            "limits": {"pids_max": 8}
        }
    }))
    .unwrap()
    .expect("profile");
    match p {
        SandboxProfile::Restricted { limits, .. } => {
            let l = limits.expect("limits parsed");
            assert_eq!(l.pids_max, Some(8));
            assert_eq!(l.memory_max_bytes, None, "an unset cap stays unset");
            assert_eq!(l.cpu_max_percent, None);
        }
        SandboxProfile::Trusted => panic!("expected restricted"),
    }
}

#[test]
fn an_empty_limits_block_is_rejected() {
    let e = SandboxProfile::parse(&json!({
        "sandbox": {
            "trust": "restricted",
            "filesystem": {"read": ["/srv"]},
            "limits": {}
        }
    }))
    .unwrap_err();
    assert!(
        e.contains("params.sandbox.limits"),
        "a limits block that caps nothing is a lie, not a default: {e}"
    );
}

#[test]
fn unknown_limits_key_is_rejected() {
    let e = SandboxProfile::parse(&json!({
        "sandbox": {
            "trust": "restricted",
            "filesystem": {"read": ["/srv"]},
            "limits": {"memory_max": 1}
        }
    }))
    .unwrap_err();
    assert!(e.contains("params.sandbox.limits"), "{e}");
    assert!(e.contains("memory_max"), "{e}");
}

#[test]
fn a_zero_or_negative_cap_is_rejected() {
    for bad in [json!({"pids_max": 0}), json!({"memory_max_bytes": -1})] {
        let e = SandboxProfile::parse(&json!({
            "sandbox": {
                "trust": "restricted",
                "filesystem": {"read": ["/srv"]},
                "limits": bad
            }
        }))
        .unwrap_err();
        assert!(e.contains("params.sandbox.limits"), "{bad}: {e}");
    }
}

#[test]
fn cpu_max_percent_above_the_core_count_is_still_a_number_not_a_ratio() {
    // 200 percent means two whole cores; the cap is a percentage of ONE core,
    // so a value above 100 is legal on a multi-core host.
    let p = SandboxProfile::parse(&json!({
        "sandbox": {
            "trust": "restricted",
            "filesystem": {"read": ["/srv"]},
            "limits": {"cpu_max_percent": 200}
        }
    }))
    .unwrap()
    .expect("profile");
    match p {
        SandboxProfile::Restricted { limits, .. } => {
            assert_eq!(limits.unwrap().cpu_max_percent, Some(200));
        }
        SandboxProfile::Trusted => panic!("expected restricted"),
    }
}

// ---- phase 2 fields: syscalls (GH #85) ------------------------------------

#[test]
fn a_present_syscalls_block_denies_every_axis_it_does_not_mention() {
    let p = SandboxProfile::parse(&json!({
        "sandbox": {
            "trust": "restricted",
            "filesystem": {"read": ["/srv"]},
            "syscalls": {}
        }
    }))
    .unwrap()
    .expect("profile");
    match p {
        SandboxProfile::Restricted { syscalls, .. } => {
            let s = syscalls.expect("syscalls parsed");
            assert!(s.deny_ptrace, "naming the block means default-deny");
            assert!(s.deny_raw_sockets);
            assert!(s.deny_foreign_signals);
        }
        SandboxProfile::Trusted => panic!("expected restricted"),
    }
}

#[test]
fn an_axis_can_be_opted_out_of_explicitly() {
    let p = SandboxProfile::parse(&json!({
        "sandbox": {
            "trust": "restricted",
            "filesystem": {"read": ["/srv"]},
            "syscalls": {"foreign_signals": "allow"}
        }
    }))
    .unwrap()
    .expect("profile");
    match p {
        SandboxProfile::Restricted { syscalls, .. } => {
            let s = syscalls.expect("syscalls parsed");
            assert!(!s.deny_foreign_signals, "an explicit allow is honoured");
            assert!(s.deny_ptrace, "and it does not loosen the other axes");
        }
        SandboxProfile::Trusted => panic!("expected restricted"),
    }
}

#[test]
fn a_syscalls_block_that_denies_nothing_is_rejected() {
    let e = SandboxProfile::parse(&json!({
        "sandbox": {
            "trust": "restricted",
            "filesystem": {"read": ["/srv"]},
            "syscalls": {"ptrace": "allow", "raw_sockets": "allow", "foreign_signals": "allow"}
        }
    }))
    .unwrap_err();
    assert!(e.contains("params.sandbox.syscalls"), "{e}");
}

#[test]
fn unknown_syscalls_key_or_value_is_rejected() {
    let e = SandboxProfile::parse(&json!({
        "sandbox": {
            "trust": "restricted",
            "filesystem": {"read": ["/srv"]},
            "syscalls": {"ptrce": "deny"}
        }
    }))
    .unwrap_err();
    assert!(e.contains("params.sandbox.syscalls"), "{e}");

    let e = SandboxProfile::parse(&json!({
        "sandbox": {
            "trust": "restricted",
            "filesystem": {"read": ["/srv"]},
            "syscalls": {"ptrace": "maybe"}
        }
    }))
    .unwrap_err();
    assert!(e.contains("params.sandbox.syscalls.ptrace"), "{e}");
}

#[test]
fn absent_phase_two_blocks_stay_absent() {
    let p = SandboxProfile::parse(&json!({
        "sandbox": {"trust": "restricted", "filesystem": {"read": ["/srv"]}}
    }))
    .unwrap()
    .expect("profile");
    match p {
        SandboxProfile::Restricted {
            limits, syscalls, ..
        } => {
            assert!(limits.is_none(), "no limits key means no caps");
            assert!(syscalls.is_none(), "no syscalls key means no filter");
        }
        SandboxProfile::Trusted => panic!("expected restricted"),
    }
}

#[test]
fn trusted_tolerates_neither_limits_nor_syscalls() {
    for key in ["limits", "syscalls"] {
        let e = SandboxProfile::parse(&json!({
            "sandbox": {"trust": "trusted", key: {}}
        }))
        .unwrap_err();
        assert!(
            e.contains(key),
            "trust \"trusted\" is the no-enforcement hatch and must not carry {key}: {e}"
        );
    }
}

// ---- path shape -----------------------------------------------------------

#[test]
fn relative_paths_are_rejected() {
    let e = SandboxProfile::parse(&json!({
        "sandbox": {"trust": "restricted", "filesystem": {"read": ["srv/data"]}}
    }))
    .unwrap_err();
    assert!(e.contains("absolute"), "{e}");
}

#[test]
fn empty_path_is_rejected() {
    let e = SandboxProfile::parse(&json!({
        "sandbox": {"trust": "restricted", "filesystem": {"write": [""]}}
    }))
    .unwrap_err();
    assert!(e.contains("params.sandbox.filesystem.write"), "{e}");
}

#[test]
fn non_string_path_is_rejected() {
    let e = SandboxProfile::parse(&json!({
        "sandbox": {"trust": "restricted", "filesystem": {"read": [42]}}
    }))
    .unwrap_err();
    assert!(e.contains("params.sandbox.filesystem.read"), "{e}");
}

#[test]
fn read_must_be_an_array() {
    let e = SandboxProfile::parse(&json!({
        "sandbox": {"trust": "restricted", "filesystem": {"read": "/srv"}}
    }))
    .unwrap_err();
    assert!(e.contains("array"), "{e}");
}

#[test]
fn restricted_with_no_allowed_path_at_all_is_rejected() {
    // An allowlist with nothing on it and no runtime set cannot start a
    // process: the runner binary itself would be unreachable. Rejecting it at
    // load beats a spawn failure the operator has to decode.
    let e = SandboxProfile::parse(&json!({
        "sandbox": {"trust": "restricted", "filesystem": {"runtime": false}}
    }))
    .unwrap_err();
    assert!(e.contains("params.sandbox.filesystem"), "{e}");
}

// ---- the factories reject a broken profile before anything spawns --------

#[test]
fn bash_factory_validate_params_rejects_a_broken_sandbox() {
    use meclaw_colony::CellFactory;
    let f = meclaw_cells::BashCellFactory;
    let r = f.validate_params(&json!({"sandbox": {"trust": "restricted"}}));
    assert!(
        r.is_err(),
        "a restricted profile without filesystem must not pass boot"
    );
}

#[test]
fn bash_factory_validate_params_accepts_a_sound_sandbox() {
    use meclaw_colony::CellFactory;
    let f = meclaw_cells::BashCellFactory;
    let r = f.validate_params(&json!({
        "sandbox": {"trust": "restricted", "filesystem": {"read": ["/usr"]}}
    }));
    assert!(r.is_ok(), "{r:?}");
}

#[test]
fn code_params_parse_rejects_a_broken_sandbox() {
    let r = meclaw_cells::code::CodeParams::parse(&json!({
        "runner": "python3",
        "script_inline": "pass",
        "sandbox": {"trust": "restricted", "filesystem": {"read": ["/usr"]}, "limits": {}}
    }));
    assert!(r.is_err(), "a reserved phase-2 key must not pass boot");
}

#[test]
fn code_params_parse_carries_a_sound_sandbox() {
    let p = meclaw_cells::code::CodeParams::parse(&json!({
        "runner": "python3",
        "script_inline": "pass",
        "sandbox": {"trust": "restricted", "filesystem": {"read": ["/usr"]}}
    }))
    .unwrap();
    assert!(p.sandbox.is_some());
}
