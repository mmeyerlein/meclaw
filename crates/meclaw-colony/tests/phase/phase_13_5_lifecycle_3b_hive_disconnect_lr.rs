//! Phase-13.5 Lifecycle-3b Task 9.2, demo (b): hive-disconnect with a real
//! long-running cell inside the subtree.
//!
//! Topology: a sub-hive `/h` holds a long-running poller `/h/lr` and an echo
//! `/h/c2`, wired internally via `/h`'s own `config.json` graph (`./lr -> ./c2`).
//! The subtree's connectivity hangs on a single parent-level edge `/x -> /h`.
//! Removing that last parent edge disconnects the whole hive.
//!
//! Asserts (end-to-end):
//!   (1) the WHOLE subtree goes inactive despite the internal wiring — `/h/lr`
//!       and `/h/c2` both `active == false`, persisted as `'inactive'`;
//!   (2) the long-running cell's polling STOPS — a poll counter ticked by its
//!       `run_io` sub-task freezes after the disconnect (peace-stop, NOT a
//!       crash). The registry entry STAYS (no `CellDied`, no restart, no
//!       removal) — the cell quiesced via the peace-path.
//!
//! This exercises the hive-cascade (Task 2 `compute_active`) plus the
//! long-running peace-stop (Task 3) together.

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, DbConn, LongRunningCell, MutationOutcome,
    RespawnFn, SpawnedCellKind, bootstrap_from_filesystem, cell_task_long_running,
    set_term_timeout_ms_for_test,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{CellEmission, JsonValue, Message, OriginSink, OutputSink, Path, Uuid};
use meclaw_testing::factories::EchoCellFactory;
use meclaw_testing::{ColonyHandle, deadline, wait};
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

// ───────────────────────────────────────────────────────────────────────────
// A self-driving "poller" long-running cell. Its `run_io` sub-task bumps a
// shared poll counter on a short interval until its channels close (which
// happens when the cell-task ends — i.e. on peace-stop). A frozen counter is
// positive proof that polling stopped.
// ───────────────────────────────────────────────────────────────────────────

struct Poller {
    polls: Arc<PollProbe>,
}
struct PollerIo {
    polls: Arc<PollProbe>,
}

/// What the test observes of the poller: its tick counter and the number of
/// live `run_io` futures.
///
/// GH #1109: the hive disconnect commits once the HANDLER task's death_ack has
/// fired, and `cell_task_long_running` aborts the `run_io` sub-task only after
/// that (handler join first, then `io_join.abort()`). A tick already in flight
/// could therefore land after the commit -- one full release-gate run counted
/// 5 polls against a frozen 4. The test waits for the confirmed stop (no live
/// `run_io` left) instead of trusting a fixed wait to cover that window.
///
/// GH #1209 (M7): `starts` counts every poller the factory built -- the
/// initial spawn and any respawn (the respawn closure runs inside
/// `handle_cell_died`).
///
/// GH #1209 (M2): `peace` reports how each poller task ended, on its way to
/// the colony's watcher: `true` for a peace-stop (the watcher gets `Ok` and
/// never sends `CellDied`), `false` for a death (the sender dropped unsent --
/// the watcher's `Err`, which ends in `CellDied` and a restart). One `true` is
/// the positive end of the disconnect: no restart can follow it. Each report
/// carries the number of the start it belongs to (recheck fix1 A-H3), so the
/// test reads "the FIRST task peace-stopped" from the report itself and not
/// from the order in which two forwarders happen to run.
struct PollProbe {
    ticks: AtomicUsize,
    live_io: watch::Sender<usize>,
    starts: AtomicUsize,
    peace: mpsc::UnboundedSender<(usize, bool)>,
}

impl PollProbe {
    fn new() -> (Self, mpsc::UnboundedReceiver<(usize, bool)>) {
        let (peace, peace_rx) = mpsc::unbounded_channel();
        (
            Self {
                ticks: AtomicUsize::new(0),
                live_io: watch::Sender::new(0),
                starts: AtomicUsize::new(0),
                peace,
            },
            peace_rx,
        )
    }

