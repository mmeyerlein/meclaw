//! Phase 12-B step-7.4: ColonyMsg::ReadMutationsAudit — Read of `mutation_log`.

use meclaw_colony::ColonyMsg;
use meclaw_colony::api_dto::ReadMutationsAuditReply;
use meclaw_testing::ColonyHandle;
use tokio::sync::oneshot;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_mutations_audit_returns_empty_on_fresh_colony() {
    let h = ColonyHandle::new();
    let (ack_tx, ack_rx) = oneshot::channel::<ReadMutationsAuditReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadMutationsAudit {
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
async fn read_mutations_audit_respects_since_filter() {
    let h = ColonyHandle::new();
    let (ack_tx, ack_rx) = oneshot::channel::<ReadMutationsAuditReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadMutationsAudit {
            since: Some(i64::MAX),
            limit: 100,
            ack: ack_tx,
        })
        .await
        .unwrap();
    let reply = ack_rx.await.unwrap();
    assert_eq!(reply.entries.len(), 0, "since=MAX should yield nothing");
    h.shutdown().await;
}
