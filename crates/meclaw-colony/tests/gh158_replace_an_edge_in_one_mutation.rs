//! Replacing an edge in ONE mutation (GitHub #158).
//!
//! Widening a port is an everyday change: the lane already exists, and it has
//! to promote one more hop key into context. The obvious spelling is "remove
//! the old edge, add the new one", in one mutation, so the lane is never
//! missing in between.
//!
//! That spelling used to delete the lane. `add_edges` ran first, then
//! `remove_edges` — and a match pattern of `{from, to}` matches EVERY edge
//! between the pair, including the one the same diff had just inserted. The
//! mutation reported `committed` and left the colony with nothing.
//!
//! The *mutation* was the silent part: it committed. The traffic afterwards was
//! not — a cell emission that matches no out-edge dead-letters as `no_route`
//! (Ruling A1), which is exactly what happened and exactly what should. The
//! receipt says "committed" and the DLQ says "nothing to route this to"; the
//! two disagree, and only one of them is right.
//!
//! Both tests below would pass with the old order too — if they only checked
//! the outcome. They check the resulting graph.

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

/// One edge `/a -> /b`, carrying `condition` and `modifier` so the replacement
/// has something to differ in.
async fn colony_with_one_edge(td: &TempDir) -> ColonyHandle {
    create_dir_all(td.path().join("main/a")).unwrap();
    create_dir_all(td.path().join("main/b")).unwrap();
    fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"/a","to":"/b","condition":"hop.route == 'go'",
             "modifier":{"set_context":{"asker":"hop.who"}}}
        ]}}}"#,
    )
    .unwrap();
    fs::write(
        td.path().join("main/a/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/b"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    fs::write(
        td.path().join("main/b/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/a"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    let h = ColonyHandle::new();
    bootstrap_from_filesystem(td.path(), &factories_with_echo(), &h.runtime())
        .await
        .expect("bootstrap");
    h
}

async fn mutate(h: &ColonyHandle, payload: meclaw_core::serde_json::Value) -> String {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    format!("{:?}", ack_rx.await.unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_mutation_replaces_an_edge_instead_of_deleting_it() {
    let td = TempDir::new().unwrap();
    let h = colony_with_one_edge(&td).await;

    let outcome = mutate(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "remove_edges": [{"match": {"from": "a", "to": "b"}}],
                "add_edges": [{
                    "from": "a", "to": "b",
                    "condition": "hop.route == 'go'",
                    // The whole point of the replacement: one more key promoted.
                    "modifier": {"set_context": {"asker": "hop.who",
                                                 "participants": "hop.participants"}}
                }]
            }
        }),
    )
    .await;
    assert!(
        outcome.contains("Committed"),
        "the replacement commits: {outcome}"
    );

    let post = read_graph_root(&h).await;
    let edges: Vec<_> = post
        .edges
        .iter()
        .filter(|e| e.from == "/a" && e.to == "/b")
        .collect();
    assert_eq!(
        edges.len(),
        1,
        "exactly ONE edge survives the replacement — not zero: {:?}",
        post.edges
    );
    let m = edges[0]
        .modifier
        .as_ref()
        .map(|v| v.to_string())
        .unwrap_or_default();
    assert!(
        m.contains("participants"),
        "and it is the NEW edge, the one with the widened promotion: {m}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removing_and_adding_unrelated_edges_still_does_both() {
    // The order change must not turn `remove_edges` into "remove what was there
    // before the diff": a diff that removes one lane and opens a different one
    // has to do both, and the removal must still hit the pre-existing edge.
    let td = TempDir::new().unwrap();
    let h = colony_with_one_edge(&td).await;

    let outcome = mutate(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "remove_edges": [{"match": {"from": "a", "to": "b"}}],
                "add_edges": [{"from": "b", "to": "a",
                               "condition": "hop.route == 'back'"}]
            }
        }),
    )
    .await;
    assert!(outcome.contains("Committed"), "{outcome}");

    let post = read_graph_root(&h).await;
    assert!(
        !post.edges.iter().any(|e| e.from == "/a" && e.to == "/b"),
        "the old lane is gone: {:?}",
        post.edges
    );
    assert!(
        post.edges.iter().any(|e| e.from == "/b" && e.to == "/a"),
        "the new lane exists: {:?}",
        post.edges
    );
}