    /// The cell's peace sender, observed: a forwarder passes the outcome on
    /// to the watcher's `to_watcher` unchanged (sent, or dropped unsent) and
    /// only then reports it, with the number of its start.
    fn observed_peace(
        self: &Arc<Self>,
        start_no: usize,
        to_watcher: oneshot::Sender<()>,
    ) -> oneshot::Sender<()> {
        let (cell_tx, cell_rx) = oneshot::channel::<()>();
        let probe = self.clone();
        tokio::spawn(async move {
            let peaced = cell_rx.await.is_ok();
            if peaced {
                let _ = to_watcher.send(());
            } else {
                drop(to_watcher);
            }
            let _ = probe.peace.send((start_no, peaced));
        });
        cell_tx
    }
}

/// One live `run_io` future; the drop runs on return AND on abort.
struct LiveIo(Arc<PollProbe>);

impl LiveIo {
    fn enter(probe: Arc<PollProbe>) -> Self {
        probe.live_io.send_modify(|n| *n += 1);
        Self(probe)
    }
}

impl Drop for LiveIo {
    fn drop(&mut self) {
        self.0.live_io.send_modify(|n| *n -= 1);
    }
}

impl LongRunningCell for Poller {
    type Event = ();
    type Reconfig = ();
    type Io = PollerIo;

    fn split_io(&mut self) -> Self::Io {
        PollerIo {
            polls: self.polls.clone(),
        }
    }

    #[allow(clippy::manual_async_fn)]
    fn run_io(
        io: Self::Io,
        events_tx: mpsc::Sender<Self::Event>,
        mut reconfig_rx: mpsc::Receiver<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send {
        async move {
            // Keep `events_tx` alive (move it into the future) so the handler
            // loop's `events_rx` never closes — otherwise the cell would exit.
            let _keep_events = events_tx;
            // Counted live until this future returns or is aborted (GH #1109).
            let _live = LiveIo::enter(io.polls.clone());
            let mut tick = tokio::time::interval(std::time::Duration::from_millis(15));
            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        io.polls.ticks.fetch_add(1, Ordering::SeqCst);
                    }
                    rc = reconfig_rx.recv() => {
                        if rc.is_none() { break; }
                    }
                }
            }
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
        async move {}
    }

    #[allow(clippy::manual_async_fn)]
    fn handle_event<'a>(
        &'a mut self,
        _event: Self::Event,
        _sink: &'a OriginSink,
        _db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {}
    }
}

/// Factory for the polling long-running cell, sharing the poll counter so the
/// test can observe (and then prove the freeze of) `run_io`'s ticking.
struct PollerFactory {
    polls: Arc<PollProbe>,
}

