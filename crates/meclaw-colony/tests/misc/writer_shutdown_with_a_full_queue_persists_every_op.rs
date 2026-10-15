//! Lock test: `ColonyDb::shutdown_async` with a full writer queue persists
//! every queued op before it returns.
//!
//! The full queue is made deterministic instead of probable (the 5000-op burst
//! in `phase_12_pre_bounded_writer` only hits it by chance): a second
//! connection holds `BEGIN EXCLUSIVE`, so the writer's first transaction waits
//! in the busy handler (`DB_BUSY_TIMEOUT`, 30 s) while 1001 ops pile up behind
//! it — the writer's first batch in its hands, the rest in the bounded channel
//! (cap 1000). The shutdown op does not fit in the full channel: it waits its
//! turn behind the queued ops (`send().await`, never a drop-only fallback —
//! the writer runs on the runtime, there is no thread to join, ADR-0051), and
//! the ack comes after the drain. Every queued op must be persisted.
//!
//! Two claims. First: the shutdown is not done while the lock still holds the
//! writer. This is a state claim, not a time window (OR-TV-3): the test polls
//! the shutdown future itself, exactly once, while it holds `BEGIN EXCLUSIVE`.
//! At that moment the writer holds one op it cannot commit and the channel
//! holds 1000 (all 1001 `send_op` calls returned, so the writer took the
//! first one), so a correct shutdown has to be `Pending`: its op does not fit
//! in the channel, and its ack only comes after a commit the lock forbids. A
//! shutdown that gives up on the full channel and returns before the drain is
//! `Ready` at that first poll, independent of machine speed. Second: once the
//! lock is released the same future finishes within the 30 s failure marker
//! with all 1001 rows in `registry`. Green on the current writer; a shutdown
//! that returns before the drain, or loses the buffered ops, turns it red.

use std::future::Future;
use std::time::Duration;

use meclaw_colony::persist::colony_db::ColonyDb;
use meclaw_colony::persist::writer::ColonyWriteOp;
use meclaw_core::Path;

/// One op the writer holds plus a channel filled to its cap of 1000.
const OPS: usize = 1001;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn writer_shutdown_with_a_full_queue_persists_every_op() {
    let td = tempfile::TempDir::new().unwrap();
    let db_path = td.path().join("c.db");
    let db = ColonyDb::open(&db_path).unwrap();

    // A second connection takes the write lock; the writer's first
    // transaction now waits in its busy handler.
    let locker = rusqlite::Connection::open(&db_path).unwrap();
    locker.execute_batch("BEGIN EXCLUSIVE").unwrap();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    for i in 0..OPS {
        tokio::time::timeout(
            Duration::from_secs(30),
            db.send_op(ColonyWriteOp::UpsertRegistry {
                path: Path::new(&format!("/shutdown-full/{i}")),
                cell_id: format!("{i:032x}"),
                cell_type: "test".to_string(),
                created_at: now,
                updated_at: now,
            }),
        )
        .await
        .unwrap_or_else(|_| panic!("send_op {i} did not get a queue slot within 30 s"));
    }

    // The test task drives the shutdown future itself, so it can poll it once
    // while the lock is held and keep driving the same future afterwards.
    let mut shutdown = std::pin::pin!(db.shutdown_async());

    // State discriminator, no clock: the writer is blocked on the lock with
    // one op, the channel is at its cap of 1000. Nothing a correct shutdown
    // waits for can happen before `ROLLBACK`, so the first poll is `Pending`;
    // a shutdown that skips the drain on a full channel is `Ready` here.
    let first_poll =
        std::future::poll_fn(|cx| std::task::Poll::Ready(shutdown.as_mut().poll(cx))).await;
    assert!(
        first_poll.is_pending(),
        "shutdown_async returned while the writer was still blocked on the lock \
         with {OPS} ops queued — it did not wait for the drain"
    );

    locker.execute_batch("ROLLBACK").unwrap();

    tokio::time::timeout(Duration::from_secs(30), shutdown)
        .await
        .expect("shutdown_async did not finish within 30 s after the lock was released");

    let cnt: i64 = locker
        .query_row("SELECT COUNT(*) FROM registry", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        cnt, OPS as i64,
        "expected {OPS} rows in registry after the shutdown drain, got {cnt}"
    );
}
