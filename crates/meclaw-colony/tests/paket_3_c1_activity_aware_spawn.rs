//! Paket-3 P3-C1 + C2: activity-aware single-cell spawn (P8 fix).
//!
//! Bug (P8): a single-cell `add_nodes` of an EAGER long-running cell whose
//! diff-edges derive it INACTIVE (target under an inactive parent hive) is
//! eager-spawned at apply step 9 — its real task runs (real side effects:
//! mcp subprocess, proxy connection) — and the connectivity recompute
//! (step 10b) then peace-stops it sub-second later. The spawn loop runs
//! BEFORE the diff's edges land in `edges`, so the gate must compute activity
//! against the POST-STATE edge view (`edges` ∪ diff-adds − diff-removes).
//!
//! Fix (C1): before the eager `spawn_cell` call, derive the cell's POST-STATE
//! activity. If it would be inactive AND it is an eager kind (the factory
//! offers a real `build_boot_inactive_respawn` — exactly the bootstrap
//! boot-inactive discriminator), register it inactive + NotYetSpawned WITHOUT
//! spawning the task (carrying the real respawn so an `add_edges` reconnect
//! eager-spawns it). The task is NEVER built — no side effect.
//!
//! Grace (C2): a pure `add_nodes`-without-edge under an active/root scope keeps
//! spawning (active, Grace, spec Z.1463ff) — the gegenprobe to C1.
//!
//! Proof discipline: a shared `spawn_count` is bumped ONLY inside `spawn_cell`
//! (the real eager-spawn path that builds + spawns the cell task), NEVER inside
//! `build_boot_inactive_respawn`. So `spawn_count == 0` is a positive proof that
//! NO task was ever spawned for the would-be-inactive cell.

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{
    CellFactory, ColonyMsg, DbConn, MutationOutcome, RespawnFn, SpawnedCellKind,
    cell_task_long_running,
};
use meclaw_core::{CellEmission, JsonValue, Message, Path, Uuid};
use meclaw_testing::ColonyHandle;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

// ───────────────────────────────────────────────────────────────────────────
// Eager long-running mock factory. `spawn_count` bumps ONLY in `spawn_cell`
// (the real build+spawn path). `build_boot_inactive_respawn` is overridden
// (returns `Some`) — this is the eager-kind discriminator the C1 gate keys on,
// and it does NOT bump the counter (no task built at registration time).
// ───────────────────────────────────────────────────────────────────────────

struct EagerLrFactory {
    /// Bumped once per REAL task build inside `spawn_cell` (never in the
    /// boot-inactive respawn). `0` ⇒ the cell task was never spawned.
    spawn_count: Arc<AtomicU32>,
}

fn make_build(
    spawn_count: Arc<AtomicU32>,
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

impl CellFactory for EagerLrFactory {
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
        let build = make_build(self.spawn_count.clone(), path, outputs_tx, colony_inbox_tx);
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
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<std::time::Duration>,
        _blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        _mailbox_capacity: usize,
    ) -> Option<RespawnFn> {
        // Same wiring as `spawn_cell`'s respawn — but we do NOT call `build()`,
        // so no task is spawned at registration (the counter stays 0). This is
        // the eager-kind discriminator the C1 gate keys on.
        let build = make_build(self.spawn_count.clone(), path, outputs_tx, colony_inbox_tx);
        Some(Box::new(build))
    }
}

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
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
            limit: 200,
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
}