impl CellFactory for PollerFactory {
    fn validate_params(&self, _params: &JsonValue) -> Result<(), String> {
        Ok(())
    }

    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        _params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        _cell_dir: std::path::PathBuf,
        _contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<std::time::Duration>,
        _blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        _mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let (death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
        let polls = self.polls.clone();
        let path = path.clone();
        // Clones reserved for the initial live spawn (taken before `build` moves
        // the originals).
        let init_polls = polls.clone();
        let init_path = path.clone();
        let init_outputs = outputs_tx.clone();
        let init_inbox = colony_inbox_tx.clone();
        let build = move || -> (
            mpsc::Sender<Message>,
            JoinHandle<()>,
            oneshot::Receiver<()>,
            oneshot::Receiver<()>,
        ) {
            // A respawn: built inside `handle_cell_died`, in the colony task.
            let start_no = polls.starts.fetch_add(1, Ordering::SeqCst) + 1;
            let cell = Poller {
                polls: polls.clone(),
            };
            let conn = rusqlite::Connection::open_in_memory().expect("open_in_memory");
            let db = DbConn::wrap(conn, None);
            let (tx, rx) = mpsc::channel::<Message>(1000);
            let (peace_tx, peace_rx) = oneshot::channel();
            let peace_tx = polls.observed_peace(start_no, peace_tx);
            let (_backstop_tx, backstop_rx) = oneshot::channel();
            let p = path.clone();
            let o = outputs_tx.clone();
            let cit = colony_inbox_tx.clone();
            // Inert respawn ends (initial spawn carries the live stop wiring).
            let join = tokio::spawn(async move {
                cell_task_long_running(
                    p,
                    rx,
                    o,
                    64,
                    cell,
                    db,
                    Some(peace_tx),
                    Some(cit),
                    None,
                    None,
                    None,
                 None,
                 Default::default(),
                 )
                .await;
            });
            (tx, join, peace_rx, backstop_rx)
        };
        // Live initial spawn: thread the real stop_rx + death_ack into the task.
        let start_no = init_polls.starts.fetch_add(1, Ordering::SeqCst) + 1;
        let (peace_tx, peace_rx) = oneshot::channel();
        let peace_tx = init_polls.observed_peace(start_no, peace_tx);
        let cell = Poller { polls: init_polls };
        let conn = rusqlite::Connection::open_in_memory().expect("open_in_memory");
        let db = DbConn::wrap(conn, None);
        let (tx, rx) = mpsc::channel::<Message>(1000);
        let (_backstop_tx, backstop_rx) = oneshot::channel();
        let p = init_path;
        let o = init_outputs;
        let cit = init_inbox;
        let join = tokio::spawn(async move {
            cell_task_long_running(
                p,
                rx,
                o,
                64,
                cell,
                db,
                Some(peace_tx),
                Some(cit),
                Some(stop_rx),
                Some(death_ack_tx),
                None,
                None,
                Default::default(),
            )
            .await;
        });
        let respawn: RespawnFn = Box::new(build);
        Ok(SpawnedCellKind::Active {
            sender: tx,
            join,
            peace_rx,
            stop_tx,
            death_ack_rx,
            backstop_rx,
            respawn,
        })
    }
}

fn factories(polls: Arc<PollProbe>) -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![
        (
            "echo".to_string(),
            Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
        ),
        (
            "lr".to_string(),
            Arc::new(PollerFactory { polls }) as Arc<dyn CellFactory>,
        ),
    ]
}

fn registry(polls: Arc<PollProbe>) -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert(
        "echo".into(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    );
    r.insert(
        "lr".into(),
        Arc::new(PollerFactory { polls }) as Arc<dyn CellFactory>,
    );
    r
}

/// Root hive + top-level `/x` + sub-hive `/h` with a long-running `/h/lr` and an
/// echo `/h/c2`, internally wired `./lr -> ./c2`. `/h` connectivity hangs on the
/// single parent-level edge `/x -> /h` (added via mutation).
fn write_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main/x")).unwrap();
    std::fs::create_dir_all(td.join("main/h/lr")).unwrap();
    std::fs::create_dir_all(td.join("main/h/c2")).unwrap();
    // A7 (Phase-16 W1a): the gating edge `/x -> /h` is part of the BOOT graph so
    // `/h` is connected from t0 and `/h/lr` boots ACTIVE via the live-spawn path
    // (with real stop wiring), polling immediately. (The test re-sends the same
    // `add_edges x->h` below — a dedup no-op — and then exercises the DISCONNECT
    // cascade via `remove_edges`, which is what this demo actually pins. Under
    // A7 an edge-less island `/h` would boot inactive, and this test's poller
    // factory has an INERT boot-inactive respawn, so a reconnect would not
    // re-spawn the live task — booting connected sidesteps that.)
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./x","to":"./h"}]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/x/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/x"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/h/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./lr","to":"./c2"}
        ]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/h/lr/config.json"),
        r#"{"cell":{"type":"lr"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/h/c2/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/h/c2"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

async fn send_mutation(h: &ColonyHandle, payload: Value) -> MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap()
}

async fn ram_active(h: &ColonyHandle, path: &str) -> Option<bool> {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadRegistryReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 100,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .entries
        .into_iter()
        .find(|e| e.path == path)
        .map(|e| e.active)
}

