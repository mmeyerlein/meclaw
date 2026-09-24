//! The configuration surface of a contract class, and its loud refusals.
use meclaw_cells::proxy::meclaw::params::{IMMUTABLE_KEYS, MeclawParams};
use meclaw_core::serde_json::{Value, json};
fn minimal() -> Value {
    json!({"platform": "meclaw", "mount": "peer", "identity_header": "X-Meclaw-Peer",
        "boundary": "north", "emit_to": "/friend", "lanes": {
            "accepts": [{"route": "topic", "fields": ["topic"],
                "because": "a subject this side may consider, never who said it"}],
            "emits": [{"route": "proposal", "fields": ["proposal"],
                "because": "one abstracted proposal, no observation about a person"}]}})
}
#[test]
fn a_minimal_declaration_parses_and_defaults_the_timeouts() {
    let p = MeclawParams::parse(&minimal()).expect("the minimal declaration parses");
    assert_eq!((p.mount.as_str(), p.boundary.as_str()), ("peer", "north"));
    assert_eq!(p.emit_to_path().as_str(), "/friend");
    assert_eq!((p.external_timeout_ms, p.query_timeout_ms), (5000, 5000));
    assert!(
        p.lanes.accepts[0].context.is_empty(),
        "nothing crosses unnamed"
    );
}
#[test]
fn an_unknown_key_is_refused_rather_than_ignored_at_both_levels() {
    let mut v = minimal();
    v["lanse"] = json!({});
    let err = MeclawParams::parse(&v).expect_err("a typo in lanes opens or closes a lane");
    assert!(err.contains("lanse"), "the refusal names the key: {err}");
    let mut w = minimal();
    w["lanes"]["accepts"][0]["at"] = json!(["./somewhere"]);
    let err = MeclawParams::parse(&w).expect_err("a lane has no connect points");
    assert!(err.contains("at"), "{err}");
}
#[test]
fn system_in_a_fields_list_is_refused_at_parse_time() {
    for bad in ["system", "system.tools"] {
        let mut v = minimal();
        v["lanes"]["emits"][0]["fields"] = json!(["proposal", bad]);
        let err = MeclawParams::parse(&v).expect_err("system must never cross");
        assert!(err.contains(bad), "the refusal names the field: {err}");
    }
}
#[test]
fn a_mount_outside_the_grammar_is_refused() {
    for bad in [json!("Peer"), json!("a/b"), json!("colony"), json!("")] {
        let mut v = minimal();
        v["mount"] = bad.clone();
        let err = MeclawParams::parse(&v).expect_err("the mount grammar holds");
        assert!(err.starts_with("mount: "), "{bad}: {err}");
    }
}
#[test]
fn an_identity_header_that_is_not_a_header_name_is_refused() {
    let mut v = minimal();
    v["identity_header"] = json!("X Meclaw Peer");
    let err = MeclawParams::parse(&v).expect_err("a typo must be a refusal at plan time");
    assert!(err.starts_with("identity_header: "), "{err}");
    let mut w = minimal();
    w["identity_header"] = json!("");
    assert!(
        MeclawParams::parse(&w).is_ok(),
        "empty parses; the MOUNT is what refuses"
    );
}
#[test]
fn a_duplicate_route_an_empty_route_and_a_relative_emit_to_are_each_refused() {
    let mut v = minimal();
    v["lanes"]["accepts"] = json!([{"route": "topic", "fields": ["topic"], "because": "one"},
        {"route": "topic", "fields": ["who"], "because": "two"}]);
    let err = MeclawParams::parse(&v).expect_err("one direction holds one lane per route");
    assert!(err.contains("topic"), "the refusal names the route: {err}");
    let mut w = minimal();
    w["lanes"]["emits"][0]["route"] = json!("");
    assert!(MeclawParams::parse(&w).is_err(), "a lane needs a name");
    let mut x = minimal();
    x["emit_to"] = json!("friend");
    assert!(
        MeclawParams::parse(&x)
            .expect_err("absolute")
            .starts_with("emit_to: ")
    );
    // A boundary a message can rename is not a boundary (README § 0a A9).
    // Nine since GH #828: `auth`, the credential, is a key of the boundary too.
    // Ten since GH #833: whose header the mount believes is part of who may cross.
    assert_eq!(
        IMMUTABLE_KEYS.len(),
        10,
        "every key of the variant, and no more"
    );
    assert!(IMMUTABLE_KEYS.contains(&"lanes") && IMMUTABLE_KEYS.contains(&"mount"));
    assert!(IMMUTABLE_KEYS.contains(&"auth"));
    assert!(IMMUTABLE_KEYS.contains(&"trusted_proxies"));
}

/// GH #833: a typo in the list is a refusal at plan time that names the entry,
/// never an address that silently matches nothing.
#[test]
fn a_trusted_proxies_entry_that_is_not_an_address_is_refused_by_name() {
    let mut v = minimal();
    v["trusted_proxies"] = json!(["127.0.0.1", "proxy.example", "192.0.2.0/24"]);
    let err = MeclawParams::parse(&v).expect_err("a host name is not an address");
    assert!(
        err.starts_with(r#"trusted_proxies[1]: "proxy.example" is not an IP address or CIDR"#),
        "{err}"
    );
    let mut w = minimal();
    w["trusted_proxies"] = json!(["192.0.2.0/33"]);
    let err = MeclawParams::parse(&w).expect_err("a prefix past 32");
    assert!(err.starts_with("trusted_proxies[0]: "), "{err}");
    let mut ok = minimal();
    ok["trusted_proxies"] = json!(["192.0.2.0/24", "2001:db8::/32", "::1"]);
    let p = MeclawParams::parse(&ok).expect("addresses and CIDRs parse");
    assert_eq!(p.trusted().len(), 3);
    let absent = MeclawParams::parse(&minimal()).expect("parses");
    assert_eq!(
        absent.trusted(),
        meclaw_colony::surfaces::loopback_only(),
        "no key is loopback (R-AG-1)"
    );
}
