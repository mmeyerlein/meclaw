//! GH #838 — a busy cell that is disconnected by a mutation never ends the colony.
//!
//! A mutation that deactivates a running cell peace-stops it and waits, inline in
//! the colony task, for the cell's death-ack (F5 variant A). A cell that is busy
//! inside `handle()` — an `llm` cell waiting on its provider is the everyday one —
//! cannot answer before its handler returns. Before this lock that wait was a
//! single silent stretch of up to `term_timeout` (5 s) PER CELL: the watchdog saw
//! a declared work item go quiet and, past `WORK_ITEM_BUDGET_FACTOR` × its window,
//! judged it `stuck_work_item` — fatal under the shipped `on_trip = exit`. And N
//! cells waited one after the other, N × `term_timeout` in one work item.
//!
//! Four parts of one promise, all measured against a REAL `colony_task`:
//!
//! 1. The wait is audible and bounded: it beats under a label that names the
//!    death-ack it is waiting for, so no trip inside it is fatal, and it still
//!    ends as a clean `Rejected{term_timeout}`.
//! 2. One deadline per mutation: however many cells are disconnected, the wait
//!    ends at one `term_timeout` after it began. A cell that would answer only
//!    later rejects the mutation at that deadline instead of extending it.
//! 3. A move waits for every relocated cell: all stops go out before the one
//!    deadline starts, so a cell behind a hung one is still waited for before
//!    its directory is renamed and a new task is spawned at the new address.
//! 4. The rollback after a `term_timeout` reject is bounded by a short grace,
//!    not by a second full `term_timeout`, and it beats like every other wait.

use meclaw_colony::watchdog::{Beat, WatchdogOnTrip, WatchdogTrip};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyConfig, ColonyDb, ColonyMsg, ColonyRuntime,
    ColonyTaskConfig, ContractView, DbConn, DiskBlobStore, LongRunningCell, MutationOutcome,
    SpawnedCellKind, bootstrap_from_filesystem, cell_task_long_running, colony_task,
    set_term_timeout_ms_for_test,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{
    Body, CellEmission, JsonValue, Message, MessageBuilder, OriginSink, OutputSink, Path, Uuid,
};
use meclaw_testing::factories::EchoCellFactory;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};

/// Every test here drives the process-global term-timeout to its own value; running
/// them in parallel would race the shared static. Held across `.await`, hence the
/// tokio mutex.
static TERM_TIMEOUT_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Generous outer failure marker for every wait in this file.
const FAILURE_MARKER: Duration = Duration::from_secs(30);

fn echo_registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert(
        "echo".into(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    );
    r
}

/// What happened, in the order it happened: `spawned <path>` from the factory,
/// `acked <path>` from a death-ack relay. The order is the assertion.
type EventLog = Arc<Mutex<Vec<String>>>;

fn events(log: &EventLog) -> Vec<String> {
    log.lock().expect("event log").clone()
}

/// The echo factory, noting every spawn before it happens — so a test can tell
/// whether a relocated cell's new task was built before its old one was down.
struct SpawnLogFactory {
    log: EventLog,
}

impl CellFactory for SpawnLogFactory {
    fn validate_params(&self, params: &JsonValue) -> Result<(), String> {
        EchoCellFactory.validate_params(params)
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        cell_dir: std::path::PathBuf,
        contract: ContractView,
        colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        idle_timeout: Option<Duration>,
        cell_timeout: i64,
        message_timeout: Option<Duration>,
        blob_store: Option<Arc<DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        self.log
            .lock()
            .expect("event log")
            .push(format!("spawned {}", path.as_str()));
        Arc::new(EchoCellFactory).spawn_cell(
            path,
            params,
            outputs_tx,
            cell_dir,
            contract,
            colony_inbox_tx,
            idle_timeout,
            cell_timeout,
            message_timeout,
            blob_store,
            mailbox_capacity,
        )
    }
}

/// A factory whose cell never answers the peace-stop: its task holds the
/// death-ack sender and parks forever. Stands in for a freshly instantiated
/// cell whose task still holds `cell.db` when the rollback stops it — the
/// worst case the rollback's wait is bounded against.
struct NeverAcksFactory;

