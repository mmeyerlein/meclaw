//! P1 (message browser): `ColonyMsg::ReadMessages` is answered over the Colony
//! inbox, exactly like the `ReadTrace` arm it mirrors.
//!
//! The arm itself carries no logic — it forwards the filter to
//! `colony_dispatch::handle_read_messages` and sends the reply back. This test
//! pins that the wiring exists and the ack resolves; the query semantics are
//! covered by the `colony_dispatch` unit tests.

use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_messages_inbox_arm_answers_with_reply() {
    let h = meclaw_testing::ColonyHandle::new();
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    let filter = meclaw_colony::api_dto::MessageLogFilter {
        limit: 10,
        scan_budget: 5000,
        ..Default::default()
    };
    h.inbox_tx
        .send(meclaw_colony::ColonyMsg::ReadMessages {
            filter,
            ack: ack_tx,
        })
        .await
        .expect("inbox alive");

    let reply = tokio::time::timeout(Duration::from_secs(30), ack_rx)
        .await
        .expect("no timeout")
        .expect("ack delivered");

    assert!(
        reply.entries.is_empty(),
        "a fresh colony has routed no messages yet"
    );
    assert!(reply.next.is_none());
    assert!(!reply.scan_truncated);
    assert_eq!(reply.scan_budget, 5000, "post-clamp budget is echoed back");
}

/// The limit is clamped defensively in the dispatch helper, not trusted from the
/// caller — an absurd request must not become an unbounded read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_messages_inbox_arm_clamps_absurd_budget() {
    let h = meclaw_testing::ColonyHandle::new();
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    let filter = meclaw_colony::api_dto::MessageLogFilter {
        limit: usize::MAX,
        scan_budget: usize::MAX,
        ..Default::default()
    };
    h.inbox_tx
        .send(meclaw_colony::ColonyMsg::ReadMessages {
            filter,
            ack: ack_tx,
        })
        .await
        .expect("inbox alive");

    let reply = tokio::time::timeout(Duration::from_secs(30), ack_rx)
        .await
        .expect("no timeout")
        .expect("ack delivered");
    assert_eq!(
        reply.scan_budget, 50_000,
        "scan budget is capped server-side, never unbounded"
    );
}
