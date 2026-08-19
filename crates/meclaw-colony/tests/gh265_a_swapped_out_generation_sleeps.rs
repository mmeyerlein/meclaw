//! GH #265 — a unit that nothing points at any more must SLEEP, and a unit that
//! is wired to the world only through a depth port must stay AWAKE.
//!
//! # What was wrong
//!
//! Connectivity is fully edge-derived. For a hive the spec counts **only
//! external** edges (`docs/meclaw-overview.en.md` § Connectivity and activity,
//! hive sharpening); its own inside is meaningless for its connectivity. But the
//! predicate treated the hive path as lying OUTSIDE its own unit, and the hive
//! boundary *mandates* the wiring `<hive> → <hive>/<cell>` — an edge whose
//! `from` IS the hive path (`docs/cell-types.md` § Die Hive-Grenze). So a unit
//! with nothing left but its own inside counted as connected and stayed awake.
//!
//! After a generation swap that is not an abstraction leak, it is a second
//! agent: the old unit keeps its `timer`, keeps ticking on schedule, and keeps
//! writing into its own stores, answering nobody.
//!
//! # What is pinned here
//!
//! 1. [`a_swapped_out_generation_sleeps_and_stops_ticking`] — the defect, proved
//!    POSITIVELY on both halves: the old generation's cell reads
//!    `active == false` in the registry, and a self-driving poll counter ticked
//!    by its `run_io` sub-task FREEZES. A frozen counter is the direct answer to
//!    "does the clock in a sleeping unit actually stop, or is 'inactive' only
//!    about delivery" — it stops, via the peace-stop path that aborts the I/O
//!    sub-task.
//! 2. [`a_unit_wired_only_through_a_depth_port_stays_awake`] — the more
//!    important half. An activity rule that sleeps too much takes a running
//!    installation off the net. A unit whose own path is named by NO edge, wired
//!    to the world only by a depth-port edge into a descendant, stays awake —
//!    across a recompute that actually reaches it.

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, DbConn, LongRunningCell, MutationOutcome,
    RespawnFn, SpawnedCellKind, bootstrap_from_filesystem, cell_task_long_running,
    set_term_timeout_ms_for_test,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{CellEmission, JsonValue, Message, OriginSink, OutputSink, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

// ─────────────────────────────────────────────────────────────────────────────
// A self-driving "night job" long-running cell — the stand-in for the `timer`
// a channel ships inside its unit (`templates/talky`: `session-keeper/night`).
// Its `run_io` sub-task bumps a shared counter on a short interval. A FROZEN
// counter is positive proof that the clock stopped; a growing one is positive
// proof that it runs.
// ─────────────────────────────────────────────────────────────────────────────

struct NightJob {
    ticks: Arc<AtomicUsize>,
}
struct NightJobIo {
    ticks: Arc<AtomicUsize>,
}

impl LongRunningCell for NightJob {
    type Event = ();
    type Reconfig = ();
    type Io = NightJobIo;

    fn split_io(&mut self) -> Self::Io {
        NightJobIo {
            ticks: self.ticks.clone(),
        }
    }

    #[allow(clippy::manual_async_fn)]
    fn run_io(
        io: Self::Io,
        events_tx: mpsc::Sender<Self::Event>,
        mut reconfig_rx: mpsc::Receiver<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send {
        async move {
            // Keep `events_tx` alive so the handler loop's `events_rx` never
            // closes — otherwise the cell would exit on its own.
            let _keep_events = events_tx;
            let mut tick = tokio::time::interval(Duration::from_millis(15));
            loop {
                tokio::select! {
                    _ = tick.tick() => { io.ticks.fetch_add(1, Ordering::SeqCst); }
                    rc = reconfig_rx.recv() => { if rc.is_none() { break; } }
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

/// Factory for the night job, sharing the tick counter so the test can watch it
/// run and then watch it freeze. One factory instance per counter — the swap pin
/// registers two of them under two type names, so the OLD generation's clock is
/// observed on its own and cannot hide behind the successor's.
struct NightJobFactory {
    ticks: Arc<AtomicUsize>,
}

impl CellFactory for NightJobFactory {
    fn validate_params(&self, _params: &JsonValue) -> Result<(), String> {
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        _params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        _cell_dir: std::path::PathBuf,
        _contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<Duration>,
        _blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
        _mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let (death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
        let ticks = self.ticks.clone();
        // Clones reserved for the initial live spawn (taken before `build`
        // moves the originals).
        let init_ticks = ticks.clone();
        let init_path = path.clone();
        let init_outputs = outputs_tx.clone();
        let init_inbox = colony_inbox_tx.clone();
        let build = move || -> (
            mpsc::Sender<Message>,
            JoinHandle<()>,
            oneshot::Receiver<()>,
            oneshot::Receiver<()>,
        ) {
            let cell = NightJob {
                ticks: ticks.clone(),
            };
            let conn = rusqlite::Connection::open_in_memory().expect("open_in_memory");
            let db = DbConn::wrap(conn, None);
            let (tx, rx) = mpsc::channel::<Message>(1000);
            let (peace_tx, peace_rx) = oneshot::channel();
            let (_backstop_tx, backstop_rx) = oneshot::channel();
            let p = path.clone();
            let o = outputs_tx.clone();
            let cit = colony_inbox_tx.clone();
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
        // Live initial spawn: thread the real stop_rx + death_ack into the task,
        // so a disconnect can peace-stop it (and abort its I/O sub-task).
        let cell = NightJob { ticks: init_ticks };
        let conn = rusqlite::Connection::open_in_memory().expect("open_in_memory");
        let db = DbConn::wrap(conn, None);
        let (tx, rx) = mpsc::channel::<Message>(1000);
        let (peace_tx, peace_rx) = oneshot::channel();
        let (_backstop_tx, backstop_rx) = oneshot::channel();
        let join = tokio::spawn(async move {
            cell_task_long_running(
                init_path,
                rx,
                init_outputs,
                64,
                cell,
                db,
                Some(peace_tx),
                Some(init_inbox),
                Some(stop_rx),
                Some(death_ack_tx),
                None,
                None,
                Default::default(),
            )
            .await;
        });
        Ok(SpawnedCellKind::Active {
            sender: tx,
            join,
            peace_rx,
            stop_tx,
            death_ack_rx,
            backstop_rx,
            respawn: Box::new(build) as RespawnFn,
        })
    }

    /// A subtree cell that arrives via `add_nodes` is registered before the
    /// diff's edges derive it active; without this hook it would stay inert and
    /// the successor generation would have a registry row and no clock. Mirrors
    /// `SubtreeEchoFactory` in `gh256_…`.
    #[allow(clippy::too_many_arguments)]
    fn build_boot_inactive_respawn(
        self: Arc<Self>,
        path: Path,
        _params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        _cell_dir: std::path::PathBuf,
        _contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<Duration>,
        _blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
        _mailbox_capacity: usize,
    ) -> Option<RespawnFn> {
        let ticks = self.ticks.clone();
        Some(Box::new(move || {
            let cell = NightJob {
                ticks: ticks.clone(),
            };
            let conn = rusqlite::Connection::open_in_memory().expect("open_in_memory");
            let db = DbConn::wrap(conn, None);
            let (tx, rx) = mpsc::channel::<Message>(1000);
            let (peace_tx, peace_rx) = oneshot::channel();
            let (_backstop_tx, backstop_rx) = oneshot::channel();
            let p = path.clone();
            let o = outputs_tx.clone();
            let cit = colony_inbox_tx.clone();
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
        }))
    }
}

// ── Harness ──────────────────────────────────────────────────────────────────

const CELL: &str = r#""contract":{"version":"0.1.0","settings":{},"consumes":{}}"#;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// `night_a` and `night_b` are the same cell over two independent counters, so a
/// test can watch one generation's clock without the other's ticks in the way.
/// `echo` is the inert anchor a lane needs at its other end.
fn factory_list(a: Arc<AtomicUsize>, b: Arc<AtomicUsize>) -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![
        (
            "night_a".to_string(),
            Arc::new(NightJobFactory { ticks: a }) as Arc<dyn CellFactory>,
        ),
        (
            "night_b".to_string(),
            Arc::new(NightJobFactory { ticks: b }) as Arc<dyn CellFactory>,
        ),
        (
            "echo".to_string(),
            Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
        ),
    ]
}

fn registry(a: Arc<AtomicUsize>, b: Arc<AtomicUsize>) -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    for (name, f) in factory_list(a, b) {
        r.insert(name, f);
    }
    r
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

async fn rescan_templates(h: &ColonyHandle, templates_root: std::path::PathBuf) {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap();
}

/// The registry's edge-derived `active` flag for one path.
async fn active(h: &ColonyHandle, path: &str) -> Option<bool> {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadRegistryReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 200,
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

/// The persisted edge list as `(from, to)` pairs.
async fn edges(h: &ColonyHandle) -> Vec<(String, String)> {
    let (ack_tx, ack_rx) = oneshot::channel::<meclaw_colony::api_dto::ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: Path::new("/"),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .edges
        .into_iter()
        .map(|e| (e.from, e.to))
        .collect()
}

/// Wait until the counter has advanced past `floor`, or give up. Generous
/// (failure-marker convention) — this only has to prove "it is running".
async fn wait_for_ticks(ticks: &Arc<AtomicUsize>, floor: usize) -> usize {
    for _ in 0..2000 {
        let n = ticks.load(Ordering::SeqCst);
        if n > floor {
            return n;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    ticks.load(Ordering::SeqCst)
}

// ─────────────────────────────────────────────────────────────────────────────
// Pin 1 — the swapped-out generation sleeps, and its clock stops.
// ─────────────────────────────────────────────────────────────────────────────

/// A generation unit: a hive that serves its own child with the two mandated
/// hive-boundary forms (`. → ./night` and back) and NOTHING else. Every edge it
/// owns is internal; the only thing tying it to the colony is the lane its
/// parent points at its path.
fn write_generation_template(root: &std::path::Path, name: &str) {
    let tpl = root.join("templates").join(name);
    write(&tpl, "template.json", &format!(r#"{{"name":"{name}"}}"#));
    write(
        &tpl,
        "config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":".","to":"./night"},
            {"from":"./night","to":"."}
        ]}}}"#,
    );
    write(
        &tpl,
        "night/config.json",
        &format!(r#"{{"cell":{{"type":"night_b"}},"params":{{}},{CELL}}}"#),
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_swapped_out_generation_sleeps_and_stops_ticking() {
    // The disconnect peace-stops an Awake long-running cell; under cargo load a
    // slow death-ack must not spuriously fire `term_timeout`.
    set_term_timeout_ms_for_test(30_000);
    let td = TempDir::new().unwrap();

    // Root hive: the lane `/t1 → /gen2` names the unit — the only thing that
    // ties the generation to the colony.
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./t1","to":"./gen2"}
        ]}}}"#,
    );
    write(
        td.path(),
        "main/t1/config.json",
        &format!(r#"{{"cell":{{"type":"echo"}},"params":{{"emitted_target":"/t1"}},{CELL}}}"#),
    );
    // The generation in place at boot: hive + its own inward/outward wiring.
    write(
        td.path(),
        "main/gen2/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":".","to":"./night"},
            {"from":"./night","to":"."}
        ]}}}"#,
    );
    write(
        td.path(),
        "main/gen2/night/config.json",
        &format!(r#"{{"cell":{{"type":"night_a"}},"params":{{}},{CELL}}}"#),
    );
    // The successor generation, same shape, as a template.
    write_generation_template(td.path(), "gen3_hive");

    // `ticks` is the OLD generation's clock; `succ_ticks` the successor's. Two
    // counters, so the old one's freeze is observed on its own.
    let ticks = Arc::new(AtomicUsize::new(0));
    let succ_ticks = Arc::new(AtomicUsize::new(0));
    let h =
        ColonyHandle::new_with_factories_at(&td, factory_list(ticks.clone(), succ_ticks.clone()));
    rescan_templates(&h, td.path().join("templates")).await;
    bootstrap_from_filesystem(
        td.path(),
        &registry(ticks.clone(), succ_ticks.clone()),
        &h.runtime(),
    )
    .await
    .expect("bootstrap succeeds");

    // Positive baseline: the generation is awake and its clock is running.
    assert_eq!(
        active(&h, "/gen2/night").await,
        Some(true),
        "the wired generation must be awake at boot"
    );
    let before = wait_for_ticks(&ticks, 2).await;
    assert!(
        before > 2,
        "the generation's clock must be running while it is wired (saw {before})"
    );

    // The generation change: instantiate the successor and swing the lane.
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{
            "add_nodes":[{"name":"gen3","template":"gen3_hive"}],
            "swap_nodes":[{"match":{"name":"gen2"},"with":{"name":"gen3"}}]
        }}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the generation change must commit; got {outcome:?}"
    );

    // The swap did what GH #256 promises: the lane moved, the old unit kept its
    // own inside. That inside is now the ONLY thing naming `/gen2`.
    let after = edges(&h).await;
    assert!(
        after.contains(&("/t1".into(), "/gen3".into())),
        "the lane must name the new generation; got {after:?}"
    );
    assert!(
        !after.contains(&("/t1".into(), "/gen2".into())),
        "the old lane must be gone; got {after:?}"
    );
    assert!(
        after.contains(&("/gen2".into(), "/gen2/night".into())),
        "the old generation keeps its own inward wiring (GH #256); got {after:?}"
    );
    assert!(
        !after.iter().any(|(f, t)| (f == "/gen2" || t == "/gen2")
            && f != "/gen2/night"
            && t != "/gen2/night"),
        "nothing outside the old unit may name it any more; got {after:?}"
    );

    // GH #265 — half one, measured positively: the old generation is ASLEEP.
    assert_eq!(
        active(&h, "/gen2/night").await,
        Some(false),
        "a unit whose own inside is all that is left must be inactive"
    );
    // The successor is awake — the rule did not take the live one down with it.
    assert_eq!(
        active(&h, "/gen3/night").await,
        Some(true),
        "the new generation must be awake"
    );

    // GH #265 — half two: the clock in the sleeping unit STOPPED. The mutation
    // only commits after the cell's death-ack, so the I/O sub-task is already
    // aborted; sample, wait several tick intervals, confirm no advance.
    //
    let frozen_at = ticks.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        ticks.load(Ordering::SeqCst),
        frozen_at,
        "the swapped-out generation's clock must stop — a disconnected unit that \
         keeps ticking is a second, invisible agent on a schedule"
    );
    // Counter-observation on its own counter: the SUCCESSOR's clock runs. This
    // rules out "the freeze above is just the whole colony being quiet".
    let succ = wait_for_ticks(&succ_ticks, 2).await;
    assert!(
        succ > 2,
        "the new generation's clock must be running (saw {succ})"
    );

    h.shutdown().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Pin 2 — the counter-case, and the more important one.
// ─────────────────────────────────────────────────────────────────────────────

/// A unit whose own path is named by NO edge at all. It is wired to the world
/// only by a depth-port edge into one of its children (`/anchor → /unit/night`,
/// R12), and it carries the full mandated inward/outward wiring besides. It
/// must stay awake — before and after a recompute that actually reaches it.
///
/// This is the half that decides whether the rule is safe to ship: an activity
/// rule that sleeps too much takes a running installation off the net.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_unit_wired_only_through_a_depth_port_stays_awake() {
    set_term_timeout_ms_for_test(30_000);
    let td = TempDir::new().unwrap();

    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./anchor","to":"./unit/night"}
        ]}}}"#,
    );
    write(
        td.path(),
        "main/anchor/config.json",
        &format!(r#"{{"cell":{{"type":"echo"}},"params":{{"emitted_target":"/anchor"}},{CELL}}}"#),
    );
    write(
        td.path(),
        "main/unit/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":".","to":"./night"},
            {"from":"./night","to":"."}
        ]}}}"#,
    );
    write(
        td.path(),
        "main/unit/night/config.json",
        &format!(r#"{{"cell":{{"type":"night_a"}},"params":{{}},{CELL}}}"#),
    );

    let ticks = Arc::new(AtomicUsize::new(0));
    let unused = Arc::new(AtomicUsize::new(0));
    let h = ColonyHandle::new_with_factories_at(&td, factory_list(ticks.clone(), unused.clone()));
    bootstrap_from_filesystem(td.path(), &registry(ticks.clone(), unused), &h.runtime())
        .await
        .expect("bootstrap succeeds");

    // Nothing names `/unit`. That is the whole point of the case.
    let boot_edges = edges(&h).await;
    assert!(
        !boot_edges
            .iter()
            .any(|(f, t)| f == "/unit" && t != "/unit/night" || t == "/unit" && f != "/unit/night"),
        "the unit's own path must be named only by its own inside; got {boot_edges:?}"
    );

    assert_eq!(
        active(&h, "/unit/night").await,
        Some(true),
        "a unit reached through a depth port is connected — it must boot awake"
    );
    let ticked = wait_for_ticks(&ticks, 2).await;
    assert!(ticked > 2, "the unit must be running (saw {ticked})");

    // Force a recompute that actually REACHES the unit: add an edge out of it
    // and take it away again. Involving `/unit/night` crosses the unit's
    // boundary, so `affected_scope` pulls the whole unit into the recompute —
    // the pass that would put it to sleep if the depth port did not count.
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"unit/night","to":"anchor"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "add_edges must commit; got {outcome:?}"
    );
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{
            "remove_edges":[{"match":{"from":"unit/night","to":"anchor"}}]
        }}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "remove_edges must commit; got {outcome:?}"
    );

    assert_eq!(
        active(&h, "/unit/night").await,
        Some(true),
        "the depth port still wires the unit — the recompute must leave it awake"
    );
    // And its clock kept running through the recompute.
    let after_floor = ticks.load(Ordering::SeqCst);
    let after = wait_for_ticks(&ticks, after_floor).await;
    assert!(
        after > after_floor,
        "the unit's clock must still be running after the recompute (saw {after})"
    );

    h.shutdown().await;
}
