//! Phase-13.5 Lifecycle-3b Task 7: Reconnect + A2 staging-timeout plumbing.
//!
//! Demo (h) — A2: `add_nodes` of a Long-Running (timer) with `cell.timeout = -1`
//! via mutation spawns with `cell_timeout = -1` (persistent, runs forever, no
//! idle-despawn), NOT the former hardcode (`cell_timeout = 0` /
//! `idle_timeout = DEFAULT_IDLE_TIMEOUT_MS`). The value is read back from a
//! shared recorder the factory writes at `spawn_cell`-time.
//!
//! Demo (e), long-running part — Reconnect eager: a disconnected Long-Running
//! cell (inactive, task stopped) → `add_edges` → recompute flips
//! `active = false → true` → the task is re-spawned IMMEDIATELY (provable via a
//! poll counter that resumes ticking), `cell.db` continuity (counter survives —
//! no reseed), `cell_id` unchanged.
//!
//! Demo (e), stateful part — Reconnect lazy: a disconnected Stateful cell →
//! reconnect → `active = true`, status `NotYetSpawned`, NO task; first message →
//! wake-pre-send spawns the task, `cell.db` resumed (counter survives).

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{
    CellFactory, ColonyMsg, DbConn, MutationOutcome, RespawnFn, SpawnedCellKind,
    cell_task_long_running, set_term_timeout_ms_for_test,
};
use meclaw_core::{Body, CellEmission, JsonValue, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::PersistCellFactory;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::{AtomicI64, Ordering};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

async fn send_mutation(
    h: &ColonyHandle,
    payload: meclaw_core::serde_json::Value,
) -> MutationOutcome {
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

async fn registry_entry(
    h: &ColonyHandle,
    path: &str,
) -> Option<meclaw_colony::api_dto::RegistryEntryDto> {
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
}

// ───────────────────────────────────────────────────────────────────────────
// Long-running "timer" mock: ignores I/O, but records the `cell_timeout` it was
// spawned with into a shared `Arc<AtomicI64>` so the test can prove the A2 value
// flowed through the mutation-spawn path verbatim.
// ───────────────────────────────────────────────────────────────────────────

struct TimerRecorderFactory {
    /// Last `cell_timeout` seen at `spawn_cell` time (-2 = never spawned).
    last_cell_timeout: Arc<AtomicI64>,
}

impl CellFactory for TimerRecorderFactory {
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
        cell_timeout: i64,
        _message_timeout: Option<std::time::Duration>,
        _blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        _mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        // Record the timeout the mutation-spawn path handed us.
        self.last_cell_timeout.store(cell_timeout, Ordering::SeqCst);

        let build = {
            let path = path.clone();
            let outputs_tx = outputs_tx.clone();
            let colony_inbox_tx = colony_inbox_tx.clone();
            move || -> (
            mpsc::Sender<Message>,
            JoinHandle<()>,
            oneshot::Receiver<()>,
            oneshot::Receiver<()>,
        ) {
                // Keep the inject sender alive so `run_io`'s recv-loop never sees
                // a closed channel → the long-running task stays alive (a real
                // persistent timer keeps its I/O sub-task running forever).
                let (cell, inject_tx) = meclaw_testing::mocks::ReceiptMockLongRunningCell::new();
                let conn = rusqlite::Connection::open_in_memory().expect("open_in_memory");
                let db = DbConn::wrap(conn, None);
                let (tx, rx) = mpsc::channel::<Message>(1000);
                let (peace_tx, peace_rx) = oneshot::channel();
                let (_backstop_tx, backstop_rx) = oneshot::channel();
                let p = path.clone();
                let o = outputs_tx.clone();
                let cit = colony_inbox_tx.clone();
                let join = tokio::spawn(async move {
                    let _keep_inject = inject_tx;
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
            }
        };
        let (sender, join, peace_rx, backstop_rx) = build();
        let (stop_tx, _stop_rx) = oneshot::channel::<()>();
        let (death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
        // Fire death_ack immediately on a (future) stop request would need the
        // task; for the A2 demo we only assert the recorded timeout, so the
        // wiring is inert here. `_stop_rx`/`death_ack_tx` are kept alive.
        let _ = &death_ack_tx;
        let respawn: RespawnFn = Box::new(build);
        Ok(SpawnedCellKind::Active {
            sender,
            join,
            peace_rx,
            stop_tx,
            death_ack_rx,
            backstop_rx,
            respawn,
        })
    }
}

/// Demo (h): `add_nodes` of a long-running timer with `cell.timeout = -1` via
/// mutation must spawn with `cell_timeout == -1` (persistent), NOT the hardcode.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutation_add_nodes_long_running_persistent_timeout_minus_one() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );

    // Template `timer` with cell.timeout = -1 (persistent, runs forever).
    let tpl_dir = td.path().join("templates").join("timer");
    std::fs::create_dir_all(&tpl_dir).unwrap();
    std::fs::write(tpl_dir.join("template.json"), r#"{"name":"timer"}"#).unwrap();
    std::fs::write(
        tpl_dir.join("config.json"),
        r#"{"cell":{"type":"timer","timeout":-1},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    let last_cell_timeout = Arc::new(AtomicI64::new(-2));
    let factory: Arc<dyn CellFactory> = Arc::new(TimerRecorderFactory {
        last_cell_timeout: last_cell_timeout.clone(),
    });
    let h = ColonyHandle::new_with_factories_at(&td, vec![("timer".to_string(), factory)]);
    rescan_templates(&h, td.path().join("templates")).await;

    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name":"ticker","template":"timer"}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "add_nodes(timer) must commit; got {outcome:?}"
    );

    assert_eq!(
        last_cell_timeout.load(Ordering::SeqCst),
        -1,
        "mutation-spawn must hand the factory cell_timeout = -1 (A2), NOT the hardcode 0"
    );
    // The cell registered as Awake/active (long-running eager-spawn).
    let e = registry_entry(&h, "/ticker")
        .await
        .expect("/ticker must be registered");
    assert_eq!(e.lifecycle_status, "Awake", "long-running cell is Awake");
    assert!(e.active, "fresh mutation-spawn is active");

    h.shutdown().await;
}

// ───────────────────────────────────────────────────────────────────────────
// Reconnect-eager long-running mock: a long-running cell whose build-closure
// opens an ON-DISK `cell.db`, increments a persisted `spawns` counter on EVERY
// spawn (initial + reconnect re-spawn), and mirrors the latest value into a
// shared `Arc<AtomicI64>`. Proves: (1) re-spawn happened immediately on
// reconnect (recorder bumps without any message), (2) `cell.db` continuity (the
// counter carried over — no reseed). Live stop/death_ack wiring on the INITIAL
// spawn so the disconnect (remove_edges) can peace-stop it.
// ───────────────────────────────────────────────────────────────────────────

/// Read/increment the persisted spawn counter in `cell.db`; returns the NEW value.
fn bump_spawn_counter(cell_dir: &std::path::Path) -> i64 {
    let conn = meclaw_colony::persist::cell_db::open_or_create_cell_db(&cell_dir.join("cell.db"))
        .expect("open cell.db");
    conn.execute(
        "CREATE TABLE IF NOT EXISTS spawns (id INTEGER PRIMARY KEY CHECK (id = 1), n INTEGER NOT NULL)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO spawns (id, n) VALUES (1, 1) ON CONFLICT(id) DO UPDATE SET n = n + 1",
        [],
    )
    .unwrap();
    conn.query_row("SELECT n FROM spawns WHERE id = 1", [], |r| r.get(0))
        .unwrap()
}

struct ReconnectEagerFactory {
    /// Latest persisted spawn count (mirror of `cell.db`).
    spawns: Arc<AtomicI64>,
}

impl CellFactory for ReconnectEagerFactory {
    fn validate_params(&self, _params: &JsonValue) -> Result<(), String> {
        Ok(())
    }

    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        _params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        cell_dir: std::path::PathBuf,
        _contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<std::time::Duration>,
        _blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        _mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let (stop_tx, initial_stop_rx) = oneshot::channel::<()>();
        let (initial_death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
        let spawns = self.spawns.clone();
        let cdir = cell_dir;

        let build = move |stop_rx: Option<oneshot::Receiver<()>>,
                          death_ack: Option<oneshot::Sender<()>>|
              -> (
            mpsc::Sender<Message>,
            JoinHandle<()>,
            oneshot::Receiver<()>,
            oneshot::Receiver<()>,
        ) {
            // cell.db continuity proof: bump the on-disk counter per (re-)spawn.
            let n = bump_spawn_counter(&cdir);
            spawns.store(n, Ordering::SeqCst);

            let (cell, inject_tx) = meclaw_testing::mocks::ReceiptMockLongRunningCell::new();
            // The cell's working connection (separate handle on the same file).
            let conn =
                meclaw_colony::persist::cell_db::open_or_create_cell_db(&cdir.join("cell.db"))
                    .expect("open cell.db");
            let db = DbConn::wrap(conn, None);
            let (tx, rx) = mpsc::channel::<Message>(1000);
            let (peace_tx, peace_rx) = oneshot::channel();
            let (_backstop_tx, backstop_rx) = oneshot::channel();
            let p = path.clone();
            let o = outputs_tx.clone();
            let cit = colony_inbox_tx.clone();
            let join = tokio::spawn(async move {
                let _keep_inject = inject_tx;
                cell_task_long_running(
                    p,
                    rx,
                    o,
                    64,
                    cell,
                    db,
                    Some(peace_tx),
                    Some(cit),
                    stop_rx,
                    death_ack,
                    None,
                    None,
                    Default::default(),
                )
                .await;
            });
            (tx, join, peace_rx, backstop_rx)
        };

        // Initial spawn → live stop/death_ack; reconnect re-spawn → inert ends
        // (documented limitation: RespawnFn 3-tuple carries no stop wiring).
        let (sender, join, peace_rx, backstop_rx) =
            build(Some(initial_stop_rx), Some(initial_death_ack_tx));
        let respawn: RespawnFn = Box::new(move || build(None, None));
        Ok(SpawnedCellKind::Active {
            sender,
            join,
            peace_rx,
            stop_tx,
            death_ack_rx,
            backstop_rx,
            respawn,
        })
    }
}

/// Demo (e), long-running part: disconnect a long-running cell, then `add_edges`
/// reconnect → the task is re-spawned IMMEDIATELY (spawn counter bumps without a
/// message), `cell.db` counter survives (continuity, no reseed), `cell_id` stays.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reconnect_long_running_eager_respawns_immediately_db_continuity() {
    // Generous death-ack term-timeout (30 s): the disconnect peace-stops an Awake
    // cell; under load a slow death-ack must not spuriously fire `term_timeout`.
    set_term_timeout_ms_for_test(30_000);
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    // A second echo-less hive peer `/peer` to anchor edges (a cell endpoint).
    write(
        td.path(),
        "main/peer/config.json",
        r#"{"cell":{"type":"timer","timeout":-1},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let spawns = Arc::new(AtomicI64::new(0));
    let lr_cell_dir = td.path().join("main/lr");
    std::fs::create_dir_all(&lr_cell_dir).unwrap();
    let factory: Arc<dyn CellFactory> = Arc::new(ReconnectEagerFactory {
        spawns: spawns.clone(),
    });
    // `/peer` uses the inert TimerRecorder so it stays a live cell endpoint.
    let peer_factory: Arc<dyn CellFactory> = Arc::new(TimerRecorderFactory {
        last_cell_timeout: Arc::new(AtomicI64::new(-2)),
    });
    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![
            ("timer".to_string(), peer_factory.clone()),
            ("lr".to_string(), factory.clone()),
        ],
    );

    // Spawn /peer (live cell endpoint) and /lr via their factories + register.
    let peer_spawned = peer_factory
        .spawn_cell(
            Path::new("/peer"),
            meclaw_core::serde_json::json!({}),
            h.runtime().outputs_tx,
            td.path().join("main/peer"),
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            -1,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned(Path::new("/peer"), peer_spawned).await;
    let lr_spawned = factory
        .spawn_cell(
            Path::new("/lr"),
            meclaw_core::serde_json::json!({}),
            h.runtime().outputs_tx,
            lr_cell_dir.clone(),
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            -1,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned(Path::new("/lr"), lr_spawned).await;
    // cell_id assigned at registration — capture it for the continuity assert.
    let lr_cell_id = registry_entry(&h, "/lr").await.expect("/lr").cell_id;

    // Connect /lr -> /peer → /lr active.
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope":"/","diff":{"add_edges":[{"from":"lr","to":"peer"}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "add_edges lr->peer must commit, got {outcome:?}"
    );
    assert!(registry_entry(&h, "/lr").await.expect("/lr").active);
    assert_eq!(spawns.load(Ordering::SeqCst), 1, "initial spawn count == 1");

    // Disconnect: remove the only edge → recompute deactivates /lr (peace-stop).
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope":"/","diff":{"remove_edges":[{"match":{"from":"lr","to":"peer"}}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "remove_edges must commit (death-ack fires), got {outcome:?}"
    );
    assert!(
        !registry_entry(&h, "/lr").await.expect("/lr stays").active,
        "/lr inactive after disconnect"
    );

    // Reconnect: re-add the edge → recompute flips false→true → eager re-spawn.
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope":"/","diff":{"add_edges":[{"from":"lr","to":"peer"}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "reconnect add_edges must commit, got {outcome:?}"
    );

    let e = registry_entry(&h, "/lr")
        .await
        .expect("/lr still registered");
    assert!(e.active, "/lr active again after reconnect");
    assert_eq!(
        e.lifecycle_status, "Awake",
        "long-running eager reconnect → Awake (task re-spawned)"
    );
    assert_eq!(
        e.cell_id, lr_cell_id,
        "cell_id must be unchanged across disconnect+reconnect"
    );
    // Eager re-spawn happened IMMEDIATELY (no message sent) → spawn count == 2.
    assert_eq!(
        spawns.load(Ordering::SeqCst),
        2,
        "reconnect must re-spawn the long-running task immediately"
    );

    h.shutdown().await;

    // cell.db continuity: the persisted counter is 2 (resumed, no reseed).
    let final_n: i64 = {
        let conn = rusqlite::Connection::open_with_flags(
            lr_cell_dir.join("cell.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        conn.query_row("SELECT n FROM spawns WHERE id = 1", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(
        final_n, 2,
        "cell.db spawn counter must survive disconnect+reconnect (no reseed)"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Step-10 interim guard: a second disconnect of a reconnect-eager long-running
// cell whose re-spawn left `stop_tx == None` (the RespawnFn 3-tuple carries no
// stop wiring) cannot be peace-stopped. Before the guard this silently flipped
// `active = false` while the task kept running (a "Task ⇔ aktiv" zombie). The
// guard turns this into an ATOMIC reject (`stop_wiring_unavailable`): the task
// keeps running, the remove_edges is rolled back, NO CellDied / registry.remove.
//
// (The crash-restart path reaches the SAME registry state — Awake + stop_tx
// None — so the guard covers it too; reconnect-eager is chosen here because it
// is deterministic without crash timing.)
// ───────────────────────────────────────────────────────────────────────────

/// Like `ReconnectEagerFactory` but also exposes the live cell's `event_calls`
/// counter + the latest event-inject sender so the test can prove the task is
/// STILL running after the rejected second disconnect (an injected event bumps
/// `event_calls`).
struct GuardProbeFactory {
    spawns: Arc<AtomicI64>,
    /// Mirror of the latest spawned cell's `event_calls` (handle_event count).
    event_calls: Arc<AtomicU32>,
    /// Latest spawn's event-inject sender (kept alive; lets the test poke I/O).
    inject_slot: Arc<std::sync::Mutex<Option<mpsc::Sender<meclaw_testing::mocks::MockEvent>>>>,
}

impl CellFactory for GuardProbeFactory {
    fn validate_params(&self, _params: &JsonValue) -> Result<(), String> {
        Ok(())
    }

    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        _params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        cell_dir: std::path::PathBuf,
        _contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<std::time::Duration>,
        _blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        _mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let (stop_tx, initial_stop_rx) = oneshot::channel::<()>();
        let (initial_death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
        let spawns = self.spawns.clone();
        let event_calls = self.event_calls.clone();
        let inject_slot = self.inject_slot.clone();
        let cdir = cell_dir;

        let build = move |stop_rx: Option<oneshot::Receiver<()>>,
                          death_ack: Option<oneshot::Sender<()>>|
              -> (
            mpsc::Sender<Message>,
            JoinHandle<()>,
            oneshot::Receiver<()>,
            oneshot::Receiver<()>,
        ) {
            let n = bump_spawn_counter(&cdir);
            spawns.store(n, Ordering::SeqCst);

            let (cell, inject_tx) = meclaw_testing::mocks::ReceiptMockLongRunningCell::new();
            // Mirror this spawn's event counter + inject sender so the test
            // can prove THIS task is alive after the rejected mutation.
            event_calls.store(0, Ordering::SeqCst);
            let cell_events = cell.event_calls.clone();
            *inject_slot.lock().unwrap() = Some(inject_tx.clone());
            let event_mirror = event_calls.clone();

            let conn =
                meclaw_colony::persist::cell_db::open_or_create_cell_db(&cdir.join("cell.db"))
                    .expect("open cell.db");
            let db = DbConn::wrap(conn, None);
            let (tx, rx) = mpsc::channel::<Message>(1000);
            let (peace_tx, peace_rx) = oneshot::channel();
            let (_backstop_tx, backstop_rx) = oneshot::channel();
            let p = path.clone();
            let o = outputs_tx.clone();
            let cit = colony_inbox_tx.clone();
            let join = tokio::spawn(async move {
                let _keep_inject = inject_tx;
                // Mirror handle_event counts into the shared AtomicU32 the
                // test polls (the cell bumps its own AtomicUsize per event).
                let mirror = tokio::spawn(async move {
                    loop {
                        let v = cell_events.load(Ordering::SeqCst) as u32;
                        event_mirror.store(v, Ordering::SeqCst);
                        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    }
                });
                cell_task_long_running(
                    p,
                    rx,
                    o,
                    64,
                    cell,
                    db,
                    Some(peace_tx),
                    Some(cit),
                    stop_rx,
                    death_ack,
                    None,
                    None,
                    Default::default(),
                )
                .await;
                mirror.abort();
            });
            (tx, join, peace_rx, backstop_rx)
        };

        let (sender, join, peace_rx, backstop_rx) =
            build(Some(initial_stop_rx), Some(initial_death_ack_tx));
        let respawn: RespawnFn = Box::new(move || build(None, None));
        Ok(SpawnedCellKind::Active {
            sender,
            join,
            peace_rx,
            stop_tx,
            death_ack_rx,
            backstop_rx,
            respawn,
        })
    }
}

/// Step-10: a disconnect that cannot be peace-stopped (re-spawned long-running
/// cell, Awake, `stop_tx == None`) must be ATOMICALLY REJECTED with
/// `stop_wiring_unavailable` — never a silent "active=false while task runs"
/// zombie. RED on HEAD before the guard (the mutation commits + silently flips).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn second_disconnect_without_stop_wiring_is_rejected_not_zombie() {
    // Generous death-ack term-timeout (30 s): the FIRST disconnect peace-stops an
    // Awake cell; under load a slow death-ack must not spuriously fire
    // `term_timeout` (the SECOND disconnect rejects via the stop-wiring guard,
    // not via the timeout, so this override does not mask the asserted reject).
    set_term_timeout_ms_for_test(30_000);
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/peer/config.json",
        r#"{"cell":{"type":"timer","timeout":-1},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let spawns = Arc::new(AtomicI64::new(0));
    let event_calls = Arc::new(AtomicU32::new(0));
    let inject_slot = Arc::new(std::sync::Mutex::new(None));
    let lr_cell_dir = td.path().join("main/lr");
    std::fs::create_dir_all(&lr_cell_dir).unwrap();
    let factory: Arc<dyn CellFactory> = Arc::new(GuardProbeFactory {
        spawns: spawns.clone(),
        event_calls: event_calls.clone(),
        inject_slot: inject_slot.clone(),
    });
    let peer_factory: Arc<dyn CellFactory> = Arc::new(TimerRecorderFactory {
        last_cell_timeout: Arc::new(AtomicI64::new(-2)),
    });
    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![
            ("timer".to_string(), peer_factory.clone()),
            ("lr".to_string(), factory.clone()),
        ],
    );

    let peer_spawned = peer_factory
        .spawn_cell(
            Path::new("/peer"),
            meclaw_core::serde_json::json!({}),
            h.runtime().outputs_tx,
            td.path().join("main/peer"),
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            -1,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned(Path::new("/peer"), peer_spawned).await;
    let lr_spawned = factory
        .spawn_cell(
            Path::new("/lr"),
            meclaw_core::serde_json::json!({}),
            h.runtime().outputs_tx,
            lr_cell_dir.clone(),
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            -1,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned(Path::new("/lr"), lr_spawned).await;
    let lr_cell_id = registry_entry(&h, "/lr").await.expect("/lr").cell_id;

    // Connect → /lr active (initial spawn has live stop wiring).
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope":"/","diff":{"add_edges":[{"from":"lr","to":"peer"}]}
        }),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));

    // First disconnect: peace-stops the cell (live stop_tx) → status
    // NotYetSpawned, stop_tx consumed (None), active=false. Commits cleanly.
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope":"/","diff":{"remove_edges":[{"match":{"from":"lr","to":"peer"}}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "first disconnect (live stop) commits, got {outcome:?}"
    );

    // Reconnect eager: re-spawn → status Awake, but stop_tx STILL None (RespawnFn
    // 3-tuple has no stop wiring). active=true.
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope":"/","diff":{"add_edges":[{"from":"lr","to":"peer"}]}
        }),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));
    assert_eq!(spawns.load(Ordering::SeqCst), 2, "reconnect re-spawned");
    let e = registry_entry(&h, "/lr").await.expect("/lr");
    assert!(e.active && e.lifecycle_status == "Awake");

    // Prove the re-spawned task is alive BEFORE the second disconnect: inject an
    // event, expect event_calls to bump.
    let inject = inject_slot.lock().unwrap().clone().expect("inject sender");
    inject
        .send(meclaw_testing::mocks::MockEvent("ping1".into()))
        .await
        .unwrap();
    meclaw_testing::wait::wait_for_spawn_count(&event_calls, 1, std::time::Duration::from_secs(30))
        .await;

    // ── SECOND disconnect: Awake + stop_tx None → cannot peace-stop. ──────────
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope":"/","diff":{"remove_edges":[{"match":{"from":"lr","to":"peer"}}]}
        }),
    )
    .await;

    // (a) ATOMIC reject with the interim-guard error_code.
    match &outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(
                error_code, "stop_wiring_unavailable",
                "second disconnect must reject with stop_wiring_unavailable"
            );
        }
        other => panic!("expected Rejected(stop_wiring_unavailable), got {other:?}"),
    }

    // (b) The task is STILL running — a fresh injected event keeps bumping.
    inject
        .send(meclaw_testing::mocks::MockEvent("ping2".into()))
        .await
        .unwrap();
    meclaw_testing::wait::wait_for_spawn_count(&event_calls, 2, std::time::Duration::from_secs(30))
        .await;

    // (c) Edges restored (remove_edges rolled back) → /lr still active.
    // (d) No flip + (e) cell_id unchanged + entry still present (no CellDied).
    let e = registry_entry(&h, "/lr")
        .await
        .expect("/lr entry still present (NO registry.remove)");
    assert!(
        e.active,
        "active stays true (no flip — rejected atomically)"
    );
    assert_eq!(e.lifecycle_status, "Awake", "still Awake (task untouched)");
    assert_eq!(e.cell_id, lr_cell_id, "cell_id unchanged (no CellDied)");

    // (f) Persisted status still 'active' (no durable inactive write — fresh probe).
    let db_dir = td.path().to_path_buf();
    let conn = rusqlite::Connection::open_with_flags(
        db_dir.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let status: String = conn
        .query_row("SELECT status FROM registry WHERE path = '/lr'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        status, "active",
        "persisted status untouched (atomic reject discarded the write_buffer)"
    );

    h.shutdown().await;
}