fn db_registry_status(db_dir: &std::path::Path, path: &str) -> Option<String> {
    let conn = rusqlite::Connection::open_with_flags(
        db_dir.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open colony.db read-only");
    conn.query_row("SELECT status FROM registry WHERE path = ?1", [path], |r| {
        r.get::<_, String>(0)
    })
    .ok()
}

/// Demo (b): disconnecting a hive cascades the whole subtree inactive AND stops
/// the long-running cell's polling (peace-stop, no CellDied).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hive_disconnect_cascades_and_stops_long_running_polling() {
    // Generous death-ack term-timeout (30 s): the hive disconnect peace-stops an
    // Awake long-running cell; under load a slow death-ack must not spuriously
    // fire `term_timeout`.
    set_term_timeout_ms_for_test(30_000);
    let td = TempDir::new().unwrap();
    write_topology(td.path());
    let db_dir = td.path().to_path_buf();

    let (polls, mut peace) = PollProbe::new();
    let polls = Arc::new(polls);
    let h = ColonyHandle::new_with_factories_at(&td, factories(polls.clone()));
    bootstrap_from_filesystem(td.path(), &registry(polls.clone()), &h.runtime())
        .await
        .expect("bootstrap");

    // Wire the parent-level gating edge /x -> /h (short-names at scope `/`).
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"x","to":"h"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "add_edges (hive wiring) must commit, got {outcome:?}"
    );
    assert_eq!(
        ram_active(&h, "/h/lr").await,
        Some(true),
        "/h/lr active while the hive is connected"
    );
    assert_eq!(
        ram_active(&h, "/h/c2").await,
        Some(true),
        "/h/c2 active while the hive is connected"
    );

    // The poller ticks a few times: it was running.
    wait::until(
        "the long-running cell polls while connected (3 ticks)",
        || {
            let n = polls.ticks.load(Ordering::SeqCst);
            async move { n >= 3 }
        },
    )
    .await;

    // remove_edges /x -> /h → /h disconnected → whole subtree inactive cascade.
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"remove_edges":[{"match":{"from":"x","to":"h"}}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "remove_edges must commit (peace-stop death_ack under TERM_TIMEOUT), got {outcome:?}"
    );

    // (1) Whole subtree inactive, entries STAY (no CellDied removal).
    assert_eq!(
        ram_active(&h, "/h/lr").await,
        Some(false),
        "/h/lr must be inactive after the hive disconnects (and still registered)"
    );
    assert_eq!(
        ram_active(&h, "/h/c2").await,
        Some(false),
        "/h/c2 must be inactive after the hive disconnects"
    );

    // (2) Polling froze. The mutation commits after the handler's death_ack
    // fired (post cell.db close), but the run_io sub-task is aborted only after
    // that (GH #1109) -- so first wait for its confirmed stop, then sample the
    // counter, wait several poll intervals, and confirm it did not advance.
    let mut live = polls.live_io.subscribe();
    deadline::hang(
        "run_io stops after the hive disconnect",
        live.wait_for(|n| *n == 0),
    )
    .await
    .map(|_| ())
    .expect("the probe outlives the colony");
    let frozen_at = polls.ticks.load(Ordering::SeqCst);
    // GH #1209 (M2): no time window and no settle -- a positive end event.
    // The poller's task reports how it ended on its way to the watcher; a
    // peace-stop means the watcher never sends `CellDied`, so no restart can
    // follow, and with no live `run_io` no tick can follow either.
    let (start_no, peaced) = deadline::hang("/h/lr's task reports how it ended", peace.recv())
        .await
        .expect("the probe outlives the colony");
    assert_eq!(
        start_no, 1,
        "the first report is the first start's -- a later start means /h/lr was restarted"
    );
    assert!(
        peaced,
        "the hive disconnect must peace-stop /h/lr -- its task died into a restart instead"
    );
    assert_eq!(
        polls.starts.load(Ordering::SeqCst),
        1,
        "the hive disconnect must peace-stop /h/lr, not restart it"
    );
    assert_eq!(
        *polls.live_io.borrow(),
        0,
        "no run_io may come back after the hive disconnect settled"
    );
    assert_eq!(
        polls.ticks.load(Ordering::SeqCst),
        frozen_at,
        "long-running polling must FREEZE after the hive disconnect (peace-stop)"
    );

    h.shutdown().await;

    // Persisted inactive for the subtree cells.
    for n in ["/h/lr", "/h/c2"] {
        assert_eq!(
            db_registry_status(&db_dir, n).as_deref(),
            Some("inactive"),
            "{n} must be persisted inactive"
        );
    }
}
