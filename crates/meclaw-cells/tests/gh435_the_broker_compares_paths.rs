//! GH #435 — the broker learns PATH prefixes, and learns them under a NEW key.
//!
//! `submit` asks a question the broker could not answer: "may this requester
//! carry a manifest whose scope root lies UNDER `/os/orgs`?". Every comparison
//! in `policy` is an equality or a wildcard, and neither of those is a path
//! prefix. So `scope_match` gains one key, `scope_prefix`, with exactly the
//! semantics the submitter's own `under()` already carries:
//!
//! * `/os` permits `/os` and `/os/orgs`, and does NOT permit `/oscar` — a plain
//!   `startswith` hands out a scope nobody named.
//! * The key is NEW rather than a reinterpretation of `scope`: giving `scope`
//!   prefix semantics would silently widen every rule that ever used it, which
//!   is the one mistake a permission comparator may not make. The second test
//!   below is that promise, measured.
//! * No coordinate, no verdict: a rule that names `scope_prefix` needs a
//!   `resource.scope` to compare against, and its absence is `scope_incomplete`
//!   rather than a match. Fail closed.
//!
//! It also pins the other half: `scope_prefix` describes a PERMISSION, not an
//! address, so it never enters the grant's frozen scope (`build_scope`).
//!
//! **R2b guard (GH #49 form).** `access` is PRIVATE — it does not travel with
//! the export, so in the public clone these tests skip instead of failing on a
//! dead `templates/` reference.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_all, shipped_script};

const POLICY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/access/policy/config.json"
);

const REQUESTER: &str = "/os/submit";

/// The shipped script, or `None` where the private template does not ship.
fn shipped_policy() -> Option<String> {
    if std::path::Path::new(POLICY).exists() {
        Some(shipped_script(POLICY))
    } else {
        None
    }
}

/// One rule, spelled the way a `policy` ROW comes off the store.
fn rule(scope_match: Value) -> Value {
    json!({
        "rule_id": "r1",
        "requester": REQUESTER,
        "capability": "colony.mutate",
        "subject": "*",
        "scope_match": scope_match,
        "verdict": "allow",
        "max_ttl_ms": 60000,
        "constraints": {},
        "priority": 100,
        "cred_ref": "",
    })
}

/// The `rules` phase: the store's answer comes back, the request rides in
/// `context.ac_carry`, and `hop.operation` is what marks the echo.
fn decide(script: &str, rules: Value, resource: Value) -> Vec<Value> {
    let carry = json!({
        "call_id": "call-1",
        "requester": REQUESTER,
        "capability": "colony.mutate",
        "subject": "member:alex",
        "resource": resource,
        "purpose": "",
        "ttl_ms": 0,
    });
    emit_all(
        script,
        &json!({
            "target": "/os/access",
            "header": {
                "hop": {"operation": "select", "route": "astore"},
                // The shipped edge `./policy -> ./store` promotes THREE keys,
                // and `access_origin` is the one the script recognises its own
                // echo by (a `hop.operation` is written by whoever emitted the
                // message, so a caller may carry one). A fixture that omits it
                // is not modelling the edge.
                "context": {"access_origin": "policy", "ac_phase": "rules",
                            "ac_carry": carry.to_string()},
            },
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "s1",
                          "text": rules.to_string()}],
        }),
    )
}

/// The payload of the answer the caller gets: `status` and `reason_code`.
fn verdict(out: &[Value]) -> (String, String) {
    let answer = out.last().expect("the script always answers");
    assert_eq!(answer["header"]["route"], json!("grant"));
    let payload: Value =
        meclaw_core::serde_json::from_str(answer["messages"][0]["text"].as_str().expect("text"))
            .expect("the answer is json");
    (
        payload["status"].as_str().unwrap_or_default().to_string(),
        payload["reason_code"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

/// The store ops the script emitted, parsed out of their `tool_call` turns.
fn ops(out: &[Value]) -> Vec<Value> {
    out.iter()
        .filter(|m| m["header"]["route"] == json!("astore"))
        .map(|m| {
            meclaw_core::serde_json::from_str(m["messages"][0]["text"].as_str().expect("text"))
                .expect("a store op is json")
        })
        .collect()
}

#[test]
fn scope_prefix_is_a_path_prefix_and_not_a_string_prefix() {
    let Some(script) = shipped_policy() else {
        return;
    };
    let rules = json!([rule(json!({"scope_prefix": "/os", "actions": ["apply"]}))]);

    for permitted in ["/os", "/os/orgs", "/os/orgs/acme/members/alex"] {
        let out = decide(&script, rules.clone(), json!({"scope": permitted}));
        assert_eq!(
            verdict(&out).0,
            "granted",
            "`/os` must permit {permitted}: it lies under the prefix"
        );
    }

    for refused in ["/oscar", "/o", "/other/os"] {
        let out = decide(&script, rules.clone(), json!({"scope": refused}));
        assert_eq!(
            verdict(&out),
            ("denied".to_string(), "scope_mismatch".to_string()),
            "`/os` must NOT permit {refused} — a path prefix is not a string prefix"
        );
    }
}

#[test]
fn a_permission_is_not_an_address_so_the_grant_freezes_no_prefix() {
    let Some(script) = shipped_policy() else {
        return;
    };
    let rules = json!([rule(json!({"scope_prefix": "/os", "actions": ["apply"]}))]);
    let out = decide(&script, rules, json!({"scope": "/os/orgs"}));
    let grant = ops(&out)
        .into_iter()
        .find(|o| o["table"] == json!("grants"))
        .expect("an allow rule mints a grant");
    assert!(
        grant["row"]["scope"].get("scope_prefix").is_none(),
        "`scope_prefix` states WHAT IS PERMITTED, not where the grant points — \
         it must not enter the frozen scope `invoke` reads its address out of, got {}",
        grant["row"]["scope"]
    );
    assert_eq!(grant["row"]["scope"]["actions"], json!(["apply"]));
}

#[test]
fn an_existing_scope_match_key_still_compares_by_equality() {
    let Some(script) = shipped_policy() else {
        return;
    };
    // The regression this whole design exists to avoid: had `scope` been given
    // prefix semantics instead, `telegram` would now match `telegram-staging`
    // in every rule ever written.
    let rules = json!([rule(
        json!({"channel": "telegram", "actions": ["send_message"]})
    )]);
    let out = decide(&script, rules, json!({"channel": "telegram-staging"}));
    assert_eq!(
        verdict(&out),
        ("denied".to_string(), "scope_mismatch".to_string()),
        "an ordinary scope_match key compares by equality, exactly as before"
    );
}

#[test]
fn scope_prefix_without_a_scope_in_the_request_is_incomplete_not_allowed() {
    let Some(script) = shipped_policy() else {
        return;
    };
    let rules = json!([rule(json!({"scope_prefix": "/os", "actions": ["apply"]}))]);
    let out = decide(&script, rules, json!({"channel": "telegram"}));
    assert_eq!(
        verdict(&out),
        ("denied".to_string(), "scope_incomplete".to_string()),
        "no coordinate, no verdict — a rule that names a prefix needs a scope to compare"
    );
}
