//! GH #47, ruling O1: the corridor does not need to know that the colony is
//! dying — its WRAPPER already knows who is about to receive a message.
//!
//! `route_with_log` computes `pre_routable` before it calls `route()`:
//! `!is_colony_endpoint && msg.ttl > 0 && registry.contains_key(&resolved_target)`.
//! That is byte for byte the condition under which `route()` takes its
//! `Some(entry)` branch and sends. This file is the receipt that the two really
//! do coincide — measured through a live colony, not asserted about a local.
//!
//! Both receipts are read off the LATENCY of `ColonyHandle::shutdown()`: the
//! ledger is `pub(crate)` and an integration test never sees it, but a colony
//! that owes a ticket must refuse to declare itself quiescent, and a colony that
//! owes none must not sit out its drain budget.

use meclaw_core::{Cell, Message, OutputSink, Path};
use meclaw_testing::{ColonyHandle, MessageBuilder};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

/// Failure marker, generous per the 30s convention (robust under cargo load).
const MARKER: Duration = Duration::from_secs(30);

/// The drain budget every colony in this file is given, via its `colony.json`.
/// Five seconds is deliberately far away from both discriminators below, so
/// "held" and "prompt" are an order of magnitude apart and neither reading
/// depends on scheduler luck.
const DRAIN_BUDGET_MS: u64 = 5_000;

/// SEMANTIC discriminator: how long the shutdown must still be running while a
/// handler blocks. Tight on purpose — a tenth of the drain budget. Anything
/// longer would also pass on a colony that simply happens to be slow; anything
/// shorter would measure the scheduler rather than the drain.
const HELD_PROBE: Duration = Duration::from_millis(500);

/// SEMANTIC discriminator: what counts as "prompt". Under a third of the drain
/// budget, so a colony that ran into its deadline (5 s) can never be mistaken
/// for one that finished because it owed nothing.
const PROMPT: Duration = Duration::from_millis(1_500);

/// A colony root whose `colony.json` pins the drain budget for this file.
fn colony_root_with_drain_budget() -> tempfile::TempDir {
    let td = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(
        td.path().join("colony.json"),
        format!(r#"{{"shutdown_drain_timeout_ms": {DRAIN_BUDGET_MS}}}"#),
    )
    .expect("write the test colony.json");
    td
}

/// A cell whose `handle()` parks on a `oneshot` until the test releases it, and
/// which announces on `entered` that it is inside the handler.
///
/// The announcement is the positive receipt the whole first test rests on: the
/// handler is running, therefore the cell TOOK the message out of its mailbox,
/// therefore the mailbox is empty — the exact state the pre-#47 shutdown read as
/// "done".
///
/// The `Arc<Mutex<..>>` around the receiver is TEST scaffolding: `spawn` wants a
/// `Fn() -> C` factory that may run more than once, and a `oneshot::Receiver`
/// is not `Clone`. The no-lock rule of `AGENTS.md` governs cell and colony
/// state in the substrate, not a test's own handle onto its trigger.
struct BlockingCell {
    entered: mpsc::Sender<()>,
    release: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
}

impl Cell for BlockingCell {
    #[allow(clippy::manual_async_fn)]
    fn handle(
        &mut self,
        _msg: Message,
        _sink: &OutputSink,
    ) -> impl std::future::Future<Output = ()> + Send {
        let entered = self.entered.clone();
        let release = self.release.clone();
        async move {
            // Taken in its own scope: the guard must be gone before the first
            // `.await`, or this future would not be `Send`.
            let waiter = {
                let mut slot = release.lock().expect("the release slot is never poisoned");
                slot.take()
            };
            let _ = entered.send(()).await;
            if let Some(waiter) = waiter {
                let _ = waiter.await;
            }
        }
    }
}

/// A cell whose `handle()` really panics — no simulation, no injected error.
struct PanickingCell;

impl Cell for PanickingCell {
    #[allow(clippy::manual_async_fn)]
    fn handle(
        &mut self,
        _msg: Message,
        _sink: &OutputSink,
    ) -> impl std::future::Future<Output = ()> + Send {
        async move {
            panic!("GH #47: a real panic, so a real `CellDied` reaches the colony");
        }
    }
}

/// One colony, one message, one shutdown — and how long the shutdown took.
///
/// `sink` registers an echo cell beforehand, for the case whose target must
/// EXIST in the registry while still not being owed anything (the TTL death).
async fn shutdown_latency_after(sink: Option<Path>, msg: Message) -> Duration {
    let td = colony_root_with_drain_budget();
    let h = ColonyHandle::new_with_factories_at(&td, vec![]);
    if let Some(p) = sink {
        let own = p.clone();
        h.spawn(p, move || {
            meclaw_testing::mocks::EchoMockCell::new(own.clone())
        })
        .await;
    }
    h.send_from(Path::new("/"), msg).await;
    // Barrier: the colony inbox is FIFO and the loop is one task, so an ack for
    // a LATER message proves the route above was handled before the shutdown.
    h.add_hive_scope(Path::new("/barrier")).await;

    let started = Instant::now();
    tokio::time::timeout(MARKER, h.shutdown())
        .await
        .expect("the shutdown must return within the failure marker");
    started.elapsed()
}

/// A message that reaches a registered cell is accounted for from before it
/// enters the mailbox until after the handler returns.
///
/// **Expected RED until Task 11.** The ticket lands in the ledger here, but
/// nothing reads the ledger yet — the shutdown has no drain phase to hold it, so
/// it returns at once and the "held" probe fails. Task 11 makes it green.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delivery_is_owed_until_the_handler_is_done() {
    // A cell whose handler blocks until the test releases it. While it blocks:
    // its mailbox is EMPTY (it took the message) and the outputs channel is
    // EMPTY (it has not emitted) — the exact state in which the pre-#47
    // shutdown declared victory. The colony must nevertheless refuse to call
    // itself quiescent.
    // Receipt: `ColonyHandle::shutdown()` must NOT return while the handler
    // blocks, and must return promptly once it is released.
    let td = colony_root_with_drain_budget();
    let h = ColonyHandle::new_with_factories_at(&td, vec![]);

    let (entered_tx, mut entered_rx) = mpsc::channel::<()>(4);
    let (release_tx, release_rx) = oneshot::channel::<()>();
    let release = Arc::new(Mutex::new(Some(release_rx)));

    h.spawn(Path::new("/blocker"), {
        let entered = entered_tx.clone();
        let release = release.clone();
        move || BlockingCell {
            entered: entered.clone(),
            release: release.clone(),
        }
    })
    .await;

    h.send_from(Path::new("/"), MessageBuilder::new("/blocker").build())
        .await;

    // Positive receipt: the handler is INSIDE `handle()`, so the mailbox has
    // already handed the message over and is empty again.
    tokio::time::timeout(MARKER, entered_rx.recv())
        .await
        .expect("the blocking handler must be entered within the failure marker")
        .expect("the entered-channel sender lives in the cell");

    let mut shutting_down = tokio::spawn(async move { h.shutdown().await });

    assert!(
        tokio::time::timeout(HELD_PROBE, &mut shutting_down)
            .await
            .is_err(),
        "the shutdown must still be running while a handler is in flight — \
         an empty mailbox is not a finished handler"
    );

    let released_at = Instant::now();
    release_tx
        .send(())
        .expect("the blocked handler still holds the release receiver");
    tokio::time::timeout(MARKER, shutting_down)
        .await
        .expect("the shutdown must return once the handler is released")
        .expect("the shutdown task must end normally, never by panic");
    let after_release = released_at.elapsed();
    assert!(
        after_release < PROMPT,
        "once the last handler returned the colony owes nothing and must finish \
         promptly, not sit out its {DRAIN_BUDGET_MS} ms budget — took {after_release:?}"
    );
}

