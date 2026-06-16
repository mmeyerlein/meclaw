//! Phase 12-B step-7.6: ColonyMsg::ReadTrace — spawn_blocking + WAL Read-Only Connection.
//!
//! `ReadTrace` opens a fresh `SQLITE_OPEN_READ_ONLY` Connection on `colony.db` inside
//! `tokio::task::spawn_blocking`. WAL allows concurrent readers, so the colony's
//! writer thread is unaffected. The Inbox-arm awaits the JoinHandle before
//! sending the reply — this STALLS the Colony-Inbox loop for the query duration
//! (bounded by limit ≤ 1000). Off-Loop-Reads = Phase 14.

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