/// Write a single-cell long-running `lr` template (cell.timeout = -1, persistent
/// runs-forever — a real eager kind) plus a root hive containing an INACTIVE
/// sub-hive `/main/h` (no incoming edge → disconnected → inactive subtree).
fn setup(dir: &std::path::Path) {
    // Root hive `/main`; sub-hive `/main/h` with NO incoming edge → inactive.
    write(
        dir,
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        dir,
        "main/h/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    // Template `lr` (single-cell, long-running, persistent).
    let tpl = dir.join("templates").join("lr");
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), r#"{"name":"lr"}"#).unwrap();
    std::fs::write(
        tpl.join("config.json"),
        r#"{"cell":{"type":"lr","timeout":-1},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

/// C1 main + A3 pin: `add_nodes` of an eager LR cell UNDER the inactive sub-hive
/// `/main/h`, with the SAME diff carrying the `add_edges` (lr→sink) that wires
/// it — the deriving edge exists ONLY in the diff buffer, not yet in committed
/// `edges`. The POST-STATE view derives `/main/h/lr` inactive (parent hive
/// inactive) → the task is NEVER spawned (spawn_count stays 0), the node is
/// registered inactive + NotYetSpawned.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_nodes_eager_lr_under_inactive_hive_is_not_spawned() {
    let td = tempfile::TempDir::new().unwrap();
    setup(td.path());

    let spawn_count = Arc::new(AtomicU32::new(0));
    let factory: Arc<dyn CellFactory> = Arc::new(EagerLrFactory {
        spawn_count: spawn_count.clone(),
    });
    let h = ColonyHandle::new_with_factories_at(&td, vec![("lr".to_string(), factory)]);
    meclaw_colony::bootstrap_from_filesystem(
        td.path(),
        &{
            let mut r = meclaw_colony::CellFactoryRegistry::new();
            r.insert(
                "lr".to_string(),
                Arc::new(EagerLrFactory {
                    spawn_count: spawn_count.clone(),
                }) as Arc<dyn CellFactory>,
            );
            r
        },
        &h.runtime(),
    )
    .await
    .expect("bootstrap must succeed");
    rescan_templates(&h, td.path().join("templates")).await;

    // Boot baseline: only the two root/sub hives exist (no cells) → no spawn.
    let boot_count = spawn_count.load(Ordering::SeqCst);

    // add_nodes `lr` + `sink` under scope `/main/h` + add_edges lr→sink in ONE
    // diff. `/main/h/lr` is connected (edge to sink, present ONLY in the diff
    // buffer — A3) but its parent hive `/main/h` is inactive (no incoming edge)
    // → the POST-STATE view derives `/main/h/lr` inactive.
    // Scope `/h` is the LOGICAL sub-hive path (the root cell dir `main` is
    // stripped from logical paths, spec Z.331) — the bootstrap registered the
    // hive scope as `/h`, so the staged cells land at `/h/lr` + `/h/sink`.
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/h",
            "diff": {
                "add_nodes": [
                    {"name":"lr","template":"lr"},
                    {"name":"sink","template":"lr"}
                ],
                "add_edges": [{"from":"lr","to":"sink"}]
            }
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "mutation must commit; got {outcome:?}"
    );

    // C1: the eager spawn path (`spawn_cell`) was NEVER taken for `/main/h/lr`
    // → no new task built → spawn_count unchanged from boot baseline.
    assert_eq!(
        spawn_count.load(Ordering::SeqCst),
        boot_count,
        "would-be-inactive eager LR cell must NOT be eager-spawned (no task built)"
    );

    // The node IS registered (logical path `/h/lr`), inactive, not running.
    let e = registry_entry(&h, "/h/lr")
        .await
        .expect("/h/lr must be registered");
    assert!(!e.active, "/h/lr must be registered inactive");
    assert_eq!(
        e.lifecycle_status, "NotYetSpawned",
        "/h/lr must be parked NotYetSpawned (no running task)"
    );

    h.shutdown().await;
}

/// C2 grace pin (gegenprobe): pure `add_nodes`-WITHOUT a reaching edge under the
/// ACTIVE root scope → the cell stays active (Grace, spec Z.1463ff) → it IS
/// eager-spawned (spawn_count bumps). Same factory, same kind — the only
/// difference from C1 is the absence of a deriving edge / inactive parent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_nodes_eager_lr_without_edge_under_root_spawns_grace() {
    let td = tempfile::TempDir::new().unwrap();
    // Plain root hive, no inactive sub-hive.
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    let tpl = td.path().join("templates").join("lr");
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), r#"{"name":"lr"}"#).unwrap();
    std::fs::write(
        tpl.join("config.json"),
        r#"{"cell":{"type":"lr","timeout":-1},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    let spawn_count = Arc::new(AtomicU32::new(0));
    let factory: Arc<dyn CellFactory> = Arc::new(EagerLrFactory {
        spawn_count: spawn_count.clone(),
    });
    let h = ColonyHandle::new_with_factories_at(&td, vec![("lr".to_string(), factory)]);
    rescan_templates(&h, td.path().join("templates")).await;

    // Pure add_nodes, NO edge → edge-less fresh cell under root → active (Grace).
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name":"lr","template":"lr"}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "grace add_nodes must commit; got {outcome:?}"
    );

    // C2: edge-less fresh cell under root is active by Grace → eager-spawned.
    assert_eq!(
        spawn_count.load(Ordering::SeqCst),
        1,
        "edge-less fresh LR cell under root must eager-spawn (Grace preserved)"
    );
    let e = registry_entry(&h, "/lr").await.expect("/lr registered");
    assert!(e.active, "/lr active (Grace)");
    assert_eq!(e.lifecycle_status, "Awake", "/lr Awake (eager-spawned)");

    h.shutdown().await;
}