// ───────────────────────────────────────────────────────────────────────────
// Demo (e), stateful part: reconnect a disconnected Stateful cell → it flips
// active=true but STAYS NotYetSpawned (NO task), and only the first message
// wakes it (wake-pre-send), resuming its `cell.db` counter (no reset).
// ───────────────────────────────────────────────────────────────────────────

/// Read the persisted `counter` slot from a stateful cell's `cell.db`.
fn db_counter(cell_dir: &std::path::Path) -> Option<i64> {
    let conn = rusqlite::Connection::open_with_flags(
        cell_dir.join("cell.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .ok()?;
    conn.query_row(
        "SELECT value FROM system WHERE slot_path='counter'",
        [],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .and_then(|v| v.parse::<i64>().ok())
}

/// Wake the stateful cell with one (terminal, non-cascading) message + wait for
/// the spawn-count to reach `expected` (proves the wake-pre-send spawned it).
async fn poke_and_wait(h: &ColonyHandle, path: &str, count: &Arc<AtomicU32>, expected: u32) {
    h.send(
        MessageBuilder::new(Path::new(path))
            .body(Body::Inline(
                meclaw_core::serde_json::json!({"messages":[]}),
            ))
            .build(),
    )
    .await;
    meclaw_testing::wait::wait_for_spawn_count(count, expected, std::time::Duration::from_secs(30))
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reconnect_stateful_lazy_stays_notyetspawned_until_first_message() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    // FS node /sf (stateful) + /peer (live cell endpoint for the gating edge).
    write(
        td.path(),
        "main/sf/config.json",
        r#"{"cell":{"type":"persist_mock","idle_timeout_ms":60000},"params":{"terminal":true},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        td.path(),
        "main/peer/config.json",
        r#"{"cell":{"type":"timer","timeout":-1},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let spawn_count = Arc::new(AtomicU32::new(0));
    let sf_factory: Arc<dyn CellFactory> = Arc::new(PersistCellFactory {
        spawn_count: spawn_count.clone(),
    });
    let peer_factory: Arc<dyn CellFactory> = Arc::new(TimerRecorderFactory {
        last_cell_timeout: Arc::new(AtomicI64::new(-2)),
    });
    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![
            ("persist_mock".to_string(), sf_factory.clone()),
            ("timer".to_string(), peer_factory.clone()),
        ],
    );

    // Pre-seed the stateful cell's `cell.db` counter = 5 (state from a prior
    // run; after a reboot the lifecycle status resets to NotYetSpawned while the
    // persisted counter survives — exactly the "disconnected-stateful with
    // history" precondition). The wake overlay (`overlay_from_db`) will load it.
    let sf_dir = td.path().join("main/sf");
    std::fs::create_dir_all(&sf_dir).unwrap();
    {
        let conn = meclaw_colony::persist::cell_db::open_or_create_cell_db(&sf_dir.join("cell.db"))
            .unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO system (slot_path, value, updated_at) VALUES ('counter','5',0)",
            [],
        )
        .unwrap();
    }

    // Register /sf (Dormant) + /peer (Active) via their factories.
    let sf_spawned = sf_factory
        .spawn_cell(
            Path::new("/sf"),
            meclaw_core::serde_json::json!({"terminal":true}),
            h.runtime().outputs_tx,
            sf_dir.clone(),
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            Some(std::time::Duration::from_millis(60000)),
            0,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned(Path::new("/sf"), sf_spawned).await;
    let peer_spawned = peer_factory
        .spawn_cell(
            Path::new("/peer"),
            meclaw_core::serde_json::json!({}),
            h.runtime().outputs_tx,
            td.path().join("main/peer"),
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            -1,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned(Path::new("/peer"), peer_spawned).await;

    // /sf starts NotYetSpawned (Dormant registration), never woken yet.
    let e = registry_entry(&h, "/sf").await.expect("/sf");
    assert_eq!(e.lifecycle_status, "NotYetSpawned");
    let sf_cell_id = e.cell_id;

    // Connect /sf -> /peer → /sf active (still NotYetSpawned, no task yet).
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope":"/","diff":{"add_edges":[{"from":"sf","to":"peer"}]}
        }),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));
    assert!(registry_entry(&h, "/sf").await.expect("/sf").active);

    // Disconnect a never-woken stateful cell: remove the edge → recompute flips
    // active=false. A NotYetSpawned cell has no running task → no peace-stop, no
    // handle_stopped; it just stays NotYetSpawned + active=false (Task-4.2).
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope":"/","diff":{"remove_edges":[{"match":{"from":"sf","to":"peer"}}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "disconnect must commit, got {outcome:?}"
    );
    let e = registry_entry(&h, "/sf").await.expect("/sf stays");
    assert!(!e.active, "/sf inactive after disconnect");
    assert_eq!(
        e.lifecycle_status, "NotYetSpawned",
        "disconnected never-woken stateful stays NotYetSpawned"
    );

    let spawns_after_disconnect = spawn_count.load(Ordering::Relaxed);
    assert_eq!(
        spawns_after_disconnect, 0,
        "no task spawned before reconnect"
    );

    // Reconnect: re-add the edge → false→true. Stateful = lazy → active=true,
    // status NotYetSpawned, NO new task spawned.
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope":"/","diff":{"add_edges":[{"from":"sf","to":"peer"}]}
        }),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));
    let e = registry_entry(&h, "/sf").await.expect("/sf");
    assert!(e.active, "/sf active again after reconnect");
    assert_eq!(
        e.lifecycle_status, "NotYetSpawned",
        "stateful reconnect = lazy → stays NotYetSpawned (no task)"
    );
    assert_eq!(
        e.cell_id, sf_cell_id,
        "cell_id unchanged across stateful disconnect+reconnect"
    );
    assert_eq!(
        spawn_count.load(Ordering::Relaxed),
        spawns_after_disconnect,
        "reconnect must NOT spawn a stateful task (lazy, wake-on-message)"
    );

    // First message after reconnect → wake-pre-send spawns the task (spawn_count
    // 0→1); overlay loads counter=5 from cell.db, handle() increments → 6.
    poke_and_wait(&h, "/sf", &spawn_count, 1).await;
    let e = registry_entry(&h, "/sf").await.expect("/sf");
    assert_eq!(
        e.lifecycle_status, "Awake",
        "first message after reconnect wakes the stateful cell"
    );

    // Wait for the handle() snapshot to land (WAL-readable from a fresh conn):
    // counter must resume from the persisted 5 → 6 (no reset, cell.db continuity).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while db_counter(&sf_dir) != Some(6) {
        assert!(
            std::time::Instant::now() < deadline,
            "cell.db counter did not reach 6 within 30s (got {:?})",
            db_counter(&sf_dir)
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    h.shutdown().await;

    assert_eq!(
        db_counter(&sf_dir),
        Some(6),
        "cell.db counter must resume from the persisted 5 (→6 after reconnect-wake), no reset"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Phase-13.5 Slice 4 T6 — Demo (e): the reconnect-eager re-spawn closure now
// RE-NOTIFIES the colony with the fresh `(stop_tx, death_ack_rx)` pair
// (`ColonyMsg::StopWiringRestored` via `renotify_stop_wiring`). This proves the
// OPPOSITE of `second_disconnect_without_stop_wiring_is_rejected_not_zombie`:
// after restoration a SECOND disconnect of the same reconnected cell COMMITS
// (no `stop_wiring_unavailable` reject) and the cell peace-stops cleanly.
// ───────────────────────────────────────────────────────────────────────────

/// Return shape of `RenotifyProbeFactory`'s internal `build` closure:
/// `(mailbox_sender, cell_join, peace_rx, stop_tx, death_ack_rx)`.
type BuiltCell = (
    mpsc::Sender<Message>,
    JoinHandle<()>,
    oneshot::Receiver<()>,
    oneshot::Sender<()>,
    oneshot::Receiver<()>,
    oneshot::Receiver<()>,
);

/// Like `GuardProbeFactory`, but its `RespawnFn` builds a LIVE stop pair, wires
/// it into the cell task, and re-notifies the colony via `renotify_stop_wiring`
/// (the production T6 behaviour). After a reconnect-eager re-spawn the cell
/// therefore regains a live `stop_tx` and a later disconnect can peace-stop it.
struct RenotifyProbeFactory {
    spawns: Arc<AtomicI64>,
    /// If true, the INITIAL spawn's cell panics on its first `handle` call, so a
    /// real `CellDied{was_panic:true}` drives `handle_cell_died` → respawn (the
    /// crash-restart path 2). The respawn's cell never panics.
    panic_first: bool,
}

impl CellFactory for RenotifyProbeFactory {
    fn validate_params(&self, _params: &JsonValue) -> Result<(), String> {
        Ok(())
    }

    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        _params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        cell_dir: std::path::PathBuf,
        _contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<std::time::Duration>,
        _blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        _mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let spawns = self.spawns.clone();
        let panic_first = self.panic_first;
        let cdir = cell_dir;

        // `renotify` toggles the T6 re-notify: the initial spawn returns its live
        // pair through `SpawnedCellKind::Active` (no re-notify needed), every
        // respawn instead re-notifies the colony with the fresh pair.
        let build = move |renotify: bool| -> BuiltCell {
            let n = bump_spawn_counter(&cdir);
            spawns.store(n, Ordering::SeqCst);

            let (stop_tx, stop_rx) = oneshot::channel::<()>();
            let (death_ack_tx, death_ack_rx) = oneshot::channel::<()>();

            let (mut cell, inject_tx) = meclaw_testing::mocks::ReceiptMockLongRunningCell::new();
            // Crash-restart path: only the INITIAL spawn panics (on first handle);
            // the respawn (`renotify == true`) runs a healthy cell.
            if panic_first && !renotify {
                cell = cell.with_panic_in_handle_after(1);
            }
            let conn =
                meclaw_colony::persist::cell_db::open_or_create_cell_db(&cdir.join("cell.db"))
                    .expect("open cell.db");
            let db = DbConn::wrap(conn, None);
            let (tx, rx) = mpsc::channel::<Message>(1000);
            let (peace_tx, peace_rx) = oneshot::channel();
            let (_backstop_tx, backstop_rx) = oneshot::channel();
            let p = path.clone();
            let o = outputs_tx.clone();
            let cit = colony_inbox_tx.clone();
            let join = tokio::spawn(async move {
                let _keep_inject = inject_tx;
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
            if renotify {
                // T6: a re-spawn (crash-restart / reconnect-eager) re-notifies the
                // colony with the freshly built stop pair instead of returning it.
                meclaw_colony::renotify_stop_wiring(
                    &colony_inbox_tx,
                    path.clone(),
                    stop_tx,
                    death_ack_rx,
                );
                // Placeholder ends so the build() tuple shape stays uniform; the
                // live pair already went to the colony via renotify above.
                let (ph_stop_tx, _ph_stop_rx) = oneshot::channel::<()>();
                let (_ph_death_ack_tx, ph_death_ack_rx) = oneshot::channel::<()>();
                (tx, join, peace_rx, ph_stop_tx, ph_death_ack_rx, backstop_rx)
            } else {
                (tx, join, peace_rx, stop_tx, death_ack_rx, backstop_rx)
            }
        };

        let (sender, join, peace_rx, stop_tx, death_ack_rx, backstop_rx) = build(false);
        let respawn: RespawnFn = Box::new(move || {
            let (s, j, prx, _stop_tx, _death_ack_rx, brx) = build(true);
            (s, j, prx, brx)
        });
        Ok(SpawnedCellKind::Active {
            sender,
            join,
            peace_rx,
            stop_tx,
            death_ack_rx,
            backstop_rx,
            respawn,
        })
    }
}

/// T6 Demo (e): after a reconnect-eager re-spawn re-notifies the fresh stop pair,
/// a SECOND disconnect of the same cell COMMITS (no `stop_wiring_unavailable`)
/// and the cell peace-stops cleanly — `cell_id` gone from registry only if a
/// CellDied followed; here the peace-exit is silent, so the entry is removed via
/// the disconnect's inactive-flip while the task stops. We assert: Committed,
/// the death-ack fired (task torn down), and NO reject code.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn second_disconnect_after_renotify_commits_and_peace_stops() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/peer/config.json",
        r#"{"cell":{"type":"lr","timeout":-1},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let spawns = Arc::new(AtomicI64::new(0));
    let lr_cell_dir = td.path().join("main/lr");
    std::fs::create_dir_all(&lr_cell_dir).unwrap();
    let factory: Arc<dyn CellFactory> = Arc::new(RenotifyProbeFactory {
        spawns: spawns.clone(),
        panic_first: false,
    });
    // The disconnect/reconnect of the `lr→peer` edge deactivates BOTH endpoints,
    // so `/peer` must ALSO re-notify its stop wiring after a reconnect-eager
    // re-spawn — otherwise the second disconnect would reject on `/peer`. Give it
    // its own RenotifyProbeFactory (same T6 behaviour, separate cell_dir).
    let peer_spawns = Arc::new(AtomicI64::new(0));
    let peer_factory: Arc<dyn CellFactory> = Arc::new(RenotifyProbeFactory {
        spawns: peer_spawns.clone(),
        panic_first: false,
    });
    let peer_cell_dir = td.path().join("main/peer");
    std::fs::create_dir_all(&peer_cell_dir).unwrap();
    let h = ColonyHandle::new_with_factories_at(&td, vec![("lr".to_string(), factory.clone())]);

    let peer_spawned = peer_factory
        .clone()
        .spawn_cell(
            Path::new("/peer"),
            meclaw_core::serde_json::json!({}),
            h.runtime().outputs_tx,
            peer_cell_dir.clone(),
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            -1,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned(Path::new("/peer"), peer_spawned).await;
    let lr_spawned = factory
        .spawn_cell(
            Path::new("/lr"),
            meclaw_core::serde_json::json!({}),
            h.runtime().outputs_tx,
            lr_cell_dir.clone(),
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            -1,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned(Path::new("/lr"), lr_spawned).await;

    // Connect → /lr active (initial spawn has live stop wiring).
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope":"/","diff":{"add_edges":[{"from":"lr","to":"peer"}]}
        }),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));

    // First disconnect: peace-stops the cell (live stop_tx) → NotYetSpawned,
    // stop_tx consumed (None), active=false. Commits cleanly.
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope":"/","diff":{"remove_edges":[{"match":{"from":"lr","to":"peer"}}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "first disconnect (live stop) commits, got {outcome:?}"
    );

    // Reconnect eager: re-spawn → Awake. The RespawnFn RE-NOTIFIES the fresh stop
    // pair (T6), so the colony restores `entry.stop_tx`.
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope":"/","diff":{"add_edges":[{"from":"lr","to":"peer"}]}
        }),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));
    assert_eq!(spawns.load(Ordering::SeqCst), 2, "reconnect re-spawned");

    // The re-notify is a fire-and-forget ColonyMsg; wait until the colony has
    // processed it (entry.stop_tx restored) by polling the registry until the
    // entry is Awake AND a subsequent disconnect would find live stop wiring.
    // We can't read stop_tx directly; instead drive the SECOND disconnect and
    // assert it COMMITS (it can only commit if stop wiring was restored). To
    // avoid racing the re-notify, give the colony a moment via a registry read
    // round-trip (ordered after the StopWiringRestored on the same inbox).
    let e = registry_entry(&h, "/lr").await.expect("/lr");
    assert!(e.active && e.lifecycle_status == "Awake");

    // ── SECOND disconnect: Awake + stop_tx RESTORED → peace-stoppable. ────────
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope":"/","diff":{"remove_edges":[{"match":{"from":"lr","to":"peer"}}]}
        }),
    )
    .await;

    // (a) COMMITS — the restored stop_tx let the colony peace-stop the cell
    // instead of hitting the interim `stop_wiring_unavailable` guard.
    match &outcome {
        MutationOutcome::Committed { .. } => {}
        MutationOutcome::Rejected { error_code, .. } => {
            panic!("second disconnect after re-notify must COMMIT, got Rejected({error_code})")
        }
    }

    // (b) The cell actually peace-stopped: it is now inactive + NotYetSpawned
    // (the disconnect flipped active=false and the peace-stop parked the task).
    let e = registry_entry(&h, "/lr").await.expect("/lr");
    assert!(!e.active, "cell deactivated by the committed disconnect");
    assert_eq!(
        e.lifecycle_status, "NotYetSpawned",
        "peace-stopped task parked (no live task)"
    );

    h.shutdown().await;
}

