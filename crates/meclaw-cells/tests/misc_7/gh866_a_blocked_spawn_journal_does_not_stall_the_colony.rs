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
//! opens it, and the reader here stays closed until the test releases it -- so
//! every spawn path waits in its journal record exactly where the `fdatasync`
//! sat, for as long as the test wants. (`sync_data` on a FIFO fails with
//! `EINVAL`; `append` logs that and moves on, which is the documented
//! degradation and irrelevant here.)
//!
//! Six cold `code` cells get one message each at once, on the production
//! runtime width (`worker_threads = 4`, `crates/meclaw/src/main.rs`). Both
//! records are blocked, one after the other, because each one alone stalled
//! the colony:
//!
//! 1. `spawned`: each child marks a file (with its pid) before it reads its
//!    input; six marks mean six spawn paths stand in their journal `open` --
//!    more than there are workers.
//! 2. `exited`: the reader takes exactly the six `spawned` records and shuts
//!    the FIFO again. The children answer and then hold on a go (a FIFO whose
//!    only writer is the test). The go lets them exit; once all six are reaped
//!    (gone from `/proc`) every spawn path stands in its `retire`, again in
//!    the journal `open`.
//!
//! The discriminator is an order of events, not a time bar: in each phase,
//! while all six wait, the colony answers a question it answers itself
//! (`ReadInboundEdges`, an in-memory lookup). With that record on the
//! workers, six blocked paths hold all four and the answer never comes (a
//! `HANG:` or, with every worker held, the runner's kill); off the workers it
//! comes at once. Only then is the reader released.
//!
//! Receipts are positive, at the receiver: both probes answered while the
//! journal was shut, every cell answered with `exit_code` 0, and the reader
//! took six records in each phase -- the `spawned` and the `exited` record of
//! each child -- so both blockades really stood on the spawn path.
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
use meclaw_testing::{EmissionsExt, deadline, wait};
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Six cold cells, one message each -- more spawn paths than workers.
const CELLS: usize = 6;
/// The child: opens the go (the test holds its only writer, so this never
/// blocks), marks `marks` with its pid, answers its input, then waits for the
/// go's EOF. The mark comes before the input, and the input only after the
/// spawn path left its `spawned` record, so a mark says "this spawn path
/// stands in the journal"; the exit comes only after the go.
fn script(marks: &std::path::Path, go: &std::path::Path) -> String {
    format!(
        "import json, os, sys\ng = open({:?}, 'rb')\n\
         open({:?}, 'a').write('m %d\\n' % os.getpid())\njson.load(sys.stdin)\n\
         sys.stdout.write(json.dumps({{'messages': []}}))\nsys.stdout.flush()\ng.read()\n",
        go.to_str().expect("a utf-8 temp path"),
        marks.to_str().expect("a utf-8 temp path")
    )
}

