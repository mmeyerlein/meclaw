//! GH #47, ruling O1: the corridor does not need to know that the colony is
//! dying — its WRAPPER already knows who is about to receive a message.
//!
//! `route_with_log` computes `pre_routable` before it calls `route()`:
//! `!is_colony_endpoint && msg.ttl > 0 && registry.contains_key(&resolved_target)`.
//! That is byte for byte the condition under which `route()` takes its
//! `Some(entry)` branch and sends. This file is the receipt that the two really
//! do coincide — measured through a live colony, not asserted about a local.
//!
//! Both receipts are read off the colony's own drain MARKERS, never off a
//! latency (tv-br M10y, re-check R H4, CLAUDE.md rule 11): the ledger is
//! `pub(crate)` and an integration test never sees it, but a colony that owes a
//! ticket must refuse to declare itself quiescent -- it still acks a message
//! sent after the shutdown -- and a colony that owes none ends its drain with
//! "drain complete -- quiescent", never with "hit its deadline".

use meclaw_core::{Cell, Message, OutputSink, Path};
use meclaw_testing::{ColonyHandle, MessageBuilder};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use tracing::Level;

/// The marker the colony logs when its drain ends because it owes nothing.
const DRAIN_DONE: &str = "drain complete — the colony is quiescent";

/// The marker the colony logs when its drain budget cuts owed work.
const DRAIN_CUT: &str = "shutdown drain hit its deadline";

/// The marker the colony logs when it takes the shutdown and starts to drain.
const DRAIN_STARTED: &str = "shutdown requested — draining in-flight work";

/// How many of each drain marker this process has recorded so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DrainMarkers {
    started: usize,
    done: usize,
    cut: usize,
}

fn drain_markers() -> DrainMarkers {
    let rec = meclaw_testing::log_recorder::global();
    DrainMarkers {
        started: rec.lines(Level::INFO, &[DRAIN_STARTED]).len(),
        done: rec.lines(Level::INFO, &[DRAIN_DONE]).len(),
        cut: rec.lines(Level::WARN, &[DRAIN_CUT]).len(),
    }
}

/// The drain since `before`: exactly one drain that ended by quiescence and
/// none that was cut -- the receipt that the colony owed nothing at its end.
fn assert_one_quiescent_drain_since(before: DrainMarkers, what: &str) {
    let now = drain_markers();
    assert_eq!(
        (now.done - before.done, now.cut - before.cut),
        (1, 0),
        "{what}: the drain must end by quiescence, never at its deadline \
         (markers before {before:?}, after {now:?})"
    );
}

/// A colony root whose `colony.json` pins the drain budget for this file: the
/// product wait of `meclaw_testing::deadline` (ten scaled hang marks), so the
/// budget never ends a drain inside a test. A drain that does not end by
/// quiescence is named by the test's own hang mark; the verdict is which drain
/// marker the colony logged, never how long it took.
fn colony_root_with_drain_budget() -> tempfile::TempDir {
    colony_root_with_drain_budget_ms(meclaw_testing::deadline::product_wait_ms())
}

/// The drain budget of the "held" receipt: short, so the cut comes soon. It is
/// the colony's configuration (SUT time), never a window the test waits out --
/// the handler stays blocked until the test has READ the cut.
const SHORT_DRAIN_BUDGET_MS: u64 = 200;

