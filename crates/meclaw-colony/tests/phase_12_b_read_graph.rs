//! Phase 12-B step-7.5: ColonyMsg::ReadGraph — scope-filtered Nodes + Edges.

use meclaw_colony::ColonyMsg;
use meclaw_colony::api_dto::ReadGraphReply;
use meclaw_core::{Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mocks::EchoMockCell;
use tokio::sync::oneshot;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_graph_returns_empty_on_fresh_colony() {
    let h = ColonyHandle::new();
    let (ack_tx, ack_rx) = oneshot::channel::<ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: Path::new("/"),
            ack: ack_tx,
        })
        .await
        .unwrap();
    let reply = ack_rx.await.unwrap();
    assert_eq!(reply.scope, "/");
    assert_eq!(reply.nodes.len(), 0);
    assert_eq!(reply.edges.len(), 0);
    assert_eq!(reply.graph_version, 0);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_graph_returns_nodes_and_edges_inside_scope() {
    let h = ColonyHandle::new();
    h.spawn(Path::new("/a/x"), || EchoMockCell::new(Path::new("/a/x")))
        .await;
    h.spawn(Path::new("/a/y"), || EchoMockCell::new(Path::new("/a/y")))
        .await;
    h.spawn(Path::new("/b/z"), || EchoMockCell::new(Path::new("/b/z")))
        .await;
    h.add_edge(Uuid::now_v7(), Path::new("/a/x"), Path::new("/a/y"))
        .await;
    h.add_edge(Uuid::now_v7(), Path::new("/a/x"), Path::new("/b/z"))
        .await;

    let (ack_tx, ack_rx) = oneshot::channel::<ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: Path::new("/a"),
            ack: ack_tx,
        })
        .await
        .unwrap();
    let reply = ack_rx.await.unwrap();
    assert_eq!(reply.scope, "/a");
    assert_eq!(reply.nodes.len(), 2, "only /a/x and /a/y are inside scope");
    // Edge /a/x -> /a/y is fully inside scope; /a/x -> /b/z crosses scope.
    assert_eq!(reply.edges.len(), 1, "only the intra-scope edge counts");
    assert_eq!(reply.edges[0].from, "/a/x");
    assert_eq!(reply.edges[0].to, "/a/y");
    h.shutdown().await;
}
