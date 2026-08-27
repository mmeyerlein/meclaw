//! Paket-1 (a)-behavioral — a bounded mailbox capacity is *observable* as
//! backpressure: once the mailbox is full, the next `send` blocks until the
//! cell drains a slot.
//!
//! Structural proof ("`cell.mailbox_size: N` → `max_capacity() == N`") is
//! already covered end-to-end elsewhere and NOT duplicated here:
//!   - factory-direct: `crates/meclaw-cells/tests/store_factory_mailbox_capacity.rs`
//!     and `crates/meclaw-cells/tests/factory_mailbox_capacity.rs`,
//!   - mutation-spawn path: `crates/meclaw-colony/tests/paket_1_mailbox_size_mutation.rs`,
//!   - bootstrap default-vs-override: `paket_1_mailbox_default_and_override.rs`.
//!
//! This file adds the *behavioral* discriminator: capacity is not just a
//! recorded number, it actually bounds the channel. A gate cell parks inside
//! `handle()` on a test-controlled `oneshot`. With `mailbox_size = 2` we fill
//! the mailbox while the gate holds the in-flight message, prove the
//! over-capacity `send` times out (blocked on backpressure), then open the
//! gate and prove the same `send` now completes (positive receipt).

use meclaw_colony::cell_task;
use meclaw_core::{ActorHandle, Cell, Message, MessageBuilder, OutputSink, Path};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Gate cell: blocks inside `handle()` until a single shared `oneshot` fires.
/// The first message it processes parks the cell task (so the mailbox cannot
/// drain); every later message returns immediately. Topology-free (no colony,
/// no `cell.db`) — this is a pure substrate backpressure probe.
struct GateCell {
    gate: Option<oneshot::Receiver<()>>,
}

impl Cell for GateCell {
    /// Park on the gate for the first message, then pass through. While parked,
    /// the cell task does not return to `mailbox.recv()`, so no mailbox slot is
    /// freed — exactly the condition that makes a bounded `send` block.
    fn handle(
        &mut self,
        _msg: Message,
        _sink: &OutputSink,
    ) -> impl std::future::Future<Output = ()> + Send {
        let gate = self.gate.take();
        async move {
            if let Some(rx) = gate {
                // Park until the test opens the gate. Err (sender dropped) also
                // releases — harmless, the test always sends first.
                let _ = rx.await;
            }
        }
    }
}

/// Tight semantic discriminator for "the send is blocked on backpressure".
///
/// Justification: a non-blocked `send` into a non-full bounded channel
/// completes in microseconds (a slot is immediately available). 200 ms is
/// ~3 orders of magnitude above that, so an `Elapsed` here can only mean the
/// channel is genuinely full and the producer is parked in
/// `Sender::send`'s reserve-a-slot wait — not lost to scheduler jitter.
/// Per the task brief: if this proves flaky, do NOT loosen it — report it for
/// the determinism watchlist instead.
const SHORT: Duration = Duration::from_millis(200);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bounded_mailbox_blocks_send_when_full_then_unblocks_on_drain() {
    // Mailbox capacity 2. The gate cell parks on its first message.
    let cap = 2usize;
    let (tx, rx) = mpsc::channel::<Message>(cap);
    assert_eq!(tx.max_capacity(), cap, "channel built with capacity 2");

    let (gate_tx, gate_rx) = oneshot::channel::<()>();
    let cell = GateCell {
        gate: Some(gate_rx),
    };
    let (otx, _orx) = mpsc::channel(8);
    let join = tokio::spawn(cell_task(
        Path::new("/gate"),
        rx,
        otx,
        cell,
        None,
        None,
        None,
    ));

    let handle = ActorHandle::new(Path::new("/gate"), tx);

    // Send 1: the cell pulls it out of the mailbox and parks in `handle()`.
    handle
        .send(MessageBuilder::new(Path::new("/gate")).build())
        .await
        .expect("first send accepted");

    // Give the cell task a moment to actually dequeue message 1 and enter the
    // parked `handle()`. Generous (failure-marker convention) — this is the
    // "cell has started processing" sync point, not the timing discriminator.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Sends 2 and 3 fill the two mailbox slots (message 1 is in-flight inside
    // the parked `handle()`, not occupying a slot).
    handle
        .send(MessageBuilder::new(Path::new("/gate")).build())
        .await
        .expect("send 2 fills slot 1");
    handle
        .send(MessageBuilder::new(Path::new("/gate")).build())
        .await
        .expect("send 3 fills slot 2");

    // Send 4 must block: both slots full and the cell is parked, so no slot is
    // freed. The tight SHORT discriminator proves backpressure (not jitter).
    let over_capacity = MessageBuilder::new(Path::new("/gate")).build();
    let blocked = tokio::time::timeout(SHORT, handle.send(over_capacity.clone())).await;
    assert!(
        blocked.is_err(),
        "send over capacity MUST block while the mailbox is full and the cell is parked"
    );

    // Open the gate: the cell returns from `handle()`, loops, drains slot 1,
    // and the previously-blocked producer gets a slot. Prove the SAME send now
    // completes (positive receipt — capacity is a real bound, not a fiction).
    gate_tx.send(()).expect("open gate");
    let unblocked = tokio::time::timeout(Duration::from_secs(30), handle.send(over_capacity))
        .await
        .expect("send must complete within 30s once the gate opens and the mailbox drains");
    assert!(
        unblocked.is_ok(),
        "the over-capacity send must succeed once a mailbox slot frees up"
    );

    // Drop the producer so the cell task's `mailbox.recv()` returns None and the
    // task ends cleanly.
    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(30), join).await;
}
