//! GH #47: a timer firing once a second must not make the drain run forever.
//!
//! The fix is structural, not a stop-loop over the registry: during the drain a
//! SOURCE emission (no parent message) is a new arrival and is refused, while a
//! FOLLOW-ON emission (it has a parent) is the very work being drained and is
//! carried. The substrate already tells the two apart — it has to, because only
//! a source gets a fresh TTL stamped.

use meclaw_core::serde_json::json;
use meclaw_core::{Cell, CellEmission, CellOutput, Headers, Message, OutputSink, Path, Uuid};
use meclaw_testing::mocks::EchoMockCell;
use meclaw_testing::{ColonyHandle, MessageBuilder};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

/// Failure marker, generous per the 30s convention (robust under cargo load).
const MARKER: Duration = Duration::from_secs(30);

/// The drain budget of the timer colony, as the plan sets it: large enough that
/// the deliberate drain window below fits several times over, small enough that
/// "the drain converged" and "the drain sat out its deadline" are far apart.
const DRAIN_BUDGET_MS: u64 = 3_000;

/// How often the source keeps firing — before, during and after the shutdown.
const TICK: Duration = Duration::from_millis(200);

/// SEMANTIC settle window: a tick that was routed just BEFORE the phase flipped
/// may still be on its way into the capture cell's `handle()` when the barrier
/// below returns. Waiting one tick out lets that last legitimate arrival land,
/// so the count read afterwards is the final pre-drain count. Nothing can be
/// added to it after the flip — that is exactly what this test claims.
const SETTLE: Duration = Duration::from_millis(250);

/// SEMANTIC drain window: how long the timer is left firing INTO the drain
/// before the blocked handler is released. Two to three ticks — enough that a
/// substrate which routed them would have to be seen doing it, short enough
/// that the whole shutdown stays far under the {DRAIN_BUDGET_MS} ms budget.
const DRAIN_WINDOW: Duration = Duration::from_millis(500);

/// SEMANTIC discriminator: the shutdown must still be running when the drain
/// window is over — otherwise the ticks above never met a draining colony and
/// the receipts below would prove nothing.
const HELD_PROBE: Duration = Duration::from_millis(200);

/// A colony root whose `colony.json` carries exactly `body`.
fn colony_root(body: &str) -> tempfile::TempDir {
    let td = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(td.path().join("colony.json"), body).expect("write the test colony.json");
    td
}

/// The emission shape a `timer`/`proxy`/`mcp` cell puts on the wire from its
/// I/O sub-task: no parent message, because nothing was consumed to produce it.
/// That absence IS the substrate's source discriminator (it is why a source and
/// only a source gets a fresh TTL stamped), so a test that produces this shape
/// exercises the same branch a real timer tick does.
fn source_tick(seq: usize) -> CellEmission {
    CellEmission {
        sender_path: Path::new("/timer"),
        parent_message_id: None,
        trace_id: Uuid::now_v7(),
        input_ttl: meclaw_core::MESSAGE_DEFAULT_TTL,
        input_headers: Headers::new(),
        input_reply_to: None,
        target: Path::new("/sink"),
        content: json!({
            "messages": [{"origin": "user", "type": "text", "text": format!("tick {seq}")}]
        }),
        direct_reply: false,
    }
}

/// A cell whose `handle()` parks on a `oneshot` until the test releases it,
/// announces on `entered` that it is inside the handler, and — once released —
/// optionally emits one message.
///
/// The announcement is the positive receipt the held-shutdown halves rest on:
/// the handler is running, therefore the cell TOOK the message out of its
/// mailbox, therefore the drain has real work to wait for.
///
/// The `Arc<Mutex<..>>` around the receiver is TEST scaffolding: `spawn` wants a
/// `Fn() -> C` factory that may run more than once, and a `oneshot::Receiver` is
/// not `Clone`. The no-lock rule of `AGENTS.md` governs cell and colony state in
/// the substrate, not a test's own handle onto its trigger.
struct BlockingCell {
    entered: mpsc::Sender<()>,
    release: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
    emit_to: Option<Path>,
}

impl Cell for BlockingCell {
    #[allow(clippy::manual_async_fn)]
    fn handle(
        &mut self,
        _msg: Message,
        sink: &OutputSink,
    ) -> impl std::future::Future<Output = ()> + Send {
        let entered = self.entered.clone();
        let release = self.release.clone();
        let emit_to = self.emit_to.clone();
        let sink = sink.clone();
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
            if let Some(target) = emit_to {
                let _ = sink
                    .push(CellOutput {
                        target,
                        content: json!({
                            "messages": [{
                                "origin": "assistant",
                                "type": "text",
                                "text": "the follow-on the drain carries",
                            }]
                        }),
                    })
                    .await;
            }
        }
    }
}

