//! Phase-13.5 Lifecycle-3b Task 9.1, demo (g): boot-gating of inactive cells.
//!
//! Spec (startup step 6): "Long-Running only for active cells". At boot, a
//! rehydrated eager (Long-Running / Stateless) cell whose persisted
//! `registry.status == 'inactive'` must NOT get a running task — the registry
//! entry must still EXIST (with `active == false`), but no cell-task is spawned
//! and no polling starts.
//!
//! Proof discipline: a recording Long-Running factory increments a shared
//! spawn-counter every time its `spawn_cell` is invoked (= a real cell task is
//! built). On the reboot where `/lr` is persisted `inactive`, the counter must
//! stay at 0 — the boot path never asked the factory to build the task. The
//! registry-entry-exists + `active == false` + status-survives-reboot facts are
//! read from the RAM registry and the colony.db round-trip.

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, DbConn, RespawnFn, SpawnedCellKind,
    bootstrap_from_filesystem, cell_task_long_running,
};
use meclaw_core::{CellEmission, JsonValue, Message, Path};
use meclaw_testing::ColonyHandle;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

/// A Long-Running factory that records how many times `spawn_cell` (= a real
/// cell-task build) was invoked. Used to prove that boot does NOT build a task
/// for an inactive cell (spawn-count == 0) while still building it when active.
struct RecordingLrFactory {
    /// Incremented once per `spawn_cell` call (one per real task build).
    spawn_count: Arc<AtomicU32>,
}

/// Build the long-running task pair, recording each real task build in
/// `spawn_count`. Shared by `spawn_cell` (eager initial spawn / respawn) and
/// `build_boot_inactive_respawn` (the boot-inactive eager-reconnect hook, T7).
fn make_lr_build(
    spawn_count: Arc<AtomicU32>,
    path: Path,
    outputs_tx: mpsc::Sender<CellEmission>,
    colony_inbox_tx: mpsc::Sender<ColonyMsg>,
) -> impl Fn() -> (
    mpsc::Sender<Message>,
    JoinHandle<()>,
    oneshot::Receiver<()>,
    oneshot::Receiver<()>,
) + Send
+ Sync
+ 'static {
    move || {
        // Each build = a real task → record it.
        spawn_count.fetch_add(1, Ordering::SeqCst);
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
            )
            .await;
        });
        (tx, join, peace_rx, backstop_rx)
    }
}

impl CellFactory for RecordingLrFactory {
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
        let build = make_lr_build(self.spawn_count.clone(), path, outputs_tx, colony_inbox_tx);
        let (sender, join, peace_rx, backstop_rx) = build();
        let (stop_tx, _stop_rx) = oneshot::channel::<()>();
        let (death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
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

    /// T7 hook: a boot-inactive eager (long-running) cell gets the SAME real
    /// respawn closure WITHOUT spawning the task at boot (counter stays 0). A
    /// later crossing `add_edges` reconnect invokes it → immediate spawn.
    fn build_boot_inactive_respawn(
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
    ) -> Option<RespawnFn> {
        // Build (but do NOT call) → no task at boot, boot-gating preserved.
        let build = make_lr_build(self.spawn_count.clone(), path, outputs_tx, colony_inbox_tx);
        Some(Box::new(build))
    }
}

/// `(name, factory)` list with `lr` registered against a shared recorder.
fn lr_factories(spawn_count: Arc<AtomicU32>) -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "lr".to_string(),
        Arc::new(RecordingLrFactory { spawn_count }) as Arc<dyn CellFactory>,
    )]
}

/// Same list as a `CellFactoryRegistry` for `bootstrap_from_filesystem`.
fn lr_registry(spawn_count: Arc<AtomicU32>) -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert(
        "lr".into(),
        Arc::new(RecordingLrFactory { spawn_count }) as Arc<dyn CellFactory>,
    );
    r
}

/// Root hive with a self-edge (so colony.db classifies as Reboot on boot 2)
/// plus one long-running cell `/lr`. The hive `/main` edge is `lr→lr` so the
/// edges + hive_scopes + registry tables are all populated.
fn write_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main/lr")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"lr","to":"lr"}]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/lr/config.json"),
        r#"{"cell":{"type":"lr"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

/// A7 (Phase-16 W1a, Option B) island topology: root hive `/` plus an ISLAND
/// sub-hive `/iso` (no external/crossing edge) containing one long-running cell
/// `/iso/lr` wired only INTERNALLY (`lr→lr` self-edge). That internal edge SEEDS
/// the boot connectivity recompute over `/iso`'s scope → the recompute REACHES
/// `/iso/lr` and derives it INACTIVE (the disconnected hive gates the subtree).
/// `/iso/lr` must therefore not be spawned.
fn write_island_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main/iso/lr")).unwrap();
    std::fs::write(td.join("main/config.json"), r#"{"cell":{"type":"hive"}}"#).unwrap();
    std::fs::write(
        td.join("main/iso/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"lr","to":"lr"}]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/iso/lr/config.json"),
        r#"{"cell":{"type":"lr"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

/// Demo (c) / A7: a long-running cell inside an ISLAND subtree must NOT be
/// spawned at the FIRST bootstrap — the first boot derives the edge-activity
/// from t0 (no grace), so the island is inactive and its task is never built
/// (spawn-count == 0). The registry entry exists with `active == false`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn island_long_running_not_spawned_at_first_boot() {
    let td = TempDir::new().unwrap();
    write_island_topology(td.path());

    let count = Arc::new(AtomicU32::new(0));
    let h = ColonyHandle::new_with_factories_at(&td, lr_factories(count.clone()));
    bootstrap_from_filesystem(td.path(), &lr_registry(count.clone()), &h.runtime())
        .await
        .expect("first boot of an island topology must succeed");

    assert_eq!(
        count.load(Ordering::SeqCst),
        0,
        "first boot must NOT build a task for the island long-running /iso/lr (A7)"
    );
    assert_eq!(
        ram_active(&h, "/iso/lr").await,
        Some(false),
        "/iso/lr must register inactive at first boot (island, A7)"
    );
    h.shutdown().await;
}

/// Send a mutation through the colony inbox and await its outcome.
async fn send_mutation(
    h: &ColonyHandle,
    payload: meclaw_core::JsonValue,
) -> meclaw_colony::MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: meclaw_core::Uuid::now_v7(),
            parent_message_id: meclaw_core::Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap()
}

