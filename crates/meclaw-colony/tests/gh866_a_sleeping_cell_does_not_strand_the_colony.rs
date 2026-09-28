//! GH #866 — a stateful cell that falls asleep does not strand the colony.
//!
//! Measured on a throwaway colony (0.44.0): a `store` that sleeps 5 s after its
//! answer, with the two `fsync`s its `cell.db` close does (the WAL checkpoint)
//! delayed by 700 ms in that process only, tripped the watchdog in 11 of 11
//! minutes -- `colony_loop`, `witness=kept`, `supervisor_lag=0`, the signature
//! of the live colony's 124 fatal trips -- at the moment the store slept. The
//! same colony with the delay on its spawn journal alone tripped in 0 of 30.
//!
//! The mechanism: the idle arm of `cell_task_stateful` sent `ColonyMsg::Sleep`
//! and then returned, and the return dropped the cell and closed its
//! `cell.db` on the runtime worker. The send had just woken the colony task
//! into THIS worker's LIFO slot, which no other worker may steal; the close
//! then held the worker, and the colony with it, for as long as the disk took.
//!
//! The lock makes the teardown deterministic without a disk: the cell's own
//! `Drop` blocks for [`BLOCK`] = 800 ms, standing in for the checkpoint. The
//! test asks the colony a question it answers itself (`ReadInboundEdges`) every
//! 50 ms while the cell falls asleep. Kept on the worker, the teardown holds
//! the colony for most of [`BLOCK`] and the shipped supervisor calls it
//! `colony_loop`; off the worker the lookup answers in microseconds. The bar
//! sits at [`PROBE_BAR`] = 250 ms -- a semantic discriminator, not a race.
//!
//! Receipts are positive: the teardown really ran (the `Drop` reports on a
//! channel), and every probe was answered.

use meclaw_colony::watchdog::{Beat, HEARTBEAT_CAPACITY, WatchdogOnTrip, WatchdogTrip};
use meclaw_colony::{
    CellFactoryRegistry, ColonyConfig, ColonyDb, ColonyMsg, ColonyTaskConfig, DbConn, StatefulCell,
    cell_task_stateful, colony_task,
};
use meclaw_core::{CellEmission, Message, MessageBuilder, OutputSink, Path};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

/// How long the cell's teardown blocks: the stand-in for a WAL checkpoint
/// under write-back pressure (measured: 0.5 s and more).
const BLOCK: Duration = Duration::from_millis(800);
/// The worst probe latency the colony may show while the cell falls asleep.
const PROBE_BAR: Duration = Duration::from_millis(250);
/// How long the cell idles before it sleeps.
const IDLE: Duration = Duration::from_millis(200);
/// Long enough to cover the idle wait and the whole teardown.
const PROBE_WINDOW: Duration = Duration::from_millis(1500);
const PROBE_EVERY: Duration = Duration::from_millis(50);
/// Failure-marker budget (30 s convention); bounds "did it ever happen" only.
const MARKER: Duration = Duration::from_secs(30);

/// A stateful cell whose teardown blocks its thread, as a `cell.db` close does
/// on a busy disk. It handles nothing; what is under test is how it goes.
struct SlowTeardown {
    dropped: std::sync::mpsc::Sender<()>,
}

impl StatefulCell for SlowTeardown {
    // The trait spells its future out; so does every implementation of it.
    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a mut self,
        _msg: Message,
        _sink: &'a OutputSink,
        _db: &'a mut DbConn,
    ) -> impl std::future::Future<Output = ()> + Send + 'a {
        async {}
    }
}

