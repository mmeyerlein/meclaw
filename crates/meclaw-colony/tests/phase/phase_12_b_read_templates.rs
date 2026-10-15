//! Phase 12-B step-7.3: ColonyMsg::ReadTemplates — sync-Read via colony_db.

use meclaw_colony::ColonyMsg;
use meclaw_colony::api_dto::ReadTemplatesReply;
use meclaw_testing::ColonyHandle;
use tokio::sync::oneshot;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_templates_returns_empty_on_fresh_colony() {
    let h = ColonyHandle::new();
    let (ack_tx, ack_rx) = oneshot::channel::<ReadTemplatesReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadTemplates {
            cell_type: None,
            name: None,
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
async fn read_templates_filters_by_name() {
    let h = ColonyHandle::new();
    let (ack_tx, ack_rx) = oneshot::channel::<ReadTemplatesReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadTemplates {
            cell_type: None,
            name: Some("nonexistent".into()),
            limit: 100,
            ack: ack_tx,
        })
        .await
        .unwrap();
    let reply = ack_rx.await.unwrap();
    assert_eq!(reply.entries.len(), 0);
    h.shutdown().await;
}
