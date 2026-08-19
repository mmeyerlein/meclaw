//! GH #257 — the header mirror must read a diff the way the apply arm applies
//! it: `remove_edges` BEFORE `add_edges`.
//!
//! GH #158 settled the order for the executor. Replacing an edge — drop the old
//! one, lay a new one with one more key promoted — belongs in ONE mutation so
//! the lane is never missing in between, and that only works if the removal
//! pattern is read against the edges that were there BEFORE the diff. The apply
//! arm has run `remove_edges` first ever since.
//!
//! The header mirror in `mutation::header_views` — the projection the 14-B
//! locality check is fed from — kept the old order. It laid the new edge and
//! then let the removal pattern (`{from, to}`, the ordinary spelling, which
//! matches every edge between the pair) take it away again. So the check saw a
//! post-state in which the lane simply did not exist, and refused the mutation
//! for the key the removed lane used to promote:
//!
//! ```text
//! node '/b' requires consumes.context 'chat_id' but context presence not reachable …
//! ```
//!
//! A false refusal, and one that names the wrong thing: the edge it complains
//! about is the edge the diff creates, and that edge is fine. The workaround —
//! spell the old modifier into the match pattern — asks a caller to know that a
//! validator and an executor disagree about order.
//!
//! The two tests below pin both directions: the replacement is accepted, and
//! the removal still removes what was there before.

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

fn cell(root: &std::path::Path, rel: &str, emitted_target: &str, consumes: &str) {
    create_dir_all(root.join(rel)).unwrap();
    fs::write(
        root.join(rel).join("config.json"),
        format!(
            r#"{{"cell":{{"type":"echo"}},"params":{{"emitted_target":"{emitted_target}"}},
                "contract":{{"version":"0.1.0","settings":{{}},"consumes":{consumes}}}}}"#
        ),
    )
    .unwrap();
}

/// `/a -> /b -> /c`. The first edge promotes `chat_id` into context; `/b`
/// requires it. The second edge exists only so `/b` keeps taking part in the
/// post-state view after the first one is momentarily gone — without it the
/// participation filter would drop `/b` and hide the defect.
async fn colony_with_a_promoting_lane(td: &TempDir) -> ColonyHandle {
    fs::create_dir_all(td.path().join("main")).unwrap();
    fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"/a","to":"/b","condition":"hop.route == 'go'",
             "modifier":{"set_context":{"chat_id":"hop.chat"}}},
            {"from":"/b","to":"/c"}
        ]}}}"#,
    )
    .unwrap();
    cell(td.path(), "main/a", "/b", "{}");
    cell(
        td.path(),
        "main/b",
        "/c",
        r#"{"context":{"chat_id":{"type":"string","required":true}}}"#,
    );
    cell(td.path(), "main/c", "/a", "{}");
    let h = ColonyHandle::new();
    bootstrap_from_filesystem(td.path(), &factories_with_echo(), &h.runtime())
        .await
        .expect("bootstrap");
    h
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

/// The pin: ONE mutation, removing `/a -> /b` by the ordinary `{from, to}`
/// pattern and laying it again with a widened promotion. The header mirror must
/// see the new edge, because the apply arm will.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replaced_edge_is_not_deleted_by_the_header_mirror() {
    let td = TempDir::new().unwrap();
    let h = colony_with_a_promoting_lane(&td).await;

    let outcome = mutate(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "remove_edges": [{"match": {"from": "a", "to": "b"}}],
                "add_edges": [{
                    "from": "a", "to": "b",
                    "condition": "hop.route == 'go'",
                    "modifier": {"set_context": {"chat_id": "hop.chat",
                                                 "participants": "hop.participants"}}
                }]
            }
        }),
    )
    .await;
    assert!(
        outcome.contains("Committed"),
        "the replacement keeps the promoting lane and must commit: {outcome}"
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
        "exactly ONE edge survives the replacement: {:?}",
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

/// The counter-direction: reading the removal against the pre-state must not
/// turn it into a no-op. A diff that takes the promoting lane away WITHOUT
/// putting one back is a real defect and still has to be refused — by the rule
/// it actually breaks, not by an artefact of the mirror.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removing_the_only_promoting_lane_is_still_refused() {
    let td = TempDir::new().unwrap();
    let h = colony_with_a_promoting_lane(&td).await;

    let outcome = mutate(
        &h,
        json!({
            "scope": "/",
            "diff": {"remove_edges": [{"match": {"from": "a", "to": "b"}}]}
        }),
    )
    .await;
    assert!(
        outcome.contains("Rejected"),
        "the lane that promotes the required key cannot just go: {outcome}"
    );
    assert!(
        outcome.contains("chat_id"),
        "and the refusal names the key that is no longer reachable: {outcome}"
    );

    let post = read_graph_root(&h).await;
    assert!(
        post.edges.iter().any(|e| e.from == "/a" && e.to == "/b"),
        "a rejected mutation leaves the graph untouched: {:?}",
        post.edges
    );
}
