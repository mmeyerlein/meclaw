//! Phase 12-B step-7.2: ColonyMsg::ReadDeadLetters — pure-Read der DLQ.
//!
//! `DrainDeadLetters` (Phase-2) consumes the queue; `ReadDeadLetters` returns
//! a filtered snapshot without removing entries. HTTP-handlers in Task 8 use
//! Read for `/colony/dead_letters` GET; Drain stays for the DELETE-path.

use meclaw_colony::ColonyMsg;
use meclaw_colony::api_dto::ReadDeadLettersReply;
use meclaw_core::{MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use tokio::sync::oneshot;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_dead_letters_returns_empty_on_fresh_colony() {
    let h = ColonyHandle::new();
    let (ack_tx, ack_rx) = oneshot::channel::<ReadDeadLettersReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadDeadLetters {
            since: None,
            error_code: None,
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
async fn read_dead_letters_returns_snapshot_without_draining() {
    let h = ColonyHandle::new();
    // Inject 2 dead letters by routing to unresolved targets.
    h.send(MessageBuilder::new(Path::new("/missing-a")).build())
        .await;
    h.send(MessageBuilder::new(Path::new("/missing-b")).build())
        .await;

    // First read — snapshot of 2.
    let (ack_tx, ack_rx) = oneshot::channel::<ReadDeadLettersReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadDeadLetters {
            since: None,
            error_code: None,
            limit: 100,
            ack: ack_tx,
        })
        .await
        .unwrap();
    let r1 = ack_rx.await.unwrap();
    assert_eq!(r1.entries.len(), 2);

    // Second read — STILL 2 (no draining).
    let (ack_tx, ack_rx) = oneshot::channel::<ReadDeadLettersReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadDeadLetters {
            since: None,
            error_code: None,
            limit: 100,
            ack: ack_tx,
        })
        .await
        .unwrap();
    let r2 = ack_rx.await.unwrap();
    assert_eq!(r2.entries.len(), 2);
    assert_eq!(r2.entries[0].error_code, "unresolved_path");
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_dead_letters_filters_by_error_code() {
    let h = ColonyHandle::new();
    h.send(MessageBuilder::new(Path::new("/missing")).build())
        .await;
    let (ack_tx, ack_rx) = oneshot::channel::<ReadDeadLettersReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadDeadLetters {
            since: None,
            error_code: Some("ttl_expired".into()),
            limit: 100,
            ack: ack_tx,
        })
        .await
        .unwrap();
    let reply = ack_rx.await.unwrap();
    assert_eq!(reply.entries.len(), 0, "no DLs match ttl_expired filter");
    h.shutdown().await;
}