/// A colony root whose `colony.json` sets the drain budget to `budget` ms.
fn colony_root_with_drain_budget_ms(budget: u64) -> tempfile::TempDir {
    let td = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(
        td.path().join("colony.json"),
        format!(r#"{{"shutdown_drain_timeout_ms": {budget}}}"#),
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

/// One colony, one message, one shutdown — and the receipt that its drain
/// ended by quiescence ([`assert_one_quiescent_drain_since`]).
///
/// `sink` registers an echo cell beforehand, for the case whose target must
/// EXIST in the registry while still not being owed anything (the TTL death).
async fn shutdown_owes_nothing_after(sink: Option<Path>, msg: Message, what: &str) {
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

    let before = drain_markers();
    meclaw_testing::deadline::hang("the shutdown returns", h.shutdown()).await;
    assert_one_quiescent_drain_since(before, what);
}

/// A message that reaches a registered cell is accounted for from before it
/// enters the mailbox until after the handler returns.
///
/// Two receipts, both markers (tv-br M10y, re-check R H4; CLAUDE.md rule 11):
///
/// 1. **Held.** A colony whose drain budget is short shuts down while the
///    handler blocks. Its mailbox is empty, its outputs are empty -- the state
///    the pre-#47 shutdown read as "done" -- and still the drain does not end
///    by quiescence: it is CUT at its budget, and the cut names `/blocker` as
///    owed. The budget is the colony's own configuration, not a test window:
///    the handler blocks until the test has read the cut, whenever it comes.
/// 2. **Released.** With the budget out of reach, a released handler leaves
///    nothing owed: the drain ends by quiescence.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delivery_is_owed_until_the_handler_is_done() {
    let _ = meclaw_testing::log_recorder::global();

    // 1. Held: the cut names the blocked handler as owed.
    let td = colony_root_with_drain_budget_ms(SHORT_DRAIN_BUDGET_MS);
    let (h, release_tx) = colony_with_a_blocked_handler(&td).await;
    let before = drain_markers();
    let shutting_down = tokio::spawn(async move { h.shutdown().await });
    meclaw_testing::wait::until("the drain is cut at its budget", || async {
        drain_markers().cut > before.cut
    })
    .await;
    let rec = meclaw_testing::log_recorder::global();
    let cut = rec.lines(Level::WARN, &[DRAIN_CUT]);
    let last = cut.last().expect("the cut marker was just seen");
    assert!(
        last.contains("drain_incomplete=1") && last.contains("/blocker"),
        "the shutdown must still owe the handler in flight — an empty mailbox \
         is not a finished handler; the cut said: {last}"
    );
    assert_eq!(
        drain_markers().done,
        before.done,
        "a drain with a handler in flight never ends by quiescence"
    );
    // Whether the teardown waits for the cell or has already stopped it, the
    // shutdown ends once the handler may return.
    let _ = release_tx.send(());
    meclaw_testing::deadline::hang("the cut shutdown returns", shutting_down)
        .await
        .expect("the shutdown task must end normally, never by panic");

    // 2. Released: the drain ends by quiescence once the handler returned.
    let td = colony_root_with_drain_budget();
    let (h, release_tx) = colony_with_a_blocked_handler(&td).await;
    let before = drain_markers();
    let shutting_down = tokio::spawn(async move { h.shutdown().await });
    meclaw_testing::wait::until("the colony enters its drain phase", || async {
        drain_markers().started > before.started
    })
    .await;
    release_tx
        .send(())
        .expect("the blocked handler still holds the release receiver");
    meclaw_testing::deadline::hang(
        "the shutdown returns once the handler is released",
        shutting_down,
    )
    .await
    .expect("the shutdown task must end normally, never by panic");
    assert_one_quiescent_drain_since(
        before,
        "once the last handler returned the colony owes nothing",
    );
}

/// A colony at `td` with `/blocker` INSIDE its handler: the message is out of
/// the mailbox, the handler has announced itself and parks until the returned
/// sender fires.
async fn colony_with_a_blocked_handler(
    td: &tempfile::TempDir,
) -> (ColonyHandle, oneshot::Sender<()>) {
    let h = ColonyHandle::new_with_factories_at(td, vec![]);
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
    meclaw_testing::deadline::hang("the blocking handler is entered", entered_rx.recv())
        .await
        .expect("the entered-channel sender lives in the cell");
    (h, release_tx)
}

/// The three classes that must NOT be owed: a dead-letter (no registry entry),
/// a colony endpoint, and a TTL death. All three take `route()` branches that
/// never touch a mailbox, and `pre_routable` is false for all three.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_undeliverable_message_is_owed_by_nobody() {
    // Send to /nowhere, to /colony/registry, and with ttl = 0.
    // Receipt: the drain ends by quiescence in all three cases — a colony that
    // owed a phantom ticket would never log it (and run into the hang mark).
    let _ = meclaw_testing::log_recorder::global();

    // 1. No registry entry: `route()` goes to `handle_unresolved`.
    shutdown_owes_nothing_after(
        None,
        MessageBuilder::new("/nowhere").build(),
        "a message with no registry entry is owed by nobody",
    )
    .await;

    // 2. A colony endpoint: `route()` returns `ColonyDispatch`, no mailbox.
    shutdown_owes_nothing_after(
        None,
        MessageBuilder::new("/colony/registry").build(),
        "a colony endpoint has no mailbox to owe a ticket to",
    )
    .await;

    // 3. TTL death, and deliberately at a target that EXISTS: this is the case
    //    that separates the TTL branch from the unresolved one. `pre_routable`
    //    is false on `msg.ttl > 0` alone, and `route()` dead-letters before it
    //    ever looks the registry up.
    shutdown_owes_nothing_after(
        Some(Path::new("/sink")),
        MessageBuilder::new("/sink").with_ttl(0).build(),
        "a message out of TTL dies before the registry lookup and is owed by \
         nobody, registered target or not",
    )
    .await;
}

/// A cell that dies mid-handler owes nothing afterwards: `handle_cell_died` at
/// the call site clears its tickets, so the drain does not sit out its budget
/// waiting for a task that is gone.
///
/// The panic is real (a cell whose `handle()` panics), not simulated, and the
/// receipt is again the drain marker: the drain ends by quiescence.
///
/// **Vacuously green until Task 11.** There is no drain phase yet, so the
/// shutdown returns at once whatever the ledger says. What this test does prove
/// today is the other half of its setup: the panic reaches the colony and the
/// cell is restarted. It becomes decisive the moment the drain lands — from then
/// on a ticket left behind by the dead cell would cost the full budget.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cell_that_died_owes_nothing() {
    let _ = meclaw_testing::log_recorder::global();
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
    meclaw_testing::wait::wait_for_spawn_count(&spawns, 2).await;

    let before = drain_markers();
    meclaw_testing::deadline::hang("the shutdown returns", h.shutdown()).await;
    assert_one_quiescent_drain_since(
        before,
        "a dead cell's tickets die with it — the drain must not wait for a task \
         that is gone",
    );
}
