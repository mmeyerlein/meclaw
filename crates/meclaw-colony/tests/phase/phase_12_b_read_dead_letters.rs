//! Phase 12-B step-7.2: ColonyMsg::ReadDeadLetters — a pure read of the DLQ.
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

/// Welle Live, R-L10 (GH #794): the DLQ is a reading instrument, not a queue
/// anybody drains. In an event-driven design an event with no target is the
/// normal case, so the table is meant to grow large — and a reader who caps at
/// `limit` and starts at the OLDEST row reads the colony's first day forever.
/// Without `since` the read therefore answers NEWEST FIRST.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_dead_letters_answers_newest_first_without_since() {
    let h = ColonyHandle::new();
    h.send(MessageBuilder::new(Path::new("/missing-first")).build())
        .await;
    h.send(MessageBuilder::new(Path::new("/missing-second")).build())
        .await;
    h.send(MessageBuilder::new(Path::new("/missing-third")).build())
        .await;

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
    let targets: Vec<&str> = reply
        .entries
        .iter()
        .map(|e| e.original_target.as_str())
        .collect();
    assert_eq!(
        targets,
        vec!["/missing-third", "/missing-second", "/missing-first"],
        "a read without `since` answers newest first"
    );

    // And the cap takes the NEWEST rows, not the oldest ones.
    let (ack_tx, ack_rx) = oneshot::channel::<ReadDeadLettersReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadDeadLetters {
            since: None,
            error_code: None,
            limit: 1,
            ack: ack_tx,
        })
        .await
        .unwrap();
    let capped = ack_rx.await.unwrap();
    assert_eq!(capped.entries.len(), 1);
    assert_eq!(
        capped.entries[0].original_target, "/missing-third",
        "`limit` without `since` keeps the newest rows"
    );
    h.shutdown().await;
}

/// The other half of R-L10: WITH a mark the read walks forward from it, so a
/// watcher that remembers the last `created_at` it saw keeps reading the queue
/// in the order it filled up.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_dead_letters_stays_ascending_from_the_mark() {
    let h = ColonyHandle::new();
    h.send(MessageBuilder::new(Path::new("/missing-first")).build())
        .await;
    h.send(MessageBuilder::new(Path::new("/missing-second")).build())
        .await;

    let (ack_tx, ack_rx) = oneshot::channel::<ReadDeadLettersReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadDeadLetters {
            since: Some(0),
            error_code: None,
            limit: 100,
            ack: ack_tx,
        })
        .await
        .unwrap();
    let reply = ack_rx.await.unwrap();
    let targets: Vec<&str> = reply
        .entries
        .iter()
        .map(|e| e.original_target.as_str())
        .collect();
    assert_eq!(
        targets,
        vec!["/missing-first", "/missing-second"],
        "a read with `since` walks forward from the mark"
    );
    h.shutdown().await;
}
