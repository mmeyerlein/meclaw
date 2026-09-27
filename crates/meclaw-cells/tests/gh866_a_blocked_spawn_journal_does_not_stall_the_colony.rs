//! GH #866 — a child spawn that waits on the disk does not stall the colony.
//!
//! Measured on a live colony (2026-09-23..26): 124 fatal `colony_loop` trips,
//! every one with `witness=kept`, `supervisor_lag` 0 ms and no work in flight,
//! 87 of them starting in second :00 of a minute -- the second in which that
//! colony spawns its cold `python3` children -- and all of them in hours with
//! heavy write-back on the host. The spawn path of a `code` cell did its
//! blocking syscalls on a runtime worker: `fork` + `pre_exec` + the wait for
//! `exec`, then the orphan journal's `open`/`write`/`fdatasync` for the
//! `spawned` record, and again for the `exited` record when the note dropped.
//! Under write-back pressure an ext4 `fdatasync` waits for the journal commit,
//! hundreds of milliseconds, and the worker it runs on polls nothing else in
//! the meantime -- including the colony task, when that sat in the worker's
//! LIFO slot (it cannot be stolen) or when every worker was held that way.
//!
//! The lock makes the disk deterministic without touching the host's disk:
//! the journal is a FIFO. A writer's `open` of a FIFO blocks until a reader
//! opens it, and the reader here opens only every [`BLOCK`] -- so every
//! journal record costs its spawn path a blocking syscall of up to [`BLOCK`],
//! exactly where the `fdatasync` sat. (`sync_data` on a FIFO fails with
//! `EINVAL`; `append` logs that and moves on, which is the documented
//! degradation and irrelevant here.)
//!
//! Six cold `code` cells get one message each at once, on the production
//! runtime width (`worker_threads = 4`, `crates/meclaw-cli/src/main.rs`). While
//! their children are spawned and retired, the test asks the colony a question
//! it answers itself (`ReadInboundEdges`, an in-memory lookup) every 50 ms.
//!
//! The discriminator is semantic, not a race: with the journal on a worker six
//! blocked spawn paths hold all four workers for up to [`BLOCK`] = 800 ms, so a
//! probe waits most of that; off the workers the lookup answers in
//! microseconds. The bar sits at [`PROBE_BAR`] = 250 ms, a third of the
//! blockade and fifty times the answer.
//!
//! Receipts are positive, at the receiver: every probe answered (and how fast),
//! every cell answered with `exit_code` 0, and the reader of the FIFO counted
//! at least twelve records -- the `spawned` and the `exited` record of each
//! child -- so the blockade really stood on the spawn path.
//!
//! Own test binary: installing the process-wide journal is once per process.

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::orphan_journal::{self, OrphanJournal};
use meclaw_colony::watchdog::{Beat, HEARTBEAT_CAPACITY, WatchdogOnTrip, WatchdogTrip};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyConfig, ColonyDb, ColonyMsg, ColonyTaskConfig,
    SpawnedCellKind, colony_task,
};
use meclaw_core::serde_json::json;
use meclaw_core::{Body, MessageBuilder, Path};
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

/// How long every journal `open` waits for the reader: the stand-in for an
/// `fdatasync` under write-back pressure (measured: 0.5 s and more).
const BLOCK: Duration = Duration::from_millis(800);
/// The worst probe latency the colony may show while the spawns block.
const PROBE_BAR: Duration = Duration::from_millis(250);
/// How long the probe runs: long enough to cover the `spawned` and the
/// `exited` blockade of every child.
const PROBE_WINDOW: Duration = Duration::from_millis(1200);
const PROBE_EVERY: Duration = Duration::from_millis(50);
/// Six cold cells, one message each -- more spawn paths than workers.
const CELLS: usize = 6;
/// Failure-marker budget (30 s convention); bounds "did it ever happen" only.
const MARKER: Duration = Duration::from_secs(30);

const SCRIPT: &str =
    "import json, sys\njson.load(sys.stdin)\nsys.stdout.write(json.dumps({'messages': []}))\n";

/// The FIFO's only reader: opens every [`BLOCK`], reads to EOF, closes, and
/// reports how many records (lines) it read. Non-blocking open on purpose -- a
/// blocking one would itself wait for the next writer and release it at once.
fn run_reader(
    fifo: std::path::PathBuf,
    stop: std::sync::mpsc::Receiver<()>,
    lines: std::sync::mpsc::Sender<usize>,
) {
    loop {
        match stop.recv_timeout(BLOCK) {
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            _ => return,
        }
        let Ok(mut f) = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&fifo)
        else {
            return;
        };
        let mut buf = [0u8; 4096];
        let mut n_lines = 0usize;
        loop {
            match f.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n_lines += buf[..n].iter().filter(|b| **b == b'\n').count(),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(_) => break,
            }
        }
        drop(f);
        if n_lines > 0 && lines.send(n_lines).is_err() {
            return;
        }
    }
}