impl CellFactory for NeverAcksFactory {
    fn validate_params(&self, _params: &JsonValue) -> Result<(), String> {
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_cell(
        self: Arc<Self>,
        _path: Path,
        _params: JsonValue,
        _outputs_tx: mpsc::Sender<CellEmission>,
        _cell_dir: std::path::PathBuf,
        _contract: ContractView,
        _colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<Duration>,
        _blob_store: Option<Arc<DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let (sender, mailbox) = mpsc::channel::<Message>(mailbox_capacity);
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let (death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
        let (peace_tx, peace_rx) = oneshot::channel::<()>();
        let (backstop_tx, backstop_rx) = oneshot::channel::<()>();
        let join = tokio::spawn(async move {
            let _keep = (mailbox, stop_rx, death_ack_tx, peace_tx, backstop_tx);
            std::future::pending::<()>().await
        });
        Ok(SpawnedCellKind::Active {
            sender,
            join,
            peace_rx,
            stop_tx,
            death_ack_rx,
            backstop_rx,
            respawn: Box::new(|| unreachable!("a never-acking cell is never restarted")),
        })
    }
}

/// Root hive + one echo cell `/a`, the target every stuck cell is wired to.
fn write_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main/a")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/a/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/a"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

/// A booted colony: the inbox, its outputs sender and the task.
struct Colony {
    inbox_tx: mpsc::Sender<ColonyMsg>,
    outputs_tx: mpsc::Sender<CellEmission>,
    join: tokio::task::JoinHandle<()>,
}

async fn boot(
    td: &TempDir,
    heartbeat: Option<mpsc::Sender<Beat>>,
    death_ack_signal: Option<mpsc::Sender<()>>,
) -> Colony {
    boot_with(td, heartbeat, death_ack_signal, echo_registry).await
}

async fn boot_with(
    td: &TempDir,
    heartbeat: Option<mpsc::Sender<Beat>>,
    death_ack_signal: Option<mpsc::Sender<()>>,
    registry: impl Fn() -> CellFactoryRegistry,
) -> Colony {
    write_topology(td.path());
    let db = ColonyDb::open(&td.path().join("colony.db")).expect("open colony.db");
    let (inbox_tx, inbox_rx) = mpsc::channel::<ColonyMsg>(1000);
    let (outputs_tx, outputs_rx) = mpsc::channel(1000);
    // A cell left stuck forever must not hold the shutdown drain for the
    // production 10 s; the harness budget of `ColonyHandle` is enough.
    let colony_config = ColonyConfig {
        shutdown_drain_timeout_ms: 250,
        ..ColonyConfig::default()
    };
    let mut cfg = ColonyTaskConfig::new(
        inbox_tx.clone(),
        inbox_rx,
        outputs_tx.clone(),
        outputs_rx,
        db,
        registry(),
        td.path().to_path_buf(),
        colony_config.clone(),
        None,
        None,
    );
    if let Some(tx) = heartbeat {
        cfg = cfg.with_heartbeat(tx);
    }
    if let Some(tx) = death_ack_signal {
        cfg = cfg.with_death_ack_wait_signal(tx);
    }
    let join = tokio::spawn(colony_task(cfg));
    let runtime = ColonyRuntime {
        inbox_tx: inbox_tx.clone(),
        outputs_tx: outputs_tx.clone(),
        colony_config,
        blob_store: None,
    };
    bootstrap_from_filesystem(td.path(), &registry(), &runtime)
        .await
        .expect("bootstrap");
    Colony {
        inbox_tx,
        outputs_tx,
        join,
    }
}

async fn mutate(c: &Colony, payload: Value) -> MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    c.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    tokio::time::timeout(FAILURE_MARKER, ack_rx)
        .await
        .expect("the mutation answers within the failure marker")
        .expect("an outcome")
}

async fn shutdown(c: Colony) {
    let (ack_tx, ack_rx) = oneshot::channel();
    let _ = c.inbox_tx.send(ColonyMsg::Shutdown { ack: ack_tx }).await;
    let _ = tokio::time::timeout(FAILURE_MARKER, ack_rx).await;
    let _ = tokio::time::timeout(FAILURE_MARKER, c.join).await;
}

/// A long-running cell that stays inside `handle()` until it is released — or
/// forever, when no release was given. Stands in for an `llm` cell waiting on its
/// provider: the peace-stop is only looked at after the handler returns.
struct StuckCell {
    entered: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    release: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
}

impl LongRunningCell for StuckCell {
    type Event = ();
    type Reconfig = ();
    type Io = ();

