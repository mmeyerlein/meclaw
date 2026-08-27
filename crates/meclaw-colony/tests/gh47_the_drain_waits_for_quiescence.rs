//! GH #47: the drain waits for the system to go idle, not for a number of
//! answers. That distinction is the whole reason this is buildable in a
//! non-deterministic substrate (`docs/roadmap.md` § Async cell shutdown drain,
//! clarification note): a quiescence drain waits for *system idle*, never for
//! "N replies", so a message that died on the way contributes nothing to the
//! wait.
//!
//! Every receipt here is positive: a cell that announced it was inside
//! `handle()`, a capture cell that was reached, a dead-letter row read back by
//! its code. Nothing concludes anything from an absence.

use meclaw_core::serde_json::json;
use meclaw_core::{Cell, CellOutput, Message, OutputSink, Path, Uuid};
use meclaw_testing::mocks::EchoMockCell;
use meclaw_testing::{ColonyHandle, MessageBuilder};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

/// Failure marker, generous per the 30s convention (robust under cargo load).
const MARKER: Duration = Duration::from_secs(30);

/// The drain budget the first two colonies here are given. Far larger than any
/// discriminator below, so "the shutdown was held" can never be confused with
/// "the shutdown ran into its deadline".
const DRAIN_BUDGET_MS: u64 = 30_000;

/// SEMANTIC discriminator: how long the shutdown must still be running while a
/// handler blocks. Tight on purpose — a sixtieth of the drain budget. Anything
/// longer would also pass on a colony that is merely slow; anything shorter
/// would measure the scheduler rather than the drain.
const HELD_PROBE: Duration = Duration::from_millis(500);

/// A colony root whose `colony.json` carries exactly `body`.
fn colony_root(body: &str) -> tempfile::TempDir {
    let td = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(td.path().join("colony.json"), body).expect("write the test colony.json");
    td
}

/// A cell whose `handle()` parks on a `oneshot` until the test releases it,
/// announces on `entered` that it is inside the handler, and — once released —
/// emits one message.
///
/// The announcement is the positive receipt the held-shutdown tests rest on: the
/// handler is running, therefore the cell TOOK the message out of its mailbox,
/// therefore the mailbox is empty. That is the exact state the pre-#47 shutdown
/// read as "done".
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
                                "text": "the work the drain saved",
                            }]
                        }),
                    })
                    .await;
            }
        }
    }
}

/// A cell that takes its time before passing the message on — one hop of the
/// ping-pong in the TTL test. Without the delay the cascade would be over before
/// the shutdown could be sent, and the test would prove nothing about the drain.
struct SlowRelayCell {
    own_path: Path,
    target: Path,
    per_hop: Duration,
}

impl Cell for SlowRelayCell {
    #[allow(clippy::manual_async_fn)]
    fn handle(
        &mut self,
        _msg: Message,
        sink: &OutputSink,
    ) -> impl std::future::Future<Output = ()> + Send {
        let own_path = self.own_path.clone();
        let target = self.target.clone();
        let per_hop = self.per_hop;
        let sink = sink.clone();
        async move {
            tokio::time::sleep(per_hop).await;
            let _ = sink
                .push(CellOutput {
                    target,
                    content: json!({
                        "messages": [{
                            "origin": "assistant",
                            "type": "text",
                            "text": format!("hop via {}", own_path.as_str()),
                        }]
                    }),
                })
                .await;
        }
    }
}

