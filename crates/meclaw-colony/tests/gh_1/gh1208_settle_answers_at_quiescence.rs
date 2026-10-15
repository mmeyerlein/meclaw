//! GH #1208 (TV-A2): `ColonyMsg::Settle` — the quiescence barrier behind
//! `ColonyHandle::quiesce`.
//!
//! A settle is answered once `drain::is_quiescent` holds (no ticket, no
//! overflow, no mailbox, inbox or output holds a message) AND the message-log
//! writer has committed everything sent before it. It changes no phase, ends
//! nothing, and a settle waiting when the colony drains is answered too.
//!
//! The "not yet" assertions are one-hop negatives (TV-A1): the settle is read,
//! then the sync marker; the settle's loop foot ran before the marker was
//! answered, so an unanswered settle after `sync()` is a fact, not a window.

use meclaw_colony::ColonyMsg;
use meclaw_colony::api_dto::MessageLogFilter;
use meclaw_core::{Cell, Message, MessageBuilder, OutputSink, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::deadline::hang;
use meclaw_testing::mocks::EchoMockCell;
use std::future::Future;
use tokio::sync::{mpsc, oneshot, watch};

/// A cell whose handler reports that it started and then waits for the gate.
struct GateCell {
    started: mpsc::Sender<()>,
    gate: watch::Receiver<bool>,
}

impl Cell for GateCell {
    #[allow(clippy::manual_async_fn)]
    fn handle(&mut self, _msg: Message, _sink: &OutputSink) -> impl Future<Output = ()> + Send {
        let started = self.started.clone();
        let mut gate = self.gate.clone();
        async move {
            let _ = started.send(()).await;
            let _ = gate.wait_for(|open| *open).await;
        }
    }
}

/// Boots a colony with a gated cell at `/gate` and hands one message to it.
/// Returns once the handler runs (it is now owed back to the drain ledger).
async fn colony_with_a_running_handler() -> (ColonyHandle, watch::Sender<bool>) {
    let h = ColonyHandle::new();
    let (started_tx, mut started_rx) = mpsc::channel(4);
    let (gate_tx, gate_rx) = watch::channel(false);
    h.spawn(Path::new("/gate"), move || GateCell {
        started: started_tx.clone(),
        gate: gate_rx.clone(),
    })
    .await;
    h.send(MessageBuilder::new(Path::new("/gate")).build())
        .await;
    hang("the gated handler starts", started_rx.recv())
        .await
        .expect("gate cell gone");
    (h, gate_tx)
}

async fn send_settle(h: &ColonyHandle) -> oneshot::Receiver<()> {
    let (ack, rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Settle { ack })
        .await
        .expect("colony inbox closed");
    rx
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn settle_answers_once_every_mailbox_is_empty_and_no_handler_runs() {
    let (h, gate) = colony_with_a_running_handler().await;

    let mut settled = send_settle(&h).await;
    h.sync().await;
    assert_eq!(
        settled.try_recv(),
        Err(oneshot::error::TryRecvError::Empty),
        "settle answered while a handler still runs"
    );

    gate.send_replace(true);
    hang("the colony settles after the handler returned", settled)
        .await
        .expect("settle dropped");
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn settle_waits_for_the_writer_fence() {
    const HOPS: usize = 200;
    let h = ColonyHandle::new();
    h.spawn(Path::new("/a"), || EchoMockCell::new(Path::new("/a")))
        .await;
    let msg = MessageBuilder::new(Path::new("/a")).build();
    let trace_id = msg.trace_id.to_string();
    let mut sent = vec![msg];
    for _ in 1..HOPS {
        sent.push(
            MessageBuilder::new(Path::new("/a"))
                .trace_id(sent[0].trace_id)
                .build(),
        );
    }
    for m in sent {
        h.send(m).await;
    }

    h.quiesce().await;
    // Read once, no wait: the fence put every logged hop before the answer.
    let rows = h
        .read_log(MessageLogFilter {
            trace_id: Some(trace_id),
            ..Default::default()
        })
        .await;
    assert_eq!(rows.len(), HOPS, "a hop was still in the writer queue");
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn settle_never_ends_the_colony() {
    let h = ColonyHandle::new();
    let (tap_tx, mut tap_rx) = mpsc::channel(4);
    h.spawn(Path::new("/a"), move || {
        EchoMockCell::new(Path::new("/a")).tap_to(tap_tx.clone())
    })
    .await;

    h.quiesce().await;
    h.quiesce().await;

    h.send(MessageBuilder::new(Path::new("/a")).build()).await;
    let tapped = hang("the colony still delivers after two settles", tap_rx.recv())
        .await
        .expect("tap closed");
    assert_eq!(tapped, Path::new("/a"));
    h.quiesce().await;
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_settle_during_a_drain_is_answered() {
    let (h, gate) = colony_with_a_running_handler().await;

    let mut settled = send_settle(&h).await;
    let (shutdown_ack, shutdown_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Shutdown { ack: shutdown_ack })
        .await
        .expect("colony inbox closed");
    // The drain keeps the inbox open: the marker is read while the handler runs.
    h.sync().await;
    assert_eq!(
        settled.try_recv(),
        Err(oneshot::error::TryRecvError::Empty),
        "settle answered while the drain still waits on a handler"
    );

    gate.send_replace(true);
    hang("the settle waiting in the drain is answered", settled)
        .await
        .expect("the drain dropped the waiting settle");
    hang("the drained colony acks its shutdown", shutdown_rx)
        .await
        .expect("shutdown ack dropped");
    h.shutdown().await;
}
