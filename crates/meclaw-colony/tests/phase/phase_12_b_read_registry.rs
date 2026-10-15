//! Phase 12-B step-7.1: ColonyMsg::ReadRegistry — inbox typed read with a oneshot ack.
//!
//! Verifies the new Read*-arm via `ColonyHandle::new()`: fresh colony, registry empty,
//! ReadRegistry returns an empty snapshot. After spawning a cell, ReadRegistry returns
//! exactly one entry with the expected path + cell_type.

use meclaw_colony::ColonyMsg;
use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_core::Path;
use meclaw_testing::ColonyHandle;
use meclaw_testing::mocks::EchoMockCell;
use tokio::sync::oneshot;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_registry_returns_empty_snapshot_on_fresh_colony() {
    let h = ColonyHandle::new();
    let (ack_tx, ack_rx) = oneshot::channel::<ReadRegistryReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 100,
            ack: ack_tx,
        })
        .await
        .unwrap();
    let reply = ack_rx.await.unwrap();
    assert_eq!(reply.entries.len(), 0);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_registry_lists_spawned_cell() {
    let h = ColonyHandle::new();
    h.spawn(Path::new("/foo"), || EchoMockCell::new(Path::new("/foo")))
        .await;
    let (ack_tx, ack_rx) = oneshot::channel::<ReadRegistryReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 100,
            ack: ack_tx,
        })
        .await
        .unwrap();
    let reply = ack_rx.await.unwrap();
    assert_eq!(reply.entries.len(), 1);
    assert_eq!(reply.entries[0].path, "/foo");
    assert_eq!(reply.entries[0].cell_type, "test-mock");
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_registry_filters_by_exact_path() {
    let h = ColonyHandle::new();
    h.spawn(Path::new("/a"), || EchoMockCell::new(Path::new("/a")))
        .await;
    h.spawn(Path::new("/b"), || EchoMockCell::new(Path::new("/b")))
        .await;
    let (ack_tx, ack_rx) = oneshot::channel::<ReadRegistryReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: Some(Path::new("/a")),
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 100,
            ack: ack_tx,
        })
        .await
        .unwrap();
    let reply = ack_rx.await.unwrap();
    assert_eq!(reply.entries.len(), 1);
    assert_eq!(reply.entries[0].path, "/a");
    h.shutdown().await;
}
