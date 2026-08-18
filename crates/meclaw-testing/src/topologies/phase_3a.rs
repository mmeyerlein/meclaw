//! Phase-3a demo topology.
//!
//! Layout:
//!   /a/forwarder → EchoMockCell with emitted_target("./receiver"), taps own path
//!   /a/receiver  → terminal capture cell (pushes every Message into an mpsc tap)
//!
//! Used by integration tests to verify:
//!   - parent_message_id propagation: receiver's message has parent = forwarder's input id
//!   - trace_id propagation: forwarder's input and receiver's message share trace_id
//!   - reply_to set by Colony: receiver's message has reply_to = "/a/forwarder"
//!   - reply_to cascade: route to /missing with reply_to=/a/receiver delivers
//!     an error-reply to /a/receiver
//!   - ttl_expired: a message with ttl=0 lands in DLQ with TtlExpired
//!
//! `CaptureCell` uses the same mpsc-tap pattern as `phase_3b::CaptureCell`
//! (no `Mutex`/`RwLock` in cell state — no await-under-lock).

use crate::ColonyHandle;
use crate::mocks::EchoMockCell;
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

/// Handle to the assembled phase-3a topology and the receiver end of the tap channels.
pub struct Phase3aTopology {
    pub colony: ColonyHandle,
    /// Receiver for every [`Message`] that arrives at `/a/receiver`.
    pub receiver_rx: mpsc::Receiver<Message>,
    /// Receiver for forwarder-path taps (one [`Path`] per message handled at `/a/forwarder`).
    pub tap_rx: mpsc::Receiver<Path>,
}

pub async fn build_phase_3a_topology() -> Phase3aTopology {
    let colony = ColonyHandle::new();
    let (tap_tx, tap_rx) = mpsc::channel(64);
    let (recv_tx, receiver_rx) = mpsc::channel::<Message>(64);

    // /a/forwarder — echoes to ../receiver (resolves to /a/receiver from sender /a/forwarder)
    let tap_f = tap_tx.clone();
    colony
        .spawn(Path::new("/a/forwarder"), move || {
            EchoMockCell::new(Path::new("/a/forwarder"))
                .emitted_target(Path::new("../receiver"))
                .tap_to(tap_f.clone())
        })
        .await;

    // /a/receiver — captures via mpsc tap
    let recv_tx_for_factory = recv_tx.clone();
    colony
        .spawn(Path::new("/a/receiver"), move || {
            CaptureCell::new(recv_tx_for_factory.clone())
        })
        .await;

    // Wire /a/forwarder -> /a/receiver as the unconditional catch-all out-edge
    // (Phase-16 W2 / Ruling A1: the forwarder's echo to ../receiver no longer
    // rides the implicit identity-fallback; it needs a real default edge). The
    // forwarder's only emission is this echo, so the catch-all reroutes nothing else.
    colony
        .add_edge(
            Uuid::now_v7(),
            Path::new("/a/forwarder"),
            Path::new("/a/receiver"),
        )
        .await;

    Phase3aTopology {
        colony,
        receiver_rx,
        tap_rx,
    }
}