/// A handler that is still running holds the shutdown, and releasing it releases
/// the shutdown. Both halves, because only the pair proves the mechanism: the
/// first alone could be a hang, the second alone could be a coincidence.
///
/// The third receipt is the point of the whole lane: the message the blocked
/// handler emitted reached the capture cell BEFORE the teardown ran. A shutdown
/// that cut at the old moment would have thrown that emission away.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_running_handler_holds_the_shutdown_until_it_finishes() {
    let td = colony_root(&format!(
        r#"{{"shutdown_drain_timeout_ms": {DRAIN_BUDGET_MS}}}"#
    ));
    let h = ColonyHandle::new_with_factories_at(&td, vec![]);

    let (entered_tx, mut entered_rx) = mpsc::channel::<()>(4);
    let (release_tx, release_rx) = oneshot::channel::<()>();
    let release = Arc::new(Mutex::new(Some(release_rx)));
    let (arrived_tx, mut arrived_rx) = mpsc::channel::<Path>(4);

    h.spawn(Path::new("/blocker"), {
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
    // An emission is routed by the EMITTING cell's out-edges, so without this
    // edge the follow-on would dead-letter as `no_route` whatever its target
    // field says.
    h.add_edge(Uuid::now_v7(), Path::new("/blocker"), Path::new("/sink"))
        .await;

    h.send_from(Path::new("/"), MessageBuilder::new("/blocker").build())
        .await;

    // Positive receipt: the handler is INSIDE `handle()`, so the mailbox has
    // already handed the message over and is empty again.
    tokio::time::timeout(MARKER, entered_rx.recv())
        .await
        .expect("the blocking handler must be entered within the failure marker")
        .expect("the entered-channel sender lives in the cell");

    // Stamps the moment /sink actually handled the follow-on, so the ordering
    // against the teardown is measured rather than assumed.
    let arrival = tokio::spawn(async move { arrived_rx.recv().await.map(|p| (p, Instant::now())) });

    let mut shutting_down = tokio::spawn(async move { h.shutdown().await });
    assert!(
        tokio::time::timeout(HELD_PROBE, &mut shutting_down)
            .await
            .is_err(),
        "the shutdown must still be running while a handler is in flight — \
         an empty mailbox is not a finished handler"
    );

    release_tx
        .send(())
        .expect("the blocked handler still holds the release receiver");
    tokio::time::timeout(MARKER, shutting_down)
        .await
        .expect("the shutdown must return once the handler is released")
        .expect("the shutdown task must end normally, never by panic");
    let teardown_done_at = Instant::now();

    let (reached, arrived_at) = tokio::time::timeout(MARKER, arrival)
        .await
        .expect("the arrival probe must resolve within the failure marker")
        .expect("the arrival probe must end normally, never by panic")
        .expect("the capture cell at /sink must have handled the drained emission");
    assert_eq!(
        reached.as_str(),
        "/sink",
        "the tap names the cell that handled the emission"
    );
    assert!(
        arrived_at <= teardown_done_at,
        "the drain must carry the emission to /sink BEFORE it tears the colony \
         down — arrived {arrived_at:?}, teardown finished {teardown_done_at:?}"
    );
}

/// The follow-on hop of an in-flight message is carried too — a drain that only
/// waited for the first handler would still lose the answer on its way through a
/// second cell.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_follow_on_hop_is_carried_to_its_end() {
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
            emit_to: Some(Path::new("/b")),
        }
    })
    .await;
    h.spawn(Path::new("/b"), || {
        EchoMockCell::new(Path::new("/b")).emitted_target(Path::new("/sink"))
    })
    .await;
    h.spawn(Path::new("/sink"), {
        let arrived = arrived_tx.clone();
        move || EchoMockCell::new(Path::new("/sink")).tap_to(arrived.clone())
    })
    .await;
    h.add_edge(Uuid::now_v7(), Path::new("/a"), Path::new("/b"))
        .await;
    h.add_edge(Uuid::now_v7(), Path::new("/b"), Path::new("/sink"))
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

    release_tx
        .send(())
        .expect("the blocked handler still holds the release receiver");
    tokio::time::timeout(MARKER, shutting_down)
        .await
        .expect("the shutdown must return once the cascade has run out")
        .expect("the shutdown task must end normally, never by panic");
    let teardown_done_at = Instant::now();

    let (reached, arrived_at) = tokio::time::timeout(MARKER, arrival)
        .await
        .expect("the arrival probe must resolve within the failure marker")
        .expect("the arrival probe must end normally, never by panic")
        .expect("/sink must have been reached — the second hop is carried too");
    assert_eq!(reached.as_str(), "/sink");
    assert!(
        arrived_at <= teardown_done_at,
        "the drain must carry BOTH hops before the teardown — /sink handled at \
         {arrived_at:?}, teardown finished {teardown_done_at:?}"
    );
}