impl Drop for SlowTeardown {
    fn drop(&mut self) {
        std::thread::sleep(BLOCK);
        let _ = self.dropped.send(());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sleeping_cell_does_not_strand_the_colony() {
    let td = tempfile::TempDir::new().expect("tempdir");

    // The colony, with its heartbeat, judged live by the shipped supervisor
    // and its independent witness (five periods of 100 ms).
    let (hb_tx, hb_rx) = mpsc::channel::<Beat>(HEARTBEAT_CAPACITY);
    let (inbox_tx, inbox_rx) = mpsc::channel::<ColonyMsg>(256);
    let (outputs_tx, outputs_rx) = mpsc::channel(256);
    let db = ColonyDb::open(&td.path().join("colony.db")).expect("open colony.db");
    let colony_join = tokio::spawn(colony_task(
        ColonyTaskConfig::new(
            inbox_tx.clone(),
            inbox_rx,
            outputs_tx,
            outputs_rx,
            db,
            CellFactoryRegistry::new(),
            td.path().to_path_buf(),
            ColonyConfig {
                shutdown_drain_timeout_ms: 0,
                ..ColonyConfig::default()
            },
            None,
            None,
        )
        .with_heartbeat(hb_tx),
    ));
    let (trip_tx, mut trip_rx) = mpsc::channel::<WatchdogTrip>(64);
    let (armed_tx, armed_rx) = oneshot::channel::<()>();
    let (witness_tx, witness_rx) = mpsc::channel::<()>(8);
    let witness = tokio::spawn(meclaw_colony::watchdog::run_liveness_witness(
        witness_tx,
        Duration::from_millis(100),
    ));
    let watchdog = tokio::spawn(meclaw_colony::watchdog::run_watchdog(
        hb_rx,
        trip_tx,
        5,
        Duration::from_millis(100),
        armed_rx,
        WatchdogOnTrip::Exit,
        Some(witness_rx),
    ));

    // Warm-up: the first answer waits for the colony to finish booting, which
    // is not what the probe measures (measured: ~160 ms on a loaded host).
    let (warm_tx, warm_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::ReadInboundEdges {
            of: Path::new("/s"),
            ack: warm_tx,
        })
        .await
        .expect("colony inbox closed");
    tokio::time::timeout(MARKER, warm_rx)
        .await
        .expect("the colony answers its first question within the failure marker")
        .expect("the ack channel stays open");

    // Armed only now, after the warm-up answer, the way the binary arms its
    // supervisor only after the bootstrap (`meclaw-cli` lib.rs, Issue #6,
    // defect 1): boot is not what this lock measures. Armed at spawn, the lock
    // tripped on a loaded CI runner before the colony loop had beaten once --
    // main CI run 36351942443 (0.47.1), `beats_seen: 0` and
    // `armed_for == silent_for` = 400.9 ms, the fifth empty period after
    // arming, while the probes stayed at worst 0 ms / 2 ms (GH #876).
    // Period, threshold, witness and the probe bar are unchanged; the whole
    // blockade below still runs under the armed supervisor.
    let _ = armed_tx.send(());

    // One stateful cell on the production task, talking to this colony.
    let (dropped_tx, dropped_rx) = std::sync::mpsc::channel::<()>();
    let (cell_tx, cell_rx) = mpsc::channel::<Message>(8);
    let (cell_out_tx, _cell_out_rx) = mpsc::channel::<CellEmission>(8);
    let cell_db = DbConn::wrap(
        rusqlite::Connection::open_in_memory().expect("an in-memory cell.db"),
        None,
    );
    let cell_join = tokio::spawn(cell_task_stateful(
        Path::new("/s"),
        cell_rx,
        cell_out_tx,
        SlowTeardown {
            dropped: dropped_tx,
        },
        cell_db,
        Some(IDLE),
        None, // message_timeout
        None, // peace_tx
        None, // backstop_tx
        Some(inbox_tx.clone()),
        0,    // cell_timeout: idle despawn, not one-shot
        None, // stop_rx
        None, // death_ack
        None, // blob_store
        None, // consumes
        Default::default(),
    ));
    cell_tx
        .send(MessageBuilder::new(Path::new("/s")).build())
        .await
        .expect("the cell takes its message");

    // The probe, while the cell idles, sleeps and tears down.
    let started = Instant::now();
    let mut worst = Duration::ZERO;
    let mut probes = 0usize;
    while started.elapsed() < PROBE_WINDOW {
        let asked = Instant::now();
        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::ReadInboundEdges {
                of: Path::new("/s"),
                ack: ack_tx,
            })
            .await
            .expect("colony inbox closed");
        tokio::time::timeout(MARKER, ack_rx)
            .await
            .expect("the colony answers within the failure marker")
            .expect("the ack channel stays open");
        worst = worst.max(asked.elapsed());
        probes += 1;
        // A thread sleep on purpose: the test body runs on the test's own
        // thread, not on a runtime worker, and a tokio timer is driven by the
        // workers -- with every worker held it would not fire, the probe would
        // not be sent, and a blocked colony would never be asked (the first
        // run of this lock measured 2 probes in the window and passed so).
        std::thread::sleep(PROBE_EVERY);
    }

    // Receipt: the teardown really ran, and the cell task ended peacefully.
    tokio::task::spawn_blocking(move || dropped_rx.recv_timeout(MARKER))
        .await
        .expect("the receipt thread returns")
        .expect("the cell's teardown ran within the failure marker");
    tokio::time::timeout(MARKER, cell_join)
        .await
        .expect("the cell task ends within the failure marker")
        .expect("the cell task ends without a panic");

    let mut trips: Vec<WatchdogTrip> = Vec::new();
    while let Ok(t) = trip_rx.try_recv() {
        trips.push(t);
    }
    eprintln!(
        "gh866: {probes} probes, worst {} ms; trips {:?}",
        worst.as_millis(),
        trips.iter().map(|t| t.starved()).collect::<Vec<_>>()
    );

    assert!(
        worst < PROBE_BAR,
        "the colony answered a probe only after {} ms while a cell that fell asleep \
         tore down for {} ms: the teardown ran on the runtime worker the `Sleep` had just \
         woken the colony onto (GH #866)",
        worst.as_millis(),
        BLOCK.as_millis()
    );
    assert!(
        !trips
            .iter()
            .any(|t| matches!(t.starved(), "colony_loop" | "stuck_work_item")),
        "a cell's teardown is the cell's wait, never a fatal verdict on the colony -- \
         trips were {trips:?}"
    );

    drop(cell_tx);
    let (ack_tx, ack_rx) = oneshot::channel();
    let _ = inbox_tx.send(ColonyMsg::Shutdown { ack: ack_tx }).await;
    let _ = tokio::time::timeout(MARKER, ack_rx).await;
    let _ = tokio::time::timeout(MARKER, colony_join).await;
    watchdog.abort();
    witness.abort();
}
