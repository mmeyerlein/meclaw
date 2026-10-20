//! GH #1273 -- a grant of a credential CLASS delivers the credential a
//! catalogue row of that class names, and nothing else.
//!
//! The llm cell's grant is its channel to the vault; which provider it talks to
//! is the registry row's word, and the credential follows the row. The access
//! hive therefore lets a `vault.deliver` payload name a `cred_ref` -- but only
//! one the grant's `scope.cred_class` covers through an ACTIVE `cred_refs` row
//! of that class (R-AW-2: registry and vault agree through the catalogue).
//! Everything else is `cred_ref_not_granted`, audited with the name asked for.
//!
//! | claim | where |
//! |---|---|
//! | an active row of the grant's class is delivered by name | case 1 |
//! | no row, a row of another class, a retired row -> refused | cases 2-4 |
//! | a payload without `cred_ref` keeps the grant's own credential | case 5 |
//! | a grant without `cred_class` refuses any other name | case 6 |
//! | every refusal names the `cred_ref` in its audit row | final check |

use super::access_template::{
    BROKER, boot, build_tree, credential_grant_for, probe, recv_route, send_json, shipped_access,
    turn_json, vault_calls,
};
use meclaw_core::serde_json::{Value, json};

fn class_rule(rule_id: &str, requester: &str, cred_ref: &str, cred_class: Option<&str>) -> Value {
    let mut scope = json!({"actions": ["vault.deliver"]});
    if let Some(c) = cred_class {
        scope["cred_class"] = json!(c);
    }
    json!({"operation": "insert", "table": "policy", "row": {
        "rule_id": rule_id, "requester": requester, "capability": "credential.read",
        "subject": "member:example", "scope_match": scope,
        "verdict": "allow", "max_ttl_ms": 900000,
        "constraints": {"max_invocations": 20},
        "cred_ref": cred_ref,
        "enabled": 1, "priority": 100, "note": "test rule"}})
}

fn catalogue_row(cred_ref: &str, class: &str, status: &str) -> Value {
    json!({"operation": "insert", "table": "cred_refs", "row": {
        "ref": cred_ref, "connector": "./llm", "env_var": "", "owner": "org:example",
        "rotated_at": "1970-01-01T00:00:00.000000Z", "status": status, "class": class}})
}

fn spend(who: &str, grant: &str, cred_ref: Option<&str>) -> Value {
    let mut payload = json!({"recipient_key": "aa".repeat(32)});
    if let Some(r) = cred_ref {
        payload["cred_ref"] = json!(r);
    }
    json!({"who": who, "grant_id": grant, "operation": "vault.deliver", "payload": payload})
}

fn as_obj(v: &Value) -> Value {
    match v {
        Value::String(s) => meclaw_core::serde_json::from_str(s).unwrap_or(Value::Null),
        other => other.clone(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1273_a_grant_of_a_class_delivers_the_row_s_credential() {
    let Some(root) = shipped_access() else {
        return; // GH #49: the template library does not travel with every tree
    };
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &root);
    let (h, mut rx) = boot(&td).await;

    for op in [
        class_rule("r-llm", "agent:brain", "cred:openrouter", Some("llm")),
        class_rule("r-plain", "agent:plain", "cred:openrouter", None),
        catalogue_row("cred:anthropic", "llm", "active"),
        catalogue_row("cred:chat", "chat", "active"),
        catalogue_row("cred:old", "llm", "retired"),
    ] {
        probe(&h, &mut rx, op).await;
    }
    let brain = credential_grant_for(&h, &mut rx, "agent:brain").await;
    let plain = credential_grant_for(&h, &mut rx, "agent:plain").await;

    // (who, grant, asked, denied?)
    let cases: [(&str, &str, Option<&str>, bool); 6] = [
        ("agent:brain", &brain, Some("cred:anthropic"), false),
        ("agent:brain", &brain, Some("cred:missing"), true),
        ("agent:brain", &brain, Some("cred:chat"), true),
        ("agent:brain", &brain, Some("cred:old"), true),
        ("agent:brain", &brain, None, false),
        ("agent:plain", &plain, Some("cred:anthropic"), true),
    ];
    for (who, grant, asked, denied) in cases {
        h.send(send_json("/spender", &spend(who, grant, asked)))
            .await;
        let ack = turn_json(&recv_route(&mut rx, "ack").await);
        if denied {
            assert_eq!(
                (ack["outcome"].as_str(), ack["reason_code"].as_str()),
                (Some("denied"), Some("cred_ref_not_granted")),
                "{who} asking for {asked:?}: {ack}"
            );
        } else {
            // The test vault is locked, so a delivery comes back as the
            // vault's own refusal -- never as the class refusal.
            assert_ne!(
                ack["reason_code"].as_str(),
                Some("cred_ref_not_granted"),
                "{who} asking for {asked:?}: {ack}"
            );
        }
    }

    let audit = probe(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "audit", "columns": ["detail"],
               "where": {"reason_code": "cred_ref_not_granted"}}),
    )
    .await;
    h.quiesce().await;
    let calls = vault_calls(&h).await;
    h.shutdown().await;

    let names: Vec<&str> = calls
        .iter()
        .filter(|(from, a)| from == BROKER && a["op"] == "deliver")
        .filter_map(|(_, a)| a["name"].as_str())
        .collect();
    assert_eq!(
        names,
        vec!["cred:anthropic", "cred:openrouter"],
        "exactly the class row's credential and the grant's own reached the vault: {calls:?}"
    );

    let mut refused: Vec<String> = audit
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|r| {
            as_obj(&r["detail"])["cred_ref"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    refused.sort();
    assert_eq!(
        refused,
        vec!["cred:anthropic", "cred:chat", "cred:missing", "cred:old"],
        "every class refusal is audited with the name asked for: {audit}"
    );
}