/// Ruling O4: TTL keeps decrementing during the drain. A two-cell ping-pong
/// started before the shutdown dies on TTL and does NOT hold the drain to its
/// deadline.
///
/// TTL is the only termination guarantee against a cycle. Freezing it — or
/// topping it up — would turn this cascade into a drain that runs its full
/// budget every time, and would change a message's fate mid-flight.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ttl_keeps_running_during_the_drain() {
    /// Hops the ping-pong is allowed before TTL kills it.
    const TTL: u32 = 40;
    /// Per-hop delay, so the cascade is still running when the shutdown lands.
    const PER_HOP: Duration = Duration::from_millis(20);
    /// SEMANTIC lower bound: the drain really carried the cascade for a while
    /// instead of cutting it at the old moment. A quarter of the nominal
    /// cascade length (40 × 20 ms = 800 ms), so scheduler jitter cannot reach it.
    const CARRIED_AT_LEAST: Duration = Duration::from_millis(200);
    /// SEMANTIC upper bound: TTL, not the deadline, is what ended the drain.
    /// A third of the 30 s budget — a colony that sat out its deadline can never
    /// be mistaken for one that ran out of TTL.
    const WELL_UNDER_BUDGET: Duration = Duration::from_secs(10);

    let td = colony_root(&format!(
        r#"{{"shutdown_drain_timeout_ms": {DRAIN_BUDGET_MS}, "message_default_ttl": {TTL}}}"#
    ));
    let db_path = td.path().join("colony.db");
    let h = ColonyHandle::new_with_factories_at(&td, vec![]);

    h.spawn(Path::new("/ping"), || SlowRelayCell {
        own_path: Path::new("/ping"),
        target: Path::new("/pong"),
        per_hop: PER_HOP,
    })
    .await;
    h.spawn(Path::new("/pong"), || SlowRelayCell {
        own_path: Path::new("/pong"),
        target: Path::new("/ping"),
        per_hop: PER_HOP,
    })
    .await;
    h.add_edge(Uuid::now_v7(), Path::new("/ping"), Path::new("/pong"))
        .await;
    h.add_edge(Uuid::now_v7(), Path::new("/pong"), Path::new("/ping"))
        .await;

    h.send_from(
        Path::new("/"),
        MessageBuilder::new("/ping").with_ttl(TTL).build(),
    )
    .await;
    // Barrier: the colony inbox is FIFO and the loop is one task, so an ack for
    // a LATER message proves the route above was handled first — the ping-pong
    // is under way when the shutdown arrives.
    h.add_hive_scope(Path::new("/barrier")).await;

    let started = Instant::now();
    tokio::time::timeout(MARKER, h.shutdown())
        .await
        .expect("the shutdown must return within the failure marker");
    let took = started.elapsed();

    assert!(
        took >= CARRIED_AT_LEAST,
        "the drain must carry the running cascade, not cut it — took {took:?}"
    );
    assert!(
        took < WELL_UNDER_BUDGET,
        "a running TTL must end the cascade long before the {DRAIN_BUDGET_MS} ms \
         deadline — took {took:?}"
    );

    // Positive receipt for the mechanism that ended it: the DLQ names TTL.
    let conn = rusqlite::Connection::open(&db_path)
        .expect("a fresh connection must open the file the joined writer left behind");
    let ttl_deaths: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM dead_letters WHERE error_code = 'ttl_expired'",
            [],
            |r| r.get(0),
        )
        .expect("the dead_letters table must exist in colony.db");
    assert_eq!(
        ttl_deaths, 1,
        "exactly the one ping-pong message must have died on TTL, not on the deadline"
    );
}