/// Demo (d) / K-H5: an A7-island long-running cell that boots INACTIVE is NOT
/// permanently dead — a later `add_edges` whose edge CROSSES the island's scope
/// boundary reactivates the cascade and spawns the long-running task
/// IMMEDIATELY (the K-H5 reconnect mechanic). Ties the new A7 boot behavior to
/// the existing crossing-edge activation: boot inactive (count == 0) →
/// crossing add_edges → active + spawned (count bumps).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn island_long_running_activates_on_crossing_add_edges() {
    let td = TempDir::new().unwrap();
    write_island_topology(td.path());

    let count = Arc::new(AtomicU32::new(0));
    let h = ColonyHandle::new_with_factories_at(&td, lr_factories(count.clone()));
    bootstrap_from_filesystem(td.path(), &lr_registry(count.clone()), &h.runtime())
        .await
        .expect("first boot of the island topology must succeed");
    // A7: island boots inactive, task not built.
    assert_eq!(
        count.load(Ordering::SeqCst),
        0,
        "island /iso/lr inactive at boot"
    );
    assert_eq!(ram_active(&h, "/iso/lr").await, Some(false));

    // An external anchor cell (registry-only) provides the outside endpoint.
    h.spawn(Path::new("/anchor"), || {
        meclaw_testing::mocks::EchoMockCell::new(Path::new("/anchor"))
    })
    .await;

    // Crossing add_edges at the root scope: `/anchor → /iso/lr` has exactly one
    // endpoint strictly inside `/iso` → it crosses the boundary and connects
    // the island (R12 crossing connectivity), reactivating its subtree.
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_edges": [{"from": "./anchor", "to": "./iso/lr"}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, meclaw_colony::MutationOutcome::Committed { .. }),
        "crossing add_edges must commit, got {outcome:?}"
    );

    // K-H5: the reactivated long-running cell spawns immediately (no reboot).
    assert_eq!(
        ram_active(&h, "/iso/lr").await,
        Some(true),
        "/iso/lr must be active after the crossing add_edges"
    );
    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "the reactivated /iso/lr long-running task must be spawned exactly once"
    );
    h.shutdown().await;
}

/// Read the in-memory `active` flag for `path`, or `None` if absent.
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
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), ack_rx)
        .await
        .expect("ReadRegistry ack timed out")
        .unwrap();
    reply
        .entries
        .into_iter()
        .find(|e| e.path == path)
        .map(|e| e.active)
}

/// Demo (g): an eager Long-Running cell persisted `status='inactive'` must NOT
/// be spawned at boot. The registry entry exists with `active == false`, no
/// task is built (spawn-count == 0), and the inactive status survives the reboot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn boot_does_not_spawn_inactive_long_running_cell() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());

    // ── Boot 1: /lr is fresh → active → its task IS built (spawn-count == 1). ─
    let count1 = Arc::new(AtomicU32::new(0));
    let h = ColonyHandle::new_with_factories_at(&td, lr_factories(count1.clone()));
    bootstrap_from_filesystem(td.path(), &lr_registry(count1.clone()), &h.runtime())
        .await
        .expect("boot1 bootstrap must succeed");
    assert_eq!(
        ram_active(&h, "/lr").await,
        Some(true),
        "boot1 /lr must be active"
    );
    assert_eq!(
        count1.load(Ordering::SeqCst),
        1,
        "boot1 must build the active /lr task exactly once"
    );
    h.shutdown().await;

    // ── Flip /lr to 'inactive' directly in colony.db between the two boots. ───
    {
        let conn = rusqlite::Connection::open(td.path().join("colony.db")).unwrap();
        let n = conn
            .execute("UPDATE registry SET status='inactive' WHERE path='/lr'", [])
            .unwrap();
        assert_eq!(n, 1, "exactly one /lr row must flip to inactive");
    }

    // ── Boot 2 (fresh recorder): /lr is inactive → MUST NOT be spawned. ──────
    let count2 = Arc::new(AtomicU32::new(0));
    let h2 = ColonyHandle::new_with_factories_at(&td, lr_factories(count2.clone()));
    bootstrap_from_filesystem(td.path(), &lr_registry(count2.clone()), &h2.runtime())
        .await
        .expect("boot2 bootstrap must succeed");

    // (a) The entry still exists, with active == false.
    assert_eq!(
        ram_active(&h2, "/lr").await,
        Some(false),
        "/lr must rehydrate as a registered entry with active == false"
    );
    // (b) No cell task was built for the inactive /lr.
    assert_eq!(
        count2.load(Ordering::SeqCst),
        0,
        "boot2 must NOT build a task for the inactive /lr (boot-gating)"
    );

    h2.shutdown().await;

    // (c) The inactive status survived the reboot (C1: UpsertRegistry must not
    // clobber it back to 'active', and the skip-spawn path does not re-activate).
    let conn = rusqlite::Connection::open(td.path().join("colony.db")).unwrap();
    let status: String = conn
        .query_row("SELECT status FROM registry WHERE path='/lr'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        status, "inactive",
        "/lr status must stay inactive across the reboot"
    );
}
