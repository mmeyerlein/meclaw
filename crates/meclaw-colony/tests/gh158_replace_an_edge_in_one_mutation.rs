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
        .expect("bootstrap");
    h
}

/// Two edges between the SAME pair, differing in nothing but the phase: one
/// regular, one default. GH #283 — this is the neighbourhood a migration
/// produces, because the natural way to convert a live catch-all is to lay the
/// default beside the unconditional edge it replaces and take the old one away
/// afterwards.
async fn colony_with_a_default_beside_a_regular_edge(td: &TempDir) -> ColonyHandle {
    create_dir_all(td.path().join("main/a")).unwrap();
    create_dir_all(td.path().join("main/b")).unwrap();
    fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"/a","to":"/b"},
            {"from":"/a","to":"/b","default":true}
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
        .expect("bootstrap");
    h
}

/// How many `/a -> /b` edges the graph currently carries.
async fn edges_a_to_b(h: &ColonyHandle) -> usize {
    read_graph_root(h)
        .await
        .edges
        .iter()
        .filter(|e| e.from == "/a" && e.to == "/b")
        .count()
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

/// GH #283 (W4 T4): with the phase part of the edge's identity, a `remove_edges`
/// pattern can NAME one of the two edges between the pair.
///
/// The graph DTO does not carry the phase, so "which one survived" is proved by
/// the validator instead of by inspection: after taking the default away, a
/// second pattern naming the default hits nothing (`match_no_hit`), while a
/// pattern naming the regular phase takes the survivor. Counting alone would
/// pass with a predicate that ignores the flag and removes whichever edge it
/// reaches first.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remove_edges_can_name_the_default_beside_the_regular_edge() {
    let td = TempDir::new().unwrap();
    let h = colony_with_a_default_beside_a_regular_edge(&td).await;
    assert_eq!(
        edges_a_to_b(&h).await,
        2,
        "boot lays BOTH: same from/to/condition/modifier, different phase"
    );

    let take_default = json!({"scope": "/", "diff": {
        "remove_edges": [{"match": {"from": "a", "to": "b", "default": true}}]
    }});
    let take_regular = json!({"scope": "/", "diff": {
        "remove_edges": [{"match": {"from": "a", "to": "b", "default": false}}]
    }});

    let outcome = mutate(&h, take_default.clone()).await;
    assert!(outcome.contains("Committed"), "{outcome}");
    assert_eq!(
        edges_a_to_b(&h).await,
        1,
        "the pattern named ONE of the two, not both"
    );

    // The default is the one that is gone …
    let again = mutate(&h, take_default).await;
    assert!(
        again.contains("match_no_hit"),
        "naming the default again hits nothing — it is the edge that was taken: {again}"
    );
    // … and the survivor is the regular edge.
    let last = mutate(&h, take_regular).await;
    assert!(last.contains("Committed"), "{last}");
    assert_eq!(edges_a_to_b(&h).await, 0, "and now the pair is unwired");
}

/// GH #283 (W4 T4): `match.default` is OPTIONAL, and its absence means
/// unconstrained — the same rule `condition` and `modifier` already follow. A
/// pattern that does not name the phase takes both edges between the pair.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_remove_edges_pattern_without_default_still_takes_both() {
    let td = TempDir::new().unwrap();
    let h = colony_with_a_default_beside_a_regular_edge(&td).await;
    assert_eq!(edges_a_to_b(&h).await, 2);

    let outcome = mutate(
        &h,
        json!({"scope": "/", "diff": {
            "remove_edges": [{"match": {"from": "a", "to": "b"}}]
        }}),
    )
    .await;
    assert!(outcome.contains("Committed"), "{outcome}");
    assert_eq!(
        edges_a_to_b(&h).await,
        0,
        "no `default` key → the phase is unconstrained → both edges go"
    );
}
