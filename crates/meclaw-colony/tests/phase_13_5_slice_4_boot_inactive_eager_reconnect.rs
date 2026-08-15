//! Phase-13.5 Slice 4 T7, demo (f): a boot-inactive eager cell runs IMMEDIATELY
//! on `add_edges`-reconnect — WITHOUT a reboot.
//!
//! Gap before T7 (checklist c): an eager (Long-Running / Stateless) cell that
//! rehydrates with persisted `status != 'active'` is registered with an INERT
//! no-op `RespawnFn`/`WakeFn` (the real task-wiring was never built for the
//! boot-inactive case). After `add_edges` reactivates it, the reconnect arm
//! flips `active = true` but the cell never actually runs until the next reboot.
//!
//! T7 fix: a boot-inactive eager cell receives a REAL respawn closure (built via
//! the new `CellFactory::build_boot_inactive_respawn` hook — same construction a
//! normal eager cell's `RespawnFn` gets, WITHOUT eager-spawning it at boot) and
//! registers as `eager_on_reconnect == true`. The reconnect arm's
//! `(entry.respawn)()` then spawns the task immediately.
//!
//! Proof discipline (positive receipt, no reboot):
//! 1. At boot the spawn-counter stays 0 (boot-gating preserved — no task built).
//! 2. After `add_edges` reconnect, the spawn-counter bumps to 1 WITHOUT any
//!    message (the reconnect arm re-spawned the task immediately).
//! 3. A message routed to the now-live cell bumps its `handle_calls` (it really
//!    processes traffic — a live cell, not a parked inert slot).

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{
    CellFactory, ColonyMsg, DbConn, MutationOutcome, RespawnFn, SpawnedCellKind,
    bootstrap_from_filesystem, cell_task_long_running,
};
use meclaw_core::{Body, CellEmission, JsonValue, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU32, AtomicUsize, Ordering};
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

/// A Long-Running factory that records (a) how many times a real cell-task was
/// built (the `build` closure runs) and (b) the latest spawned cell's
/// `handle_calls` counter (so the test can prove the live cell processes a
/// routed message). It implements `build_boot_inactive_respawn` so a
/// boot-inactive eager cell can obtain its REAL respawn WITHOUT building (and
/// thus WITHOUT spawning) a task at boot.
struct BootInactiveEagerFactory {
    /// Incremented once per real task build (initial eager-spawn or re-spawn).
    spawn_count: Arc<AtomicU32>,
    /// Mirror of the latest spawned cell's `handle_calls` (message receipts).
    handle_calls: Arc<AtomicUsize>,
    /// Last `idle_timeout` (ms, -1 = None) the boot path handed this factory via
    /// `build_boot_inactive_respawn`. Phase-13.5 Slice-6 Nachzieh-Fix (A3 site 1):
    /// proves the boot-inactive registration computes idle from colony.json, not
    /// the 60_000ms `DEFAULT_IDLE_TIMEOUT_MS` constant.
    last_idle_ms: Arc<std::sync::atomic::AtomicI64>,
}

/// Build the closure that constructs a fresh cell-task. Shared verbatim by
/// `spawn_cell` (eager initial spawn) and `build_boot_inactive_respawn`
/// (boot-inactive: respawn only, no initial spawn) so the wiring is identical to
/// a normal eager cell — the only difference is whether the initial task is
/// spawned at construction time.
fn make_build(
    spawn_count: Arc<AtomicU32>,
    handle_calls: Arc<AtomicUsize>,
    path: Path,
    outputs_tx: mpsc::Sender<CellEmission>,
    colony_inbox_tx: mpsc::Sender<ColonyMsg>,
) -> impl Fn() -> (
    mpsc::Sender<Message>,
    JoinHandle<()>,
    oneshot::Receiver<()>,
    oneshot::Receiver<()>,
) {
    move || {
        spawn_count.fetch_add(1, Ordering::SeqCst);
        let (cell, inject_tx) = meclaw_testing::mocks::ReceiptMockLongRunningCell::new();
        // Mirror this spawn's handle_calls into the shared counter the test
        // polls (the cell bumps its own AtomicUsize per routed message).
        handle_calls.store(0, Ordering::SeqCst);
        let cell_handle_calls = cell.handle_calls.clone();
        let mirror_target = handle_calls.clone();
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
            let mirror = tokio::spawn(async move {
                loop {
                    let v = cell_handle_calls.load(Ordering::SeqCst) as u32;
                    mirror_target.store(v as usize, Ordering::SeqCst);
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
                None,
                None,
                None,
                None,
            )
            .await;
            mirror.abort();
        });
        (tx, join, peace_rx, backstop_rx)
    }
}