/// How many dead letters `colony.db` holds under one canonical code.
fn dead_letters_with_code(db: &std::path::Path, code: &str) -> i64 {
    let conn = rusqlite::Connection::open(db)
        .expect("a fresh connection must open the file the joined writer left behind");
    conn.query_row(
        "SELECT COUNT(*) FROM dead_letters WHERE error_code = ?1",
        [code],
        |r| r.get(0),
    )
    .expect("the dead_letters table must exist in colony.db")
}

/// A timer that keeps firing during the drain lands in the dead-letter queue
/// with `shutdown_draining`, and the drain still finishes well inside its
/// budget.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_source_emission_during_the_drain_is_dead_lettered_not_routed() {
    let td = colony_root(&format!(
        r#"{{"shutdown_drain_timeout_ms": {DRAIN_BUDGET_MS}}}"#
    ));
    let db_path = td.path().join("colony.db");
    let h = ColonyHandle::new_with_factories_at(&td, vec![]);

    let (entered_tx, mut entered_rx) = mpsc::channel::<()>(4);
    let (release_tx, release_rx) = oneshot::channel::<()>();
    let release = Arc::new(Mutex::new(Some(release_rx)));
    let (arrived_tx, mut arrived_rx) = mpsc::channel::<Path>(64);

    // The firing cell itself: it never receives anything, its emissions come
    // from the ticker task below — exactly the split a long-running cell has
    // between its handler and its I/O sub-task.
    h.spawn(Path::new("/timer"), || {
        EchoMockCell::new(Path::new("/timer"))
    })
    .await;
    // The capture cell, and the ONLY thing that can count a routed tick.
    let arrivals = Arc::new(AtomicUsize::new(0));
    h.spawn(Path::new("/sink"), {
        let arrived = arrived_tx.clone();
        move || EchoMockCell::new(Path::new("/sink")).tap_to(arrived.clone())
    })
    .await;
    // A cell whose handler holds the drain open, so the ticks below fire into a
    // colony that is really draining rather than one that is already gone.
    h.spawn(Path::new("/blocker"), {
        let entered = entered_tx.clone();
        let release = release.clone();
        move || BlockingCell {
            entered: entered.clone(),
            release: release.clone(),
            emit_to: None,
        }
    })
    .await;
    // An emission is routed by the EMITTING cell's out-edges, so without this
    // edge a tick would dead-letter as `no_route` whatever its target says.
    h.add_edge(Uuid::now_v7(), Path::new("/timer"), Path::new("/sink"))
        .await;

    // Count arrivals as they happen — the tap fires from `/sink`'s handler.
    let counting = tokio::spawn({
        let arrivals = arrivals.clone();
        async move {
            while let Some(p) = arrived_rx.recv().await {
                assert_eq!(p.as_str(), "/sink", "the tap names the cell that handled");
                arrivals.fetch_add(1, Ordering::SeqCst);
            }
        }
    });

    // The I/O half of the timer: one source emission every TICK, from before the
    // shutdown until the test stops it.
    let ticker = tokio::spawn({
        let outputs = h.outputs_sender();
        async move {
            let mut interval = tokio::time::interval(TICK);
            let mut seq = 0usize;
            loop {
                interval.tick().await;
                seq += 1;
                if outputs.send(source_tick(seq)).await.is_err() {
                    break;
                }
            }
        }
    });

    // Positive control: while the colony SERVES, a source emission routes. Every
    // claim below is about the difference to this state, so it has to be shown
    // rather than assumed.
    tokio::time::timeout(MARKER, async {
        while arrivals.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("a timer tick must reach /sink while the colony is still serving");

    // Occupy the blocker so the drain has something to wait for.
    h.send_from(Path::new("/"), MessageBuilder::new("/blocker").build())
        .await;
    tokio::time::timeout(MARKER, entered_rx.recv())
        .await
        .expect("the blocking handler must be entered within the failure marker")
        .expect("the entered-channel sender lives in the cell");

    let inbox_tx = h.inbox_tx.clone();
    let started = Instant::now();
    let mut shutting_down = tokio::spawn(async move { h.shutdown().await });

    // Barrier: the colony inbox is FIFO and the loop is one task, so an ack for
    // a LATER message proves the shutdown was handled first — the colony is in
    // its drain phase from here on. `ReadLiveness` is a pure read and changes
    // nothing about what the drain sees.
    let (live_tx, live_rx) = oneshot::channel();
    inbox_tx
        .send(meclaw_colony::ColonyMsg::ReadLiveness { ack: live_tx })
        .await
        .expect("the inbox stays OPEN during the drain — that is the point of it");
    tokio::time::timeout(MARKER, live_rx)
        .await
        .expect("the drain must keep answering reads within the failure marker")
        .expect("the colony answers the barrier read");

    tokio::time::sleep(SETTLE).await;
    let arrived_before_the_drain = arrivals.load(Ordering::SeqCst);
    assert!(
        arrived_before_the_drain >= 1,
        "the positive control must still hold: at least one tick routed while serving"
    );

    // Now let the timer fire INTO the drain for a while.
    tokio::time::sleep(DRAIN_WINDOW).await;
    assert!(
        tokio::time::timeout(HELD_PROBE, &mut shutting_down)
            .await
            .is_err(),
        "the shutdown must still be running while the blocked handler holds it — \
         otherwise the ticks above never met a draining colony"
    );

    release_tx
        .send(())
        .expect("the blocked handler still holds the release receiver");
    tokio::time::timeout(MARKER, shutting_down)
        .await
        .expect("the shutdown must return once the handler is released")
        .expect("the shutdown task must end normally, never by panic");
    let took = started.elapsed();
    ticker.abort();

    assert!(
        took < Duration::from_millis(DRAIN_BUDGET_MS),
        "a timer firing every {TICK:?} must not hold the drain to its \
         {DRAIN_BUDGET_MS} ms deadline — the shutdown took {took:?}"
    );

    // Receipt 1 (positive): the refusal is recorded, by its canonical code.
    let refused = dead_letters_with_code(&db_path, "shutdown_draining");
    assert!(
        refused >= 1,
        "every tick fired into the drain is a NEW arrival and belongs in the \
         dead-letter queue under `shutdown_draining` — found {refused} rows"
    );

    // Receipt 2: and none of them was routed. The count stopped growing the
    // moment the drain began, which is the whole claim of this test.
    let arrived_total = arrivals.load(Ordering::SeqCst);
    assert_eq!(
        arrived_total, arrived_before_the_drain,
        "no timer message may reach /sink after the drain began — {arrived_before_the_drain} \
         before, {arrived_total} in total"
    );
    counting.abort();
}

/// The other half, and the one that would be easy to break: a follow-on hop is
/// NOT refused. Without this assertion the previous test would also pass on an
/// implementation that simply stops routing everything.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_follow_on_emission_during_the_drain_is_still_routed() {
    let td = colony_root(&format!(
        r#"{{"shutdown_drain_timeout_ms": {DRAIN_BUDGET_MS}}}"#
    ));
    let h = ColonyHandle::new_with_factories_at(&td, vec![]);

    let (entered_tx, mut entered_rx) = mpsc::channel::<()>(4);
    let (release_tx, release_rx) = oneshot::channel::<()>();
    let release = Arc::new(Mutex::new(Some(release_rx)));
    let (arrived_tx, mut arrived_rx) = mpsc::channel::<Path>(4);

    h.spawn(Path::new("/a"), {
        let entered = entered_tx.clone();
        let release = release.clone();
        move || BlockingCell {
            entered: entered.clone(),
            release: release.clone(),
            emit_to: Some(Path::new("/sink")),
        }
    })
    .await;
    h.spawn(Path::new("/sink"), {
        let arrived = arrived_tx.clone();
        move || EchoMockCell::new(Path::new("/sink")).tap_to(arrived.clone())
    })
    .await;
    h.add_edge(Uuid::now_v7(), Path::new("/a"), Path::new("/sink"))
        .await;

    h.send_from(Path::new("/"), MessageBuilder::new("/a").build())
        .await;
    tokio::time::timeout(MARKER, entered_rx.recv())
        .await
        .expect("the blocking handler at /a must be entered within the marker")
        .expect("the entered-channel sender lives in the cell");

    let arrival = tokio::spawn(async move { arrived_rx.recv().await.map(|p| (p, Instant::now())) });
    let mut shutting_down = tokio::spawn(async move { h.shutdown().await });
    assert!(
        tokio::time::timeout(HELD_PROBE, &mut shutting_down)
            .await
            .is_err(),
        "the shutdown must still be running while /a is inside handle()"
    );

    // /a emits WITH a parent (it consumed the message above) while the drain is
    // running. That is the work being drained, and it must arrive.
    release_tx
        .send(())
        .expect("the blocked handler still holds the release receiver");
    tokio::time::timeout(MARKER, shutting_down)
        .await
        .expect("the shutdown must return once the follow-on has been carried")
        .expect("the shutdown task must end normally, never by panic");
    let teardown_done_at = Instant::now();

    let (reached, arrived_at) = tokio::time::timeout(MARKER, arrival)
        .await
        .expect("the arrival probe must resolve within the failure marker")
        .expect("the arrival probe must end normally, never by panic")
        .expect("/sink must have handled the follow-on emitted during the drain");
    assert_eq!(reached.as_str(), "/sink");
    assert!(
        arrived_at <= teardown_done_at,
        "a follow-on emission is carried BEFORE the teardown — arrived {arrived_at:?}, \
         teardown finished {teardown_done_at:?}"
    );
}