fn mkfifo(path: &std::path::Path) {
    let status = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("mkfifo runs");
    assert!(status.success(), "mkfifo {path:?} failed: {status:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_blocked_spawn_journal_does_not_stall_the_colony() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let fifo = td.path().join("journal.fifo");
    mkfifo(&fifo);
    assert!(
        orphan_journal::install(OrphanJournal::at(fifo.clone())),
        "this binary installs the journal exactly once"
    );
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    let (lines_tx, lines_rx) = std::sync::mpsc::channel::<usize>();
    let reader = {
        let fifo = fifo.clone();
        std::thread::spawn(move || run_reader(fifo, stop_rx, lines_tx))
    };

    // The colony, with its heartbeat, judged live by the shipped supervisor
    // and its independent witness (five periods of 100 ms, `lib.rs`).
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
    let _ = armed_tx.send(());

    // Six cold `code` cells, no sandbox: what is under test is the journal on
    // the spawn path, not the profile.
    let (cell_out_tx, mut cell_out_rx) = mpsc::channel(64);
    let mut senders = Vec::with_capacity(CELLS);
    let mut dirs = Vec::with_capacity(CELLS);
    for i in 0..CELLS {
        let dir = tempfile::TempDir::new().expect("a cell dir");
        let spawned = Arc::new(CodeCellFactory)
            .spawn_cell(
                Path::new(&format!("/c{i}")),
                json!({
                    "runner": "python3",
                    "runner_mode": "cold",
                    "script_inline": SCRIPT,
                    "external_timeout_ms": 20000
                }),
                cell_out_tx.clone(),
                dir.path().to_path_buf(),
                meclaw_colony::ContractView::default(),
                inbox_tx.clone(),
                None,
                0,
                None,
                None,
                64,
            )
            .expect("a cold code cell spawns");
        match spawned {
            SpawnedCellKind::Active { sender, .. } => senders.push(sender),
            SpawnedCellKind::Dormant { .. } => unreachable!("code spawns Active"),
        }
        dirs.push(dir);
    }

    // Warm-up: the first answer waits for the colony to finish booting, which
    // is not what the probe measures (measured: ~160 ms on a loaded host).
    let (warm_tx, warm_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::ReadInboundEdges {
            of: Path::new("/c0"),
            ack: warm_tx,
        })
        .await
        .expect("colony inbox closed");
    tokio::time::timeout(MARKER, warm_rx)
        .await
        .expect("the colony answers its first question within the failure marker")
        .expect("the ack channel stays open");

    // t0: one message into every cell at once.
    for (i, sender) in senders.iter().enumerate() {
        sender
            .send(
                MessageBuilder::new(Path::new(&format!("/c{i}")))
                    .body(Body::Inline(json!({"messages": []})))
                    .reply_to(Path::new("/sink"))
                    .build(),
            )
            .await
            .expect("the mailbox takes the message");
    }

    // The probe: a question the colony answers itself, every 50 ms.
    let started = Instant::now();
    let mut worst = Duration::ZERO;
    let mut probes = 0usize;
    while started.elapsed() < PROBE_WINDOW {
        let asked = Instant::now();
        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::ReadInboundEdges {
                of: Path::new("/c0"),
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

    // Receipt 1: every cell answered, and its child really ran.
    for n in 0..CELLS {
        let em = tokio::time::timeout(MARKER, cell_out_rx.recv())
            .await
            .unwrap_or_else(|_| panic!("cell answer {n} arrived within the marker"))
            .expect("the cells answer");
        assert_eq!(
            em.content["header"]["exit_code"], 0,
            "answer {n}: {:?}",
            em.content
        );
    }
    // Receipt 2: the blockade stood on the spawn path -- the reader took the
    // `spawned` and the `exited` record of every child out of the FIFO.
    let deadline = Instant::now() + MARKER;
    let mut records = 0usize;
    while records < 2 * CELLS {
        while let Ok(n) = lines_rx.try_recv() {
            records += n;
        }
        if records >= 2 * CELLS {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the FIFO reader saw only {records} of {} journal records",
            2 * CELLS
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let mut trips: Vec<WatchdogTrip> = Vec::new();
    while let Ok(t) = trip_rx.try_recv() {
        trips.push(t);
    }
    eprintln!(
        "gh866: {probes} probes, worst {} ms; {records} journal records; trips {:?}",
        worst.as_millis(),
        trips.iter().map(|t| t.starved()).collect::<Vec<_>>()
    );

    assert!(
        worst < PROBE_BAR,
        "the colony answered a probe only after {} ms while six spawn paths waited on \
         their journal ({} ms each): the blocking syscalls of a spawn ran on the runtime \
         workers and held the colony task (GH #866)",
        worst.as_millis(),
        BLOCK.as_millis()
    );
    assert!(
        !trips
            .iter()
            .any(|t| matches!(t.starved(), "colony_loop" | "stuck_work_item")),
        "a journal that waits on the disk is the spawn path's wait, never a fatal verdict on \
         the colony -- trips were {trips:?}"
    );

    let _ = stop_tx.send(());
    let _ = reader.join();
    let (ack_tx, ack_rx) = oneshot::channel();
    let _ = inbox_tx.send(ColonyMsg::Shutdown { ack: ack_tx }).await;
    let _ = tokio::time::timeout(MARKER, ack_rx).await;
    let _ = tokio::time::timeout(MARKER, colony_join).await;
    watchdog.abort();
    witness.abort();
    drop(dirs);
}