    fn split_io(&mut self) -> Self::Io {}

    #[allow(clippy::manual_async_fn)]
    fn run_io(
        io: Self::Io,
        events_tx: mpsc::Sender<Self::Event>,
        reconfig_rx: mpsc::Receiver<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send {
        async move {
            let _keep = (io, events_tx, reconfig_rx);
            std::future::pending::<()>().await
        }
    }

    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a mut self,
        _msg: Message,
        _sink: &'a OutputSink,
        _db: &'a mut DbConn,
        _reconfig_tx: &'a mpsc::Sender<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            if let Some(tx) = self.entered.lock().expect("entered lock").take() {
                let _ = tx.send(());
            }
            let release = self.release.lock().expect("release lock").take();
            match release {
                Some(rx) => {
                    let _ = rx.await;
                }
                None => std::future::pending::<()>().await,
            }
        }
    }

    #[allow(clippy::manual_async_fn)]
    fn handle_event<'a>(
        &'a mut self,
        _event: Self::Event,
        _sink: &'a OriginSink,
        _db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async {}
    }
}

/// Spawn one `StuckCell` at `path` with real stop/death-ack wiring, register it,
/// wire it to `/a` so it is active, and wedge it inside `handle()`.
///
/// Returns the release sender (`None` = stuck forever) — held by the caller, since
/// dropping it releases the cell.
async fn stuck_cell(c: &Colony, name: &str, releasable: bool) -> Option<oneshot::Sender<()>> {
    let path = Path::new(&format!("/{name}"));
    let (entered_tx, entered_rx) = oneshot::channel::<()>();
    let (release_tx, release_rx) = if releasable {
        let (tx, rx) = oneshot::channel::<()>();
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };
    let cell = StuckCell {
        entered: Arc::new(Mutex::new(Some(entered_tx))),
        release: Arc::new(Mutex::new(release_rx)),
    };
    let _no_relay = register_cell(c, name, cell, None).await;

    c.inbox_tx
        .send(ColonyMsg::Route {
            sender_path: Path::new("/"),
            msg: MessageBuilder::new(path.clone())
                .body(Body::Inline(json!({"messages":[]})))
                .build(),
        })
        .await
        .unwrap();
    tokio::time::timeout(FAILURE_MARKER, entered_rx)
        .await
        .expect("the cell enters handle() within the failure marker")
        .expect("the cell reports that it entered handle()");
    release_tx
}

/// A cell that is idle — never sent a message — but whose teardown takes
/// `teardown`: its death-ack reaches the colony that long after the cell
/// answered the stop, noted as `acked /<name>` at the moment it is handed on.
/// A healthy cell with a real `cell.db` to close; waited for, it is down well
/// inside any deadline, not waited for, its old task is still up when the
/// colony moves on.
async fn idle_cell_with_teardown(
    c: &Colony,
    name: &str,
    teardown: Duration,
    log: EventLog,
) -> tokio::task::JoinHandle<()> {
    let cell = StuckCell {
        entered: Arc::new(Mutex::new(None)),
        release: Arc::new(Mutex::new(None)),
    };
    register_cell(c, name, cell, Some((teardown, log)))
        .await
        .expect("a teardown relay")
}