/// T6 Demo (e), crash-restart path (path 2): a `CellDied{was_panic:true}` drives
/// `handle_cell_died` → `(entry.respawn)()`, whose closure RE-NOTIFIES the fresh
/// stop pair. After the restart the cell regains a live `stop_tx`, so a
/// subsequent disconnect COMMITS (no `stop_wiring_unavailable`) and peace-stops.
/// `/peer` keeps its INITIAL live stop wiring throughout (never restarted), so
/// the disconnect of the `lr→peer` edge finds live stop wiring on both ends.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn crash_restart_renotifies_stop_pair_then_disconnect_commits() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/peer/config.json",
        r#"{"cell":{"type":"lr","timeout":-1},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let spawns = Arc::new(AtomicI64::new(0));
    let lr_cell_dir = td.path().join("main/lr");
    std::fs::create_dir_all(&lr_cell_dir).unwrap();
    let factory: Arc<dyn CellFactory> = Arc::new(RenotifyProbeFactory {
        spawns: spawns.clone(),
        // /lr panics on its first message → a REAL CellDied(panic) drives the
        // crash-restart corridor (handle_cell_died → respawn → re-notify).
        panic_first: true,
    });
    // `/peer` is never restarted here, so its initial live stop wiring survives.
    let peer_spawns = Arc::new(AtomicI64::new(0));
    let peer_factory: Arc<dyn CellFactory> = Arc::new(RenotifyProbeFactory {
        spawns: peer_spawns.clone(),
        panic_first: false,
    });
    let peer_cell_dir = td.path().join("main/peer");
    std::fs::create_dir_all(&peer_cell_dir).unwrap();
    let h = ColonyHandle::new_with_factories_at(&td, vec![("lr".to_string(), factory.clone())]);

    let peer_spawned = peer_factory
        .clone()
        .spawn_cell(
            Path::new("/peer"),
            meclaw_core::serde_json::json!({}),
            h.runtime().outputs_tx,
            peer_cell_dir.clone(),
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            -1,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned(Path::new("/peer"), peer_spawned).await;
    let lr_spawned = factory
        .spawn_cell(
            Path::new("/lr"),
            meclaw_core::serde_json::json!({}),
            h.runtime().outputs_tx,
            lr_cell_dir.clone(),
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            -1,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned(Path::new("/lr"), lr_spawned).await;

    // Connect → both /lr and /peer active with live (initial) stop wiring.
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope":"/","diff":{"add_edges":[{"from":"lr","to":"peer"}]}
        }),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));

    // Crash /lr for real: send it a message; the initial cell panics on its first
    // handle → the watcher reports a genuine `CellDied{was_panic:true}` →
    // `handle_cell_died` → `(entry.respawn)()`, whose closure RE-NOTIFIES the
    // fresh stop pair. /lr stays active+Awake (restart, not disconnect); its
    // stop_tx is restored via StopWiringRestored. (A real panic — not an injected
    // CellDied — so the old task genuinely dies, leaving no orphan whose clean
    // exit would later remove the entry.)
    h.send(
        MessageBuilder::new(Path::new("/lr"))
            .body(Body::Inline(
                meclaw_core::serde_json::json!({"messages":[]}),
            ))
            .build(),
    )
    .await;

    // Wait until the respawn ran (spawns 1→2). The respawn's StopWiringRestored is
    // enqueued inside the same handle_cell_died iteration; the registry read below
    // is therefore ordered after it.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while spawns.load(Ordering::SeqCst) < 2 {
        assert!(
            std::time::Instant::now() < deadline,
            "crash-restart did not re-spawn /lr within 30s"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    // Registry read round-trip: ordered after the StopWiringRestored → guarantees
    // it has been applied before the disconnect below.
    let e = registry_entry(&h, "/lr").await.expect("/lr");
    assert!(
        e.active && e.lifecycle_status == "Awake",
        "crash-restarted /lr is active+Awake"
    );

    // Disconnect: both ends deactivate. /lr has its RE-NOTIFIED stop_tx, /peer its
    // INITIAL one → both peace-stoppable → COMMITS (no stop_wiring_unavailable).
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope":"/","diff":{"remove_edges":[{"match":{"from":"lr","to":"peer"}}]}
        }),
    )
    .await;
    match &outcome {
        MutationOutcome::Committed { .. } => {}
        MutationOutcome::Rejected { error_code, .. } => panic!(
            "disconnect after crash-restart re-notify must COMMIT, got Rejected({error_code})"
        ),
    }

    // /lr peace-stopped: inactive + NotYetSpawned (task parked).
    let e = registry_entry(&h, "/lr").await.expect("/lr");
    assert!(!e.active, "/lr deactivated by the committed disconnect");
    assert_eq!(
        e.lifecycle_status, "NotYetSpawned",
        "peace-stopped task parked (no live task)"
    );

    h.shutdown().await;
}
