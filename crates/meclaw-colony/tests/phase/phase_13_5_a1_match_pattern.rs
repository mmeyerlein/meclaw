//! Phase-13.5-A1-F6: the remove_edges match pattern for condition/modifier is
//! string equality (not semantic). Two edges with the same from/to but different
//! conditions: only the one with the exactly matching condition source is
//! removed by remove_edges.
//!
//! Spec anchor: docs/meclaw-overview.md Z.253 ("by properties
//! (from/to/condition/modifier)"). The F6 pin unit tests already live in
//! `crates/meclaw-colony/src/cel_eval.rs::tests` (T9, commit 9133b14) for
//! `CompiledCondition.source` + `CompiledModifier.source` preservation; this
//! integration test proves the match-pattern discipline in the
//! `handle_mutation::remove_edges` apply path + the DTO exposure via
//! `ColonyMsg::ReadGraph`.

use meclaw_colony::api_dto::ReadGraphReply;
use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, bootstrap_from_filesystem};
use meclaw_core::{Path, Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use std::fs::{self, create_dir_all};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::oneshot;

fn factories_with_echo() -> CellFactoryRegistry {
    let mut r: CellFactoryRegistry = CellFactoryRegistry::new();
    let echo: Arc<dyn CellFactory> = Arc::new(EchoCellFactory);
    r.insert("echo".into(), echo);
    r
}

async fn read_graph_root(h: &ColonyHandle) -> ReadGraphReply {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: Path::new("/"),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remove_edges_match_pattern_string_equality_on_condition_f6() {
    let td = TempDir::new().unwrap();
    create_dir_all(td.path().join("main/a")).unwrap();
    create_dir_all(td.path().join("main/b")).unwrap();

    // Two edges from /a to /b with different conditions — duplicates with
    // different conditions are allowed per spec Z.253.
    fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"/a","to":"/b","condition":"hop.x == 'y'"},
            {"from":"/a","to":"/b","condition":"hop.x == 'z'"}
        ]}}}"#,
    )
    .unwrap();
    fs::write(
        td.path().join("main/a/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/b"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    fs::write(
        td.path().join("main/b/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/a"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    let h = ColonyHandle::new();
    bootstrap_from_filesystem(td.path(), &factories_with_echo(), &h.runtime())
        .await
        .expect("bootstrap with duplicate-condition edges succeeds");

    // Pre-Mutation: read_graph shows 2 edges /a -> /b with the two distinct
    // condition sources.
    let pre = read_graph_root(&h).await;
    let pre_edges: Vec<_> = pre
        .edges
        .iter()
        .filter(|e| e.from == "/a" && e.to == "/b")
        .collect();
    assert_eq!(pre_edges.len(), 2, "pre-mutation: 2 edges /a -> /b");
    let mut pre_conditions: Vec<&str> = pre_edges
        .iter()
        .map(|e| e.condition.as_deref().unwrap_or(""))
        .collect();
    pre_conditions.sort();
    assert_eq!(
        pre_conditions,
        vec!["hop.x == 'y'", "hop.x == 'z'"],
        "F6-PIN: DTO exposes condition source strings for match-pattern equality"
    );

    // Mutation: remove_edges with match-pattern condition="hop.x == 'y'".
    // Names in remove_edges-match are scope-relative (Phase-6 convention via
    // `resolve_scoped_path`); use "a"/"b" here against scope "/" → /a, /b.
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: json!({
                "scope": "/",
                "diff": {
                    "remove_edges": [{
                        "match": {
                            "from": "a",
                            "to": "b",
                            "condition": "hop.x == 'y'"
                        }
                    }]
                }
            }),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    let outcome = ack_rx.await.unwrap();
    assert!(
        matches!(outcome, meclaw_colony::MutationOutcome::Committed { .. }),
        "remove_edges with condition-match-pattern committed; got {outcome:?}"
    );

    // Post-Mutation: exactly 1 edge /a -> /b survives, the one with the
    // non-matching condition source.
    let post = read_graph_root(&h).await;
    let post_edges: Vec<_> = post
        .edges
        .iter()
        .filter(|e| e.from == "/a" && e.to == "/b")
        .collect();
    assert_eq!(
        post_edges.len(),
        1,
        "F6-PIN: only the edge with exact-string-matching condition got removed"
    );
    let remaining_cond = post_edges[0].condition.as_deref();
    assert_eq!(
        remaining_cond,
        Some("hop.x == 'z'"),
        "F6-PIN: condition string equality — the 'y' one was removed, 'z' remains"
    );

    h.shutdown().await;
}