/// The FIFO's only reader, in two phases: each waits for its `release`, opens
/// the FIFO, takes exactly [`CELLS`] records (lines), shuts it again and
/// reports the count. Between the phases nothing can write: the six `spawned`
/// records are in, and no child exits before the go.
///
/// Opened read-write: the reader is a writer of its own, so it never reads an
/// EOF between two journal writers and never closes in between -- a FIFO's
/// buffer goes with its last descriptor, and a record written into a pipe the
/// reader was about to close was lost (1 of 20 under load, 11 of 12 records
/// seen).
fn run_reader(
    fifo: std::path::PathBuf,
    release: std::sync::mpsc::Receiver<()>,
    taken: mpsc::UnboundedSender<usize>,
) {
    for _phase in 0..2 {
        if release.recv().is_err() {
            return;
        }
        let Ok(mut f) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&fifo)
        else {
            return;
        };
        let mut buf = [0u8; 4096];
        let mut seen = 0usize;
        while seen < CELLS {
            match f.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => seen += buf[..n].iter().filter(|b| **b == b'\n').count(),
            }
        }
        drop(f);
        if taken.send(seen).is_err() {
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
    let marks = td.path().join("marks");
    let go = td.path().join("go.fifo");
    mkfifo(&go);
    // The go's only writer: read-write never blocks on a FIFO, and the
    // children's read-only `open` finds it there. Dropping it is the go.
    let go_holder = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&go)
        .expect("the go FIFO opens");
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let (taken_tx, mut taken_rx) = mpsc::unbounded_channel::<usize>();
    let reader = {
        let fifo = fifo.clone();
        std::thread::spawn(move || run_reader(fifo, release_rx, taken_tx))
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
                    "script_inline": script(&marks, &go),
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
    deadline::hang("the colony answers its first question", warm_rx)
        .await
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

    // Phase 1, `spawned`: every spawn path stands in its journal `open`, six
    // children marked.
    let pids = || -> Vec<u32> {
        std::fs::read_to_string(&marks)
            .map(|m| {
                m.lines()
                    .filter_map(|l| l.strip_prefix("m ")?.parse().ok())
                    .collect()
            })
            .unwrap_or_default()
    };
    // TV-Z5: a runner kill (TIMEOUT) prints the captured output only; this
    // line names the wait the test last stood in (recheck fix1 A-H1).
    eprintln!("gh866 phase 1: waiting for every child to run (journal `spawned`)");
    wait::until(
        "every child ran, so every spawn path waits on its `spawned` record",
        || {
            let n = pids().len();
            async move { n >= CELLS }
        },
    )
    .await;
    probe(
        &inbox_tx,
        "the colony answers while six spawn paths wait on their `spawned` record \
         (with the journal on the runtime workers it never does, GH #866)",
    )
    .await;
    assert_eq!(pids().len(), CELLS, "one child per cell, no second spawn");

    // The disk comes back for the six `spawned` records only: the reader takes
    // them and shuts the FIFO again before any child may exit.
    release_tx
        .send(())
        .expect("the reader waits for its first release");
    let spawned = deadline::hang("the reader took the six `spawned` records", taken_rx.recv())
        .await
        .expect("the reader reports phase 1");

    // Phase 2, `exited`: the go, then every child reaped -- each spawn path
    // stands in its `retire`, in the journal `open` again.
    drop(go_holder);
    let children = pids();
    eprintln!("gh866 phase 2: waiting for every child to be reaped (`/proc`, journal `exited`)");
    wait::until(
        "every child exited and was reaped, so every spawn path waits on its `exited` record",
        || {
            let left = children
                .iter()
                .filter(|pid| std::path::Path::new(&format!("/proc/{pid}")).exists())
                .count();
            async move { left == 0 }
        },
    )
    .await;
    probe(
        &inbox_tx,
        "the colony answers while six spawn paths wait on their `exited` record \
         (with `retire` on the runtime workers it never does, GH #866)",
    )
    .await;

    // Only now the disk comes back for good.
    release_tx
        .send(())
        .expect("the reader waits for its second release");

    // Receipt 1: every cell answered, and its child really ran.
    for n in 0..CELLS {
        let em = deadline::hang(&format!("cell answer {n}"), cell_out_rx.recv_answer())
            .await
            .expect("the cells answer");
        assert_eq!(
            em.content["header"]["exit_code"], 0,
            "answer {n}: {:?}",
            em.content
        );
    }
    // Receipt 2: both blockades stood on the spawn path -- the reader took the
    // six `spawned` records in phase 1 and the six `exited` records in phase 2.
    let exited = deadline::hang("the reader took the six `exited` records", taken_rx.recv())
        .await
        .expect("the reader reports phase 2");
    assert_eq!(
        (spawned, exited),
        (CELLS, CELLS),
        "one record of each kind per child"
    );
    let records = spawned + exited;

    let mut trips: Vec<WatchdogTrip> = Vec::new();
    while let Ok(t) = trip_rx.try_recv() {
        trips.push(t);
    }
    eprintln!(
        "gh866: {records} journal records; trips {:?}",
        trips.iter().map(|t| t.starved()).collect::<Vec<_>>()
    );
    assert!(
        !trips
            .iter()
            .any(|t| matches!(t.starved(), "colony_loop" | "stuck_work_item")),
        "a journal that waits on the disk is the spawn path's wait, never a fatal verdict on \
         the colony -- trips were {trips:?}"
    );

    let _ = reader.join();
    let (ack_tx, ack_rx) = oneshot::channel();
    let _ = inbox_tx.send(ColonyMsg::Shutdown { ack: ack_tx }).await;
    let _ = tokio::time::timeout(deadline::scaled(deadline::HANG), ack_rx).await;
    let _ = tokio::time::timeout(deadline::scaled(deadline::HANG), colony_join).await;
    watchdog.abort();
    witness.abort();
    drop(dirs);
}

/// The probe: a question the colony answers itself, awaited as a hang.
async fn probe(inbox_tx: &mpsc::Sender<ColonyMsg>, what: &str) {
    let (ack_tx, ack_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::ReadInboundEdges {
            of: Path::new("/c0"),
            ack: ack_tx,
        })
        .await
        .expect("colony inbox closed");
    deadline::hang(what, ack_rx)
        .await
        .expect("the ack channel stays open");
}