impl CellFactory for BootInactiveEagerFactory {
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
        let build = make_build(
            self.spawn_count.clone(),
            self.handle_calls.clone(),
            path,
            outputs_tx,
            colony_inbox_tx,
        );
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

    fn build_boot_inactive_respawn(
        self: Arc<Self>,
        path: Path,
        _params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        _cell_dir: std::path::PathBuf,
        _contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<std::time::Duration>,
        _blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        _mailbox_capacity: usize,
    ) -> Option<RespawnFn> {
        // Record the idle the boot-inactive path computed (A3 site 1 proof).
        self.last_idle_ms.store(
            idle_timeout.map(|d| d.as_millis() as i64).unwrap_or(-1),
            Ordering::SeqCst,
        );
        // Same wiring as `spawn_cell`'s respawn, but we DO NOT call `build()` here
        // → no task is spawned and the counter stays 0 at boot (boot-gating).
        let build = make_build(
            self.spawn_count.clone(),
            self.handle_calls.clone(),
            path,
            outputs_tx,
            colony_inbox_tx,
        );
        Some(Box::new(build))
    }
}

fn factory_pair(
    spawn_count: Arc<AtomicU32>,
    handle_calls: Arc<AtomicUsize>,
    last_idle_ms: Arc<AtomicI64>,
) -> Arc<dyn CellFactory> {
    Arc::new(BootInactiveEagerFactory {
        spawn_count,
        handle_calls,
        last_idle_ms,
    })
}

/// `(type, factory)` list: the recorded `lr` factory (shared counters) plus a
/// `peer` factory with its own throwaway counters (so reactivating `/peer` on
/// reconnect does not perturb the `/lr` assertions).
fn factories(
    lr_count: Arc<AtomicU32>,
    lr_handle: Arc<AtomicUsize>,
    lr_idle: Arc<AtomicI64>,
) -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![
        ("lr".to_string(), factory_pair(lr_count, lr_handle, lr_idle)),
        (
            "peer".to_string(),
            factory_pair(
                Arc::new(AtomicU32::new(0)),
                Arc::new(AtomicUsize::new(0)),
                Arc::new(AtomicI64::new(-2)),
            ),
        ),
    ]
}

/// Same list as a `CellFactoryRegistry` for `bootstrap_from_filesystem`.
fn registry(
    lr_count: Arc<AtomicU32>,
    lr_handle: Arc<AtomicUsize>,
    lr_idle: Arc<AtomicI64>,
) -> meclaw_colony::CellFactoryRegistry {
    let mut r = meclaw_colony::CellFactoryRegistry::new();
    for (name, f) in factories(lr_count, lr_handle, lr_idle) {
        r.insert(name, f);
    }
    r
}

/// Root hive with two long-running cells `/lr` and `/peer`, wired `lr→peer` at
/// boot (so both are active on boot 1). The edge is the reconnect anchor.
fn write_topology(td: &std::path::Path) {
    // colony.json sets a small idle default (≠ the 60_000ms constant) so the
    // boot-inactive registration's computed idle is observable (A3 site 1).
    std::fs::write(
        td.join("colony.json"),
        r#"{"idle_timeout_default_ms": 5000}"#,
    )
    .unwrap();
    std::fs::create_dir_all(td.join("main/lr")).unwrap();
    std::fs::create_dir_all(td.join("main/peer")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"lr","to":"peer"}]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/lr/config.json"),
        r#"{"cell":{"type":"lr"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/peer/config.json"),
        r#"{"cell":{"type":"peer"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

async fn send_mutation(h: &ColonyHandle, payload: JsonValue) -> MutationOutcome {
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
    tokio::time::timeout(std::time::Duration::from_secs(30), ack_rx)
        .await
        .expect("ReadRegistry ack timed out")
        .unwrap()
        .entries
        .into_iter()
        .find(|e| e.path == path)
        .map(|e| e.active)
}

