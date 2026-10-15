//! Phase 12-B step-7.6: ColonyMsg::ReadTrace — spawn_blocking + WAL Read-Only Connection.
//!
//! `ReadTrace` opens a fresh `SQLITE_OPEN_READ_ONLY` Connection on `colony.db` inside
//! `tokio::task::spawn_blocking`. WAL allows concurrent readers, so the colony's
//! writer thread is unaffected. Since GH #683 (ADR-0041) the inbox arm spawns
//! the read into a task of its own and parks; the reply is sent from that task
//! (bounded by limit ≤ 1000).

use meclaw_colony::ColonyMsg;
use meclaw_colony::api_dto::ReadTraceReply;
use meclaw_core::{MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mocks::EchoMockCell;
use tokio::sync::oneshot;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_trace_returns_empty_on_fresh_colony() {
    let h = ColonyHandle::new();
    let (ack_tx, ack_rx) = oneshot::channel::<ReadTraceReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadTrace {
            trace_id: None,
            path_prefix: None,
            correlation_id: None,
            only_error: false,
            since: None,
            limit: 100,
            wait: None,
            ack: ack_tx,
        })
        .await
        .unwrap();
    let reply = ack_rx.await.unwrap();
    assert_eq!(reply.entries.len(), 0);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_trace_returns_routed_messages() {
    let h = ColonyHandle::new();
    h.spawn(Path::new("/x"), || EchoMockCell::new(Path::new("/x")))
        .await;
    // Route a couple of messages — each produces a message_log row.
    h.send(MessageBuilder::new(Path::new("/x")).build()).await;
    h.send(MessageBuilder::new(Path::new("/x")).build()).await;
    // Need to flush the writer thread before the read sees the rows; the
    // simplest barrier is shutdown(), but we want to read while colony is
    // still alive. Issue a synchronous round-trip via Drain (which goes
    // through the inbox-loop and gives the writer thread a chance to commit).
    let _ = h.drain_dead_letters().await;

    // Allow a brief flush window — writer thread batches in 50ms cycles.
    // Re-read in a tiny retry loop to avoid flake; bounded by 1s.
    let mut last_len = 0usize;
    for _ in 0..20 {
        let (ack_tx, ack_rx) = oneshot::channel::<ReadTraceReply>();
        h.inbox_tx
            .send(ColonyMsg::ReadTrace {
                trace_id: None,
                path_prefix: None,
                correlation_id: None,
                only_error: false,
                since: None,
                limit: 100,
                wait: None,
                ack: ack_tx,
            })
            .await
            .unwrap();
        let reply = ack_rx.await.unwrap();
        last_len = reply.entries.len();
        if last_len >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(last_len >= 2, "expected ≥2 routed messages, got {last_len}");
    h.shutdown().await;
}

/// One `ReadTrace` over the whole log, with an optional wait.
async fn read_trace_waiting(
    h: &ColonyHandle,
    wait: Option<meclaw_colony::api_dto::TraceWait>,
) -> ReadTraceReply {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadTraceReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadTrace {
            trace_id: None,
            path_prefix: None,
            correlation_id: None,
            only_error: false,
            since: None,
            limit: 1000,
            wait,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap()
}

/// GH #1099: a waiting read is held until the writer commits past the
/// caller's `log_seq`, and answers right after that commit -- an event, so
/// the reader learns of a new row within milliseconds instead of on its next
/// poll tick (the `meclaw ask` loop used to sleep 500 ms between reads).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1099_a_waiting_read_answers_right_after_the_commit() {
    let h = ColonyHandle::new();
    h.spawn(Path::new("/x"), || EchoMockCell::new(Path::new("/x")))
        .await;
    let mut lags = Vec::new();
    for round in 0..5 {
        // Settle: wait until the previous round's commits have all landed
        // (a held read that times out on an unchanged `log_seq`).
        let mut before = read_trace_waiting(&h, None).await;
        loop {
            let next = read_trace_waiting(
                &h,
                Some(meclaw_colony::api_dto::TraceWait {
                    after_seq: before.log_seq,
                    max: std::time::Duration::from_millis(150),
                }),
            )
            .await;
            let settled = next.log_seq == before.log_seq;
            before = next;
            if settled {
                break;
            }
        }
        let seen = before.entries.len();
        let wait = meclaw_colony::api_dto::TraceWait {
            after_seq: before.log_seq,
            max: std::time::Duration::from_secs(10),
        };
        let (done_tx, mut done_rx) = oneshot::channel();
        let h_inbox = h.inbox_tx.clone();
        let reader = tokio::spawn(async move {
            let (ack_tx, ack_rx) = oneshot::channel::<ReadTraceReply>();
            h_inbox
                .send(ColonyMsg::ReadTrace {
                    trace_id: None,
                    path_prefix: None,
                    correlation_id: None,
                    only_error: false,
                    since: None,
                    limit: 1000,
                    wait: Some(wait),
                    ack: ack_tx,
                })
                .await
                .unwrap();
            let reply = ack_rx.await.unwrap();
            let _ = done_tx.send(std::time::Instant::now());
            reply
        });
        // Nothing was committed: the read is still held.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(
            done_rx.try_recv().is_err(),
            "round {round}: a waiting read must not answer before the log moves"
        );
        let sent = std::time::Instant::now();
        h.send(MessageBuilder::new(Path::new("/x")).build()).await;
        let reply = reader.await.unwrap();
        let answered = done_rx.await.unwrap();
        assert!(
            reply.entries.len() > seen,
            "round {round}: the woken read sees the row that woke it"
        );
        assert_ne!(reply.log_seq, before.log_seq);
        lags.push(answered - sent);
    }
    lags.sort();
    let median = lags[lags.len() / 2];
    assert!(
        median < std::time::Duration::from_millis(50),
        "the read must answer within 50 ms of the commit; lags {lags:?}"
    );
    h.shutdown().await;
}

/// GH #1099 (review fix1): a held read whose caller hung up ends at once. The
/// task used to sit out its whole `max` for nobody -- here 60 s, fifty of
/// them -- holding a log subscription each; now it watches `ack.closed()`
/// beside the wait. Counted as alive runtime tasks before and after.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1099_a_held_read_ends_when_its_caller_hangs_up() {
    let h = ColonyHandle::new();
    let before = read_trace_waiting(&h, None).await;
    let metrics = tokio::runtime::Handle::current().metrics();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let baseline = metrics.num_alive_tasks();
    const READS: usize = 50;
    for _ in 0..READS {
        let (ack_tx, ack_rx) = oneshot::channel::<ReadTraceReply>();
        h.inbox_tx
            .send(ColonyMsg::ReadTrace {
                trace_id: None,
                path_prefix: None,
                correlation_id: None,
                only_error: false,
                since: None,
                limit: 10,
                wait: Some(meclaw_colony::api_dto::TraceWait {
                    after_seq: before.log_seq,
                    max: std::time::Duration::from_secs(60),
                }),
                ack: ack_tx,
            })
            .await
            .unwrap();
        drop(ack_rx);
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let alive = metrics.num_alive_tasks();
        if alive < baseline + READS / 5 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "held reads outlive their hung-up callers: {alive} alive tasks, {baseline} before"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    h.shutdown().await;
}