/// Spawn `cell` at `/name` with real stop/death-ack wiring, register it and wire
/// it to `/a` so it is active. With `slow_ack`, the cell's death-ack goes through
/// a relay that holds it for the given time and notes `acked /<name>` into the
/// log when it hands it on; the relay's handle is returned.
async fn register_cell(
    c: &Colony,
    name: &str,
    cell: StuckCell,
    slow_ack: Option<(Duration, EventLog)>,
) -> Option<tokio::task::JoinHandle<()>> {
    let path = Path::new(&format!("/{name}"));
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let (death_ack_tx, colony_ack_rx) = oneshot::channel::<()>();
    let (death_ack_rx, relay) = match slow_ack {
        None => (colony_ack_rx, None),
        Some((teardown, log)) => {
            let (relay_tx, relay_rx) = oneshot::channel::<()>();
            let label = format!("acked {}", path.as_str());
            let relay = tokio::spawn(async move {
                // An `Err` is the sender dropped with the task: down either way.
                let _ = colony_ack_rx.await;
                tokio::time::sleep(teardown).await;
                log.lock().expect("event log").push(label);
                let _ = relay_tx.send(());
            });
            (relay_rx, Some(relay))
        }
    };
    let (peace_tx, peace_rx) = oneshot::channel::<()>();
    let (_backstop_tx, backstop_rx) = oneshot::channel::<()>();
    let (tx, rx) = mpsc::channel::<Message>(1000);
    let conn = rusqlite::Connection::open_in_memory().expect("open_in_memory");
    let db = DbConn::wrap(conn, None);
    let outputs_tx = c.outputs_tx.clone();
    let inbox_tx = c.inbox_tx.clone();
    let task_path = path.clone();
    let join = tokio::spawn(async move {
        cell_task_long_running(
            task_path,
            rx,
            outputs_tx,
            64,
            cell,
            db,
            Some(peace_tx),
            Some(inbox_tx),
            Some(stop_rx),
            Some(death_ack_tx),
            None,
            None,
            Default::default(),
        )
        .await;
    });
    let respawn: meclaw_colony::RespawnFn =
        Box::new(|| unreachable!("a stuck cell is never restarted in this test"));
    let (ack_tx, ack_rx) = oneshot::channel();
    c.inbox_tx
        .send(ColonyMsg::Register {
            path: path.clone(),
            sender: tx,
            join,
            peace_rx,
            backstop_rx,
            stop_tx: Some(stop_tx),
            death_ack_rx: Some(death_ack_rx),
            respawn,
            wake: None,
            restart_limit: None,
            cell_id: Uuid::now_v7(),
            cell_type: "test-mock".into(),
            active: true,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.expect("register ack");

    let outcome = mutate(
        c,
        json!({"scope":"/","diff":{"add_edges":[{"from":name,"to":"a"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "wiring {name} -> a must commit, got {outcome:?}"
    );
    relay
}

/// The supervisor on the tight window of GH #439 and `log-only`, so a failure of
/// a test is an assertion and not a dead process. A relay forwards every real
/// beat and keeps the labels, so the test can also read what a wait called
/// itself. Returns the trips and the labels.
fn supervise(
    mut hb_rx: mpsc::Receiver<Beat>,
) -> (
    mpsc::Receiver<WatchdogTrip>,
    mpsc::UnboundedReceiver<String>,
) {
    let (relay_tx, relay_rx) = mpsc::channel::<Beat>(4096);
    let (trip_tx, trip_rx) = mpsc::channel::<WatchdogTrip>(1024);
    let (label_tx, label_rx) = mpsc::unbounded_channel::<String>();
    let (armed_tx, armed_rx) = oneshot::channel::<()>();
    tokio::spawn(meclaw_colony::watchdog::run_watchdog(
        relay_rx,
        trip_tx,
        5,
        Duration::from_millis(10),
        armed_rx,
        WatchdogOnTrip::LogOnly,
        None,
    ));
    tokio::spawn(async move {
        while let Some(b) = hb_rx.recv().await {
            if let Beat::WorkingOn(w) = &b {
                let _ = label_tx.send(w.as_str().to_string());
            }
            if relay_tx.send(b).await.is_err() {
                return;
            }
        }
    });
    let _ = armed_tx.send(());
    (trip_rx, label_rx)
}

/// Every trip that happened INSIDE the mutation (it carries the work item the
/// loop declared) must be non-fatal under the shipped policy. Trips without a
/// work item are the idle loop on this tight window, a different observation.
fn assert_no_fatal_trip_inside_the_mutation(trip_rx: &mut mpsc::Receiver<WatchdogTrip>) {
    let mut inside = Vec::new();
    while let Ok(t) = trip_rx.try_recv() {
        if t.work_item.is_some() {
            inside.push(t);
        }
    }
    let fatal: Vec<String> = inside
        .iter()
        .filter(|t| t.is_fatal(WatchdogOnTrip::Exit))
        .map(|t| t.to_string())
        .collect();
    assert!(
        fatal.is_empty(),
        "a death-ack wait must never be a fatal trip; {} of {} trips inside the mutation \
         were: {fatal:#?}",
        fatal.len(),
        inside.len()
    );
}

/// The wait beat under a label naming the cell it waited for.
fn assert_the_wait_named(label_rx: &mut mpsc::UnboundedReceiver<String>, path: &str) {
    let mut labels = Vec::new();
    while let Ok(l) = label_rx.try_recv() {
        labels.push(l);
    }
    let suffix = format!("death-ack {path}");
    assert!(
        labels
            .iter()
            .any(|l| l.starts_with("mutation ") && l.ends_with(&suffix)),
        "the wait beats under a label naming the cell it waits for ({path}); labels were \
         {labels:?}"
    );
}

/// Half 1: a disconnect that waits on a busy cell keeps beating, is never a fatal
/// trip, and still rejects cleanly at the term-timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_disconnect_waiting_on_a_busy_cell_keeps_the_watchdog_quiet() {
    let _guard = TERM_TIMEOUT_TEST_LOCK.lock().await;
    // Three times the work-item budget below (10 × 5 × 10 ms = 500 ms): a silent
    // wait of this length is `stuck_work_item`, a pulsed one never gets there.
    const TERM_MS: u64 = 1_500;
    set_term_timeout_ms_for_test(TERM_MS);

    let td = TempDir::new().unwrap();
    let (hb_tx, hb_rx) = mpsc::channel::<Beat>(4096);
    let c = boot(&td, Some(hb_tx), None).await;
    let _held = stuck_cell(&c, "busy", false).await;

    let (mut trip_rx, mut label_rx) = supervise(hb_rx);

    let started = std::time::Instant::now();
    let outcome = mutate(
        &c,
        json!({"scope":"/","diff":{"remove_edges":[{"match":{"from":"busy","to":"a"}}]}}),
    )
    .await;
    let took = started.elapsed();
    match &outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(
                error_code, "term_timeout",
                "the busy cell rejects with term_timeout"
            )
        }
        other => panic!("expected Rejected{{term_timeout}}, got {other:?}"),
    }
    assert!(
        took >= Duration::from_millis(TERM_MS),
        "the wait lasts the term-timeout, took {took:?}"
    );

    // Every trip that happened INSIDE the mutation (it carries the work item the
    // loop declared) must be non-fatal under the shipped policy. Trips without a
    // work item are the idle loop on this tight window, a different observation.
    assert_no_fatal_trip_inside_the_mutation(&mut trip_rx);
    assert_the_wait_named(&mut label_rx, "/busy");

    shutdown(c).await;
}

/// Half 2: one deadline per mutation. Three cells answer at half the budget, the
/// fourth only after it; the mutation rejects at ONE term-timeout, never later.
///
/// Before the fix every cell got a fresh budget of its own, in `HashMap` order: a
/// late cell waited on after an early one got its budget counted from the early
/// one's answer, so the mutation committed at 1.5 × term — or, with more cells,
/// took up to N × term in one work item.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_disconnected_cell_shares_one_term_timeout() {
    let _guard = TERM_TIMEOUT_TEST_LOCK.lock().await;
    const TERM_MS: u64 = 1_000;
    set_term_timeout_ms_for_test(TERM_MS);

    let td = TempDir::new().unwrap();
    let (signal_tx, mut signal_rx) = mpsc::channel::<()>(8);
    let c = boot(&td, None, Some(signal_tx)).await;
    let mut early = Vec::new();
    for i in 0..3 {
        early.push(
            stuck_cell(&c, &format!("early{i}"), true)
                .await
                .expect("releasable"),
        );
    }
    let late = stuck_cell(&c, "late", true).await.expect("releasable");

    // Releases are timed from the moment the colony starts waiting — the test
    // hook fires exactly there.
    let releaser = tokio::spawn(async move {
        tokio::time::timeout(FAILURE_MARKER, signal_rx.recv())
            .await
            .expect("the colony reaches the death-ack wait")
            .expect("the signal fires");
        let t0 = tokio::time::Instant::now();
        tokio::time::sleep_until(t0 + Duration::from_millis(TERM_MS / 2)).await;
        for tx in early {
            let _ = tx.send(());
        }
        tokio::time::sleep_until(t0 + Duration::from_millis(TERM_MS * 3 / 2)).await;
        let _ = late.send(());
        t0
    });

    let outcome = mutate(
        &c,
        json!({"scope":"/","diff":{"remove_edges":[
            {"match":{"from":"early0","to":"a"}},
            {"match":{"from":"early1","to":"a"}},
            {"match":{"from":"early2","to":"a"}},
            {"match":{"from":"late","to":"a"}}
        ]}}),
    )
    .await;
    let answered = tokio::time::Instant::now();
    let t0 = tokio::time::timeout(FAILURE_MARKER, releaser)
        .await
        .expect("the releaser ends")
        .expect("no panic in the releaser");
    let took = answered.saturating_duration_since(t0);
    match &outcome {
        MutationOutcome::Rejected { error_code, .. } => assert_eq!(
            error_code, "term_timeout",
            "a cell answering after the shared deadline rejects the mutation"
        ),
        other => panic!(
            "expected Rejected{{term_timeout}} at one term-timeout, got {other:?} after {took:?}"
        ),
    }
    assert!(
        took < Duration::from_millis(TERM_MS * 13 / 10),
        "four disconnected cells cost one term-timeout ({TERM_MS} ms), took {took:?}"
    );

    shutdown(c).await;
}

/// The `config.json` a relocated cell carries: an echo cell like `/a`.
const ECHO_CONFIG: &str = r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/a"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#;

async fn rescan_templates(c: &Colony, templates_root: std::path::PathBuf) {
    let (ack_tx, ack_rx) = oneshot::channel();
    c.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .expect("the template rescan completes");
}

/// Part 3: a move waits for every relocated cell. The first moved cell hangs
/// past the deadline, the second is healthy and needs a short teardown; the
/// second must be down before its directory moves and its new task is spawned.
///
/// Before the fix the one deadline started before the first stop, and the stops
/// went out one by one inside the wait loop: the healthy cell got its stop only
/// after the hung one had used the whole deadline, `timeout_at` on an expired
/// deadline returned at once, and its new task was spawned while the old one —
/// and its `cell.db` — was still up.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_move_waits_for_every_relocated_cell_before_it_respawns_one() {
    let _guard = TERM_TIMEOUT_TEST_LOCK.lock().await;
    const TERM_MS: u64 = 1_000;
    // A tenth of the term: waited for, the healthy cell is down long before the
    // deadline; not waited for, its old task outlives the spawn of the new one
    // by this much, which is the discriminator.
    const TEARDOWN: Duration = Duration::from_millis(100);
    set_term_timeout_ms_for_test(TERM_MS);

    let td = TempDir::new().unwrap();
    let log: EventLog = Arc::default();
    let factory_log = log.clone();
    let registry = move || {
        let mut r = CellFactoryRegistry::new();
        r.insert(
            "echo".into(),
            Arc::new(SpawnLogFactory {
                log: factory_log.clone(),
            }) as Arc<dyn CellFactory>,
        );
        r
    };
    let c = boot_with(&td, None, None, registry).await;
    // The directories a move reads the cells' configuration from, written after
    // the boot so the boot does not spawn cells there of its own.
    for name in ["hung", "healthy"] {
        let dir = td.path().join("main").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), ECHO_CONFIG).unwrap();
    }
    let _held = stuck_cell(&c, "hung", false).await;
    let relay = idle_cell_with_teardown(&c, "healthy", TEARDOWN, log.clone()).await;

    let outcome = mutate(
        &c,
        json!({"scope":"/","diff":{"move_nodes":[
            {"match":{"name":"hung"},"to":"moved_hung"},
            {"match":{"name":"healthy"},"to":"moved_healthy"}
        ]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "a move past a hung cell still commits (the rename does not need the old task), \
         got {outcome:?}"
    );
    tokio::time::timeout(FAILURE_MARKER, relay)
        .await
        .expect("the healthy cell's teardown ends within the failure marker")
        .expect("no panic in the relay");

    let seen = events(&log);
    let spawned = seen.iter().position(|e| e == "spawned /moved_healthy");
    let acked = seen.iter().position(|e| e == "acked /healthy");
    assert!(
        spawned.is_some(),
        "the move spawns the healthy cell at its new address; events were {seen:?}"
    );
    assert!(
        acked.is_some() && acked < spawned,
        "the healthy cell's old task must be down before its new task is spawned — a hung \
         cell ahead of it in the same move must not use up its wait; events were {seen:?}"
    );

    shutdown(c).await;
}

/// Part 4: the rollback after a `term_timeout` reject waits a short grace for
/// the cells the rejected mutation created, not a second full `term_timeout`.
///
/// One mutation creates `/fresh` and disconnects the hung `/busy`. `/busy`
/// rejects it at the deadline; the rollback then stops `/fresh`, which never
/// answers (the worst case). Before the fix the rollback opened a deadline of
/// its own and the mutation took two full term-timeouts; the grace bounds it
/// at one term-timeout plus the grace. The wait beats like every other one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_rollback_after_a_term_timeout_waits_a_grace_not_a_second_term() {
    let _guard = TERM_TIMEOUT_TEST_LOCK.lock().await;
    const TERM_MS: u64 = 1_500;
    // The colony's `ROLLBACK_GRACE` (250 ms) plus a scheduling margin; a second
    // term-timeout would put the mutation at 2 × 1 500 ms, far outside it.
    const GRACE_AND_MARGIN_MS: u64 = 250 + 500;
    set_term_timeout_ms_for_test(TERM_MS);

    let td = TempDir::new().unwrap();
    let registry = || {
        let mut r = echo_registry();
        r.insert(
            "never_acks".into(),
            Arc::new(NeverAcksFactory) as Arc<dyn CellFactory>,
        );
        r
    };
    let (hb_tx, hb_rx) = mpsc::channel::<Beat>(4096);
    let c = boot_with(&td, Some(hb_tx), None, registry).await;
    let tpl = td.path().join("templates/never_acks");
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), r#"{"name":"never_acks"}"#).unwrap();
    std::fs::write(
        tpl.join("config.json"),
        r#"{"cell":{"type":"never_acks"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    rescan_templates(&c, td.path().join("templates")).await;
    let _held = stuck_cell(&c, "busy", false).await;
    let (mut trip_rx, mut label_rx) = supervise(hb_rx);

    let started = std::time::Instant::now();
    let outcome = mutate(
        &c,
        json!({"scope":"/","diff":{
            "add_nodes":[{"name":"fresh","template":"never_acks"}],
            "add_edges":[{"from":"fresh","to":"a"}],
            "remove_edges":[{"match":{"from":"busy","to":"a"}}]
        }}),
    )
    .await;
    let took = started.elapsed();
    match &outcome {
        MutationOutcome::Rejected { error_code, .. } => assert_eq!(
            error_code, "term_timeout",
            "the hung cell rejects the mutation with term_timeout"
        ),
        other => panic!("expected Rejected{{term_timeout}}, got {other:?}"),
    }
    assert!(
        took >= Duration::from_millis(TERM_MS),
        "the disconnect waits the term-timeout, took {took:?}"
    );
    // The fresh cell never acks, so the rollback spends its whole grace. Without
    // this floor a rollback that handed the spent deadline through (no wait,
    // `remove_dir_all` over an open cell.db -- the #276 hazard OR-SN.K.1 rules
    // out) would stay green: the label ticks at the start of the wait either way
    // (review of K, m1).
    assert!(
        took >= Duration::from_millis(TERM_MS + 250),
        "the rollback waits its grace (250 ms) for the fresh cell, took {took:?}"
    );
    assert!(
        took < Duration::from_millis(TERM_MS + GRACE_AND_MARGIN_MS),
        "the rollback waits a grace for the fresh cell, not a second term-timeout \
         ({TERM_MS} ms); the mutation took {took:?}"
    );
    assert_no_fatal_trip_inside_the_mutation(&mut trip_rx);
    assert_the_wait_named(&mut label_rx, "/fresh");

    shutdown(c).await;
}
