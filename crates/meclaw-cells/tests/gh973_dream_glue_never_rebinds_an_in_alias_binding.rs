//! GH #973 M-B1 -- the night never bends a binding.
//!
//! `dream-glue` wrote every alias of the identity round as a plain `set_alias`,
//! an upsert: a spelling a member had bound on `in_alias` to an id moved to
//! whatever the judge named, and every fact of that spelling with it. And the
//! resolution of a fact is one hop, never transitive, so an alias written onto
//! a spelling that is itself an alias hung on a non-canonical identity.
//!
//! Measured at the receiver: the binding is made on the booted hive through
//! `in_alias` (the writer's lane), the round's ops come out of the shipped
//! `dream-glue` script for a judgement that contradicts it, and they land in
//! the colony's own `cell.db` through the store's own dispatcher
//! (`support::store_op`). The table says what holds: the member's binding,
//! and the judged alias written onto the canonical end of its target.

#[path = "support/memory_hive_subject.rs"]
mod support;

use meclaw_core::serde_json::{Value, json};
use support::*;

const GLUE: &str = "../../templates/memory-hive/dream-glue/config.json";

/// The ops the shipped `dream-glue` emits for the judge's answer `text`.
fn round_of(text: &str) -> Vec<Value> {
    let doc = json!({
        "header": {
            "context": {"store_origin": "dream", "mem_phase": "canon-judged",
                        "dream_run": "r-973", "dream_to": "2026-10-03T03:00:00Z"},
            "hop": {"finish_reason": "stop"}
        },
        "messages": [{"origin": "assistant", "type": "text", "text": text}]
    });
    let out = meclaw_testing::run_shipped_script(
        &meclaw_testing::shipped_script(GLUE),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    assert!(
        out.status.success(),
        "dream-glue: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let msgs: Vec<Value> =
        meclaw_core::serde_json::from_slice(&out.stdout).expect("a message array");
    msgs.iter()
        .filter_map(|m| m["messages"][0]["text"].as_str())
        .filter_map(|t| meclaw_core::serde_json::from_str::<Value>(t).ok())
        .filter(|a| a["operation"] == json!("set_alias"))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dream_glue_never_rebinds_an_in_alias_binding() {
    if !shipped() {
        eprintln!("gh973: the memory hive is not in this tree, skipped (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    build(
        &td,
        &[
            fact(
                "f-1",
                "Firewall X",
                "firewall x",
                ROUND_EA,
                "2026-09-01T10:00:00Z",
            ),
            fact(
                "f-2",
                "the box",
                "the box",
                ROUND_EB,
                "2026-09-02T10:00:00Z",
            ),
        ],
    );
    let (h, mut rx) = boot(&td).await;
    let mut seen = Vec::new();
    let db = db(&td);
    // A member states what "Firewall X" is.
    h.send(in_alias(
        json!({"aliases": [{"alias": "Firewall X", "canonical": "ob-1"}]}),
        "t-1",
    ))
    .await;
    let a = ack(&mut rx, &mut seen, "t-1").await;
    assert_eq!(body_of(&a)["done"], json!(1));
    h.shutdown().await;

    // The night judges otherwise: "Firewall X" is "the box", and "the box" is
    // "Firewall X" -- the second a chain onto the member's binding.
    let ops = round_of(
        r#"{"entities": [{"alias": "Firewall X", "canonical": "the box"},
                         {"alias": "the box", "canonical": "Firewall X"}]}"#,
    );
    // Inside the verdict the two are a cycle: neither link goes out.
    assert_eq!(
        ops,
        Vec::<Value>::new(),
        "a cycle of one verdict writes none of its links"
    );

    // One at a time, as two nights would judge them.
    let first = round_of(r#"{"entities": [{"alias": "Firewall X", "canonical": "the box"}]}"#);
    let second = round_of(r#"{"entities": [{"alias": "the box", "canonical": "Firewall X"}]}"#);
    for op in first.iter().chain(&second) {
        assert_eq!(
            (op["if_absent"].clone(), op["resolve"].clone()),
            (json!(true), json!(true)),
            "every alias of the round is guarded and resolved: {op}"
        );
    }
    assert_eq!((first.len(), second.len()), (1, 1));
    let bent = store_op(&db, first[0].clone());
    assert_eq!(bent.error_code, None);
    assert_eq!(bent.rows_affected, 0, "the member's binding is not bent");
    assert_eq!(bent.payload["conflict"], json!(true));
    let chained = store_op(&db, second[0].clone());
    assert_eq!(chained.error_code, None);
    assert_eq!(chained.rows_affected, 1);
    assert_eq!(
        rows(
            &db,
            "SELECT alias, canonical FROM subject_aliases ORDER BY alias"
        ),
        vec![
            vec!["firewall x".to_string(), "ob-1".to_string()],
            vec!["the box".to_string(), "ob-1".to_string()],
        ],
        "the binding holds, and the judged alias lands on the canonical end of \
         its target's chain, not on a spelling that is itself an alias"
    );
}
