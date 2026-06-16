//! Phase-3b demo topology: 3-Hop forwarder chain.
//!
//! /a → /b → /c. /a and /b are EchoMockCells with `.with_emitted_header`
//! setting `forwarded_by` to their own path. /c is a `CaptureCell` that
//! pushes every `Message` it sees into an mpsc channel for test inspection
//! (no Mutex — tap_to-pattern, no await under lock).

use crate::ColonyHandle;
use crate::mocks::EchoMockCell;
use meclaw_core::serde_json::json;
use meclaw_core::{Cell, Message, OutputSink, Path, Uuid};
use std::future::Future;
use tokio::sync::mpsc;

/// Terminal cell that forwards every handled [`Message`] into an mpsc channel.
///
/// No `Mutex`/`RwLock` — the sender is cloned cheaply and used directly
/// inside the async block (tap_to-pattern, no await-under-lock).
pub struct CaptureCell {
    tx: mpsc::Sender<Message>,
}

impl CaptureCell {
    /// Create a new `CaptureCell` that sends received messages to `tx`.
    pub fn new(tx: mpsc::Sender<Message>) -> Self {
        Self { tx }
    }
}

impl Cell for CaptureCell {
    #[allow(clippy::manual_async_fn)]
    fn handle(&mut self, msg: Message, _sink: &OutputSink) -> impl Future<Output = ()> + Send {
        let tx = self.tx.clone();
        async move {
            let _ = tx.send(msg).await;
        }
    }
}

/// Handle to the assembled 3-hop topology and the receiver end of /c's channel.
pub struct Phase3bTopology {
    /// Running colony with /a, /b, /c spawned.
    pub colony: ColonyHandle,
    /// Receiver for every [`Message`] that arrives at /c.
    pub c_rx: mpsc::Receiver<Message>,
}

/// Assemble the 3-hop chain: /a → /b → /c.
///
/// * /a: [`EchoMockCell`] forwarding to /b, emitting `forwarded_by = "/a"`.
/// * /b: [`EchoMockCell`] forwarding to /c, emitting `forwarded_by = "/b"`.
/// * /c: [`CaptureCell`] — pushes every message into the returned channel.
pub async fn build_phase_3b_topology() -> Phase3bTopology {
    let colony = ColonyHandle::new();
    let (c_tx, c_rx) = mpsc::channel::<Message>(16);

    colony
        .spawn(Path::new("/a"), move || {
            EchoMockCell::new(Path::new("/a"))
                .echo_to(Path::new("/b"))
                .with_emitted_header("forwarded_by", json!("/a"))
        })
        .await;

    colony
        .spawn(Path::new("/b"), move || {
            EchoMockCell::new(Path::new("/b"))
                .echo_to(Path::new("/c"))
                .with_emitted_header("forwarded_by", json!("/b"))
        })
        .await;

    let c_tx_for_factory = c_tx.clone();
    colony
        .spawn(Path::new("/c"), move || {
            CaptureCell::new(c_tx_for_factory.clone())
        })
        .await;

    // Wire the chain as unconditional catch-all out-edges: /a -> /b and /b -> /c.
    // Ruling A1 dropped the implicit identity-fallback; each forwarder's single
    // echo now needs a real default edge. Each edge carries the emission's headers
    // (forwarded_by) through, so header + parent_message_id propagation across the
    // 3 hops is unchanged. Neither forwarder emits anything else, so nothing is
    // rerouted.
    colony
        .add_edge(Uuid::now_v7(), Path::new("/a"), Path::new("/b"))
        .await;
    colony
        .add_edge(Uuid::now_v7(), Path::new("/b"), Path::new("/c"))
        .await;

    Phase3bTopology { colony, c_rx }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn topology_spawns_3_hop_chain_cleanly() {
        let topo = build_phase_3b_topology().await;
        topo.colony.shutdown().await;
    }
}