/// The three classes that must NOT be owed: a dead-letter (no registry entry),
/// a colony endpoint, and a TTL death. All three take `route()` branches that
/// never touch a mailbox, and `pre_routable` is false for all three.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_undeliverable_message_is_owed_by_nobody() {
    // Send to /nowhere, to /colony/registry, and with ttl = 0.
    // Receipt: `shutdown()` returns promptly in all three cases — a colony that
    // owed a phantom ticket would sit out the whole drain budget instead.

    // 1. No registry entry: `route()` goes to `handle_unresolved`.
    let took = shutdown_latency_after(None, MessageBuilder::new("/nowhere").build()).await;
    assert!(
        took < PROMPT,
        "a message with no registry entry is owed by nobody — took {took:?}"
    );

    // 2. A colony endpoint: `route()` returns `ColonyDispatch`, no mailbox.
    let took = shutdown_latency_after(None, MessageBuilder::new("/colony/registry").build()).await;
    assert!(
        took < PROMPT,
        "a colony endpoint has no mailbox to owe a ticket to — took {took:?}"
    );

    // 3. TTL death, and deliberately at a target that EXISTS: this is the case
    //    that separates the TTL branch from the unresolved one. `pre_routable`
    //    is false on `msg.ttl > 0` alone, and `route()` dead-letters before it
    //    ever looks the registry up.
    let took = shutdown_latency_after(
        Some(Path::new("/sink")),
        MessageBuilder::new("/sink").with_ttl(0).build(),
    )
    .await;
    assert!(
        took < PROMPT,
        "a message out of TTL dies before the registry lookup and is owed by \
         nobody, registered target or not — took {took:?}"
    );
}

/// A cell that dies mid-handler owes nothing afterwards: `handle_cell_died` at
/// the call site clears its tickets, so the drain does not sit out its budget
/// waiting for a task that is gone.
///
/// The panic is real (a cell whose `handle()` panics), not simulated, and the
/// receipt is again latency: `shutdown()` returns promptly.
///
/// **Vacuously green until Task 11.** There is no drain phase yet, so the
/// shutdown returns at once whatever the ledger says. What this test does prove
/// today is the other half of its setup: the panic reaches the colony and the
/// cell is restarted. It becomes decisive the moment the drain lands — from then
/// on a ticket left behind by the dead cell would cost the full budget.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cell_that_died_owes_nothing() {
    let td = colony_root_with_drain_budget();
    let h = ColonyHandle::new_with_factories_at(&td, vec![]);

    // The factory counts its own invocations — `spawn` calls it once now and
    // stores it as the respawn closure, so a second count IS the restart.
    let spawns = Arc::new(AtomicU32::new(0));
    h.spawn(Path::new("/doomed"), {
        let spawns = spawns.clone();
        move || {
            spawns.fetch_add(1, Ordering::SeqCst);
            PanickingCell
        }
    })
    .await;
    assert_eq!(
        spawns.load(Ordering::SeqCst),
        1,
        "the initial spawn must have gone through the counting factory"
    );

    h.send_from(Path::new("/"), MessageBuilder::new("/doomed").build())
        .await;

    // Positive receipt that the panic reached the colony and `handle_cell_died`
    // ran: the respawn closure was called a second time.
    meclaw_testing::wait::wait_for_spawn_count(&spawns, 2, MARKER).await;

    let started = Instant::now();
    tokio::time::timeout(MARKER, h.shutdown())
        .await
        .expect("the shutdown must return within the failure marker");
    let took = started.elapsed();
    assert!(
        took < PROMPT,
        "a dead cell's tickets die with it — the drain must not wait out its \
         {DRAIN_BUDGET_MS} ms budget for a task that is gone; took {took:?}"
    );
}
