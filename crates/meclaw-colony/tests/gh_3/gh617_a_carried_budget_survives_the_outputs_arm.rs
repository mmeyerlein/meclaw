//! GH #617 — the outputs arm re-stamps the budget of a parentless emission
//! (`colony.rs:3466`). A cell declaring `contract.ingress.carries_trace` is the one
//! exception: a cycle across a colony boundary must die on what it set out with
//! rather than buy 64 fresh hops per colony. Twin assertion: nothing else moved.

use meclaw_colony::mutation::validate::HeaderNodeView;
use meclaw_colony::{ColonyMsg, NodeContract};
use meclaw_core::serde_json::json;
use meclaw_core::{CellEmission, CellOutput, Headers, Message, OriginSink, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::time::Duration;
use tokio::sync::mpsc;

/// A colony with a capturing `/sink`, an edge `/peer -> /sink`, and `/peer`'s
/// node contract carrying `carries_trace`. The colony default budget is 64.
async fn colony_with_peer(carries_trace: bool) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let h = ColonyHandle::new();
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    h.add_edge(Uuid::now_v7(), Path::new("/peer"), Path::new("/sink"))
        .await;
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::SetNodeContract {
            path: Path::new("/peer"),
            contract: NodeContract {
                header_view: HeaderNodeView {
                    ingress_carries_trace: carries_trace,
                    ..HeaderNodeView::default()
                },
                emits: None,
                validate_emits: false,
            },
            ack: ack_tx,
        })
        .await
        .expect("colony inbox open");
    tokio::time::timeout(Duration::from_secs(30), ack_rx)
        .await
        .expect("SetNodeContract ack within 30s")
        .expect("ack sender not dropped");
    (h, sink_rx)
}

async fn receive_one(h: ColonyHandle, sink_rx: &mut mpsc::Receiver<Message>) -> Message {
    let msg = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("/sink must receive the routed message within 30s")
        .expect("capture channel delivers");
    h.shutdown().await;
    msg
}

/// One source emission as an `IngressEmitter` mints it — no parent, a carried
/// trace, a carried budget of 41 — through a colony whose default is 64.
async fn route_one_carried_emission(carries_trace: bool, trace: Uuid) -> Message {
    let (h, mut sink_rx) = colony_with_peer(carries_trace).await;
    h.outputs_sender()
        .send(CellEmission {
            sender_path: Path::new("/peer"),
            parent_message_id: None,
            trace_id: trace,
            input_ttl: 41,
            input_reply_to: None,
            input_headers: Headers::new(),
            target: Path::new("/sink"),
            content: json!({"messages": [{"origin": "user", "type": "text", "text": "crossed"}]}),
            direct_reply: false,
        })
        .await
        .expect("outputs channel open");
    receive_one(h, &mut sink_rx).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_declared_ingress_keeps_the_budget_it_carried() {
    let trace = Uuid::now_v7();
    let msg = route_one_carried_emission(true, trace).await;
    assert_eq!(
        msg.ttl, 40,
        "41 survives the outputs arm, decremented once by route(); 63 means re-stamped"
    );
    assert_eq!(
        msg.trace_id, trace,
        "one conversation stays one trace across two logs"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_undeclared_source_emission_still_gets_the_colony_default() {
    let msg = route_one_carried_emission(false, Uuid::now_v7()).await;
    assert_eq!(
        msg.ttl, 63,
        "without the declaration the colony default (64) is still stamped, then decremented"
    );
}

/// The price of `CellEmission` carrying no ingress marker (OR-Peer7, pinned as
/// `docs/config.en.md` § `contract.ingress` states it): the outputs arm cannot tell
/// an ingress emission from a plain `OriginSink::emit` of the same declared cell,
/// so the cell's own source emissions also keep the budget its sink carries (50
/// here, 64 for `proxy`) instead of being stamped with `message_default_ttl`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_declared_cells_own_source_emission_keeps_its_sink_budget() {
    let (h, mut sink_rx) = colony_with_peer(true).await;
    let origin = OriginSink::new(h.outputs_sender(), Path::new("/peer"), 50).with_ingress();
    origin
        .emit(CellOutput {
            target: Path::new("/sink"),
            content: json!({"messages": [{"origin": "user", "type": "text", "text": "own"}]}),
        })
        .await
        .expect("outputs channel open");
    drop(origin);
    let msg = receive_one(h, &mut sink_rx).await;
    assert_eq!(
        msg.ttl, 49,
        "the sink budget (50) survives the outputs arm, decremented once by route(); 63 means re-stamped"
    );
}