/// Demo (f): boot an eager Long-Running cell as `inactive`, then `add_edges` to
/// reconnect it, and prove it runs IMMEDIATELY (spawn-counter bumps + it
/// processes a routed message) WITHOUT any reboot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn boot_inactive_eager_runs_immediately_on_reconnect() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());

    // ── Boot 1: /lr is fresh → active → its task IS built (spawn-count == 1). ─
    let count1 = Arc::new(AtomicU32::new(0));
    let hc1 = Arc::new(AtomicUsize::new(0));
    let idle1 = Arc::new(AtomicI64::new(-2));
    let h = ColonyHandle::new_with_factories_at(
        &td,
        factories(count1.clone(), hc1.clone(), idle1.clone()),
    );
    bootstrap_from_filesystem(
        td.path(),
        &registry(count1.clone(), hc1.clone(), idle1.clone()),
        &h.runtime(),
    )
    .await
    .expect("boot1 bootstrap must succeed");
    assert_eq!(ram_active(&h, "/lr").await, Some(true), "boot1 /lr active");
    h.shutdown().await;

    // ── Between boots: flip both /lr and /peer to 'inactive' directly in
    // colony.db (the lr→peer edge stays, so the boot-state probe sees a
    // consistent Reboot). This rehydrates /lr as a boot-inactive eager cell. ──
    {
        let conn = rusqlite::Connection::open(td.path().join("colony.db")).unwrap();
        let n = conn
            .execute(
                "UPDATE registry SET status='inactive' WHERE path IN ('/lr','/peer')",
                [],
            )
            .unwrap();
        assert_eq!(n, 2, "both /lr and /peer must flip to inactive");
    }

    // ── Boot 2 (fresh recorders): /lr is inactive → MUST NOT be spawned. ──────
    let count2 = Arc::new(AtomicU32::new(0));
    let hc2 = Arc::new(AtomicUsize::new(0));
    let idle2 = Arc::new(AtomicI64::new(-2));
    let h2 = ColonyHandle::new_with_factories_at(
        &td,
        factories(count2.clone(), hc2.clone(), idle2.clone()),
    );
    bootstrap_from_filesystem(
        td.path(),
        &registry(count2.clone(), hc2.clone(), idle2.clone()),
        &h2.runtime(),
    )
    .await
    .expect("boot2 bootstrap must succeed");

    // (a) Boot-gating preserved: inactive at boot → no task built (count == 0).
    assert_eq!(ram_active(&h2, "/lr").await, Some(false), "/lr inactive");
    assert_eq!(
        count2.load(Ordering::SeqCst),
        0,
        "boot2 must NOT build a task for the inactive /lr (boot-gating)"
    );

    // A3 site 1: the boot-inactive registration handed `build_boot_inactive_respawn`
    // the colony.json idle default (5000ms), NOT the 60_000ms constant. (/lr has
    // cell.timeout == 0 and no per-cell idle_timeout_ms override.)
    assert_eq!(
        idle2.load(Ordering::SeqCst),
        5000,
        "boot-inactive registration must compute idle_timeout = colony.json default \
         (5000ms), NOT the 60_000ms DEFAULT_IDLE_TIMEOUT_MS constant"
    );

    // The persisted lr→peer edge survived the reboot but both endpoints booted
    // inactive (status-driven boot-gating). First remove the edge (both already
    // inactive → no activity change), then re-add it to drive a real
    // false→true recompute on /lr.
    let outcome = send_mutation(
        &h2,
        meclaw_core::serde_json::json!({
            "scope":"/","diff":{"remove_edges":[{"match":{"from":"lr","to":"peer"}}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "remove_edges must commit, got {outcome:?}"
    );

    // ── Reconnect via add_edges: re-add the lr→peer edge → recompute flips
    // /lr active=false→true → eager reconnect arm re-spawns the task NOW. ─────
    let outcome = send_mutation(
        &h2,
        meclaw_core::serde_json::json!({
            "scope":"/","diff":{"add_edges":[{"from":"lr","to":"peer"}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "reconnect add_edges must commit, got {outcome:?}"
    );

    // (b) The cell is active + Awake (a real task is now running) WITHOUT reboot.
    assert_eq!(ram_active(&h2, "/lr").await, Some(true), "/lr active again");

    // (c) The task was re-spawned IMMEDIATELY on reconnect — the spawn-counter
    // bumped to 1 WITHOUT any message having been routed (this is the T7 fix;
    // on HEAD the inert respawn does nothing and the counter stays 0).
    meclaw_testing::wait::wait_for_spawn_count(&count2, 1, std::time::Duration::from_secs(30))
        .await;

    // (d) The reconnected cell really PROCESSES a routed message (positive
    // receipt): send it one terminal message → its handle_calls bumps to 1.
    h2.send(
        MessageBuilder::new(Path::new("/lr"))
            .body(Body::Inline(
                meclaw_core::serde_json::json!({"messages":[]}),
            ))
            .build(),
    )
    .await;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while hc2.load(Ordering::SeqCst) < 1 {
        assert!(
            std::time::Instant::now() < deadline,
            "reconnected /lr did not process a routed message within 30s"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    h2.shutdown().await;
}
