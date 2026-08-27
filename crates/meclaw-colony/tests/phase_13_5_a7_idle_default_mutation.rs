//! Phase-13.5 Slice-6 (A7) — `colony.json` `idle_timeout_default_ms` reaches
//! EVERY spawn path that fills in the colony-wide idle default (Reviewer-Auflage
//! A3, completed in the Slice-6 Nachzieh-Fix).
//!
//! Deterministic proof (mirrors the A2 `TimerRecorderFactory` pattern in
//! `phase_13_5_lifecycle_3b_reconnect.rs`): a recorder factory captures the
//! `idle_timeout: Option<Duration>` each spawn path hands it — both in
//! `spawn_cell` (mutation single-cell + swap re-spawn) AND in
//! `build_boot_inactive_respawn` (boot-inactive registration + subtree
//! instantiation + inactive-swap rebuild). A template / config with
//! `cell.timeout: 0` and NO per-cell `idle_timeout_ms` must inherit the
//! colony.json default, NOT the 60_000ms `DEFAULT_IDLE_TIMEOUT_MS` constant.
//!
//! Cases:
//! - `mutation_spawn_inherits_colony_json_idle_default` — single-cell `add_nodes`
//!   (`spawn_cell`, colony.rs:2607 — already wired before this fix).
//! - `boot_inactive_cell_inherits_colony_json_idle_default` — (a) a parentless
//!   boot-inactive cell (`bootstrap_apply::register_inactive_non_spawned`).
//! - `subtree_spawn_inherits_colony_json_idle_default` — (b) a subtree-template
//!   `add_nodes` (`colony.rs` subtree-instantiation loop).

use meclaw_colony::{
    CellFactory, ColonyMsg, DbConn, MutationOutcome, RespawnFn, SpawnedCellKind,
    cell_task_long_running,
};
use meclaw_core::{CellEmission, JsonValue, Message, Path, Uuid};
use meclaw_testing::ColonyHandle;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

/// Build the closure that constructs a fresh long-running cell-task. Shared by
/// `spawn_cell` (eager initial spawn / swap re-spawn) and
/// `build_boot_inactive_respawn` (boot-inactive: respawn only) so both entry
/// points wire an identical task — the recorder only differs in WHEN the
/// `idle_timeout` arg is captured.
fn make_build(
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
}

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
    ack_rx
        .await
        .unwrap()
        .expect("GH #440: the rescan must not have aborted");
}

/// Records the `idle_timeout` (ms, -1 = None) the spawn path handed the factory.
struct IdleRecorderFactory {
    last_idle_ms: Arc<AtomicI64>,
}

impl CellFactory for IdleRecorderFactory {
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
        idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<std::time::Duration>,
        _blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        _mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        self.last_idle_ms.store(
            idle_timeout.map(|d| d.as_millis() as i64).unwrap_or(-1),
            Ordering::SeqCst,
        );
        let build = make_build(path, outputs_tx, colony_inbox_tx);
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
        // Boot-inactive / subtree / inactive-swap paths flow through here. Record
        // the idle the path computed, WITHOUT spawning a task at boot (boot-gating
        // preserved — we only hand back the respawn closure).
        self.last_idle_ms.store(
            idle_timeout.map(|d| d.as_millis() as i64).unwrap_or(-1),
            Ordering::SeqCst,
        );
        Some(Box::new(make_build(path, outputs_tx, colony_inbox_tx)))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutation_spawn_inherits_colony_json_idle_default() {
    let td = tempfile::TempDir::new().unwrap();
    // colony.json: small idle default (≠ the 60_000ms constant).
    write(
        td.path(),
        "colony.json",
        r#"{"idle_timeout_default_ms": 150}"#,
    );
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );

    // Template: cell.timeout = 0 (idle model), NO per-cell idle_timeout_ms override
    // → the mutation-spawn path must fill in the colony.json default.
    let tpl_dir = td.path().join("templates").join("recorder");
    std::fs::create_dir_all(&tpl_dir).unwrap();
    std::fs::write(tpl_dir.join("template.json"), r#"{"name":"recorder"}"#).unwrap();
    std::fs::write(
        tpl_dir.join("config.json"),
        r#"{"cell":{"type":"recorder","timeout":0},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    let last_idle_ms = Arc::new(AtomicI64::new(-2));
    let factory: Arc<dyn CellFactory> = Arc::new(IdleRecorderFactory {
        last_idle_ms: last_idle_ms.clone(),
    });
    let h = ColonyHandle::new_with_factories_at(&td, vec![("recorder".to_string(), factory)]);
    rescan_templates(&h, td.path().join("templates")).await;

    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name":"rec","template":"recorder"}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "add_nodes(recorder) must commit; got {outcome:?}"
    );

    assert_eq!(
        last_idle_ms.load(Ordering::SeqCst),
        150,
        "mutation-spawn must hand the factory idle_timeout = colony.json default (150ms), \
         NOT the 60_000ms DEFAULT_IDLE_TIMEOUT_MS constant"
    );

    h.shutdown().await;
}

// (a) Boot-inactive registration path (`register_inactive_non_spawned`) is
// covered in `phase_13_5_slice_4_boot_inactive_eager_reconnect.rs` — that harness
// already drives a genuine boot-inactive cell (status flipped to 'inactive' in
// colony.db between two boots), which is where `build_boot_inactive_respawn` is
// reached at boot. A fresh nested-hive cell would boot ACTIVE and route through
// the already-wired eager `spawn_cell` path instead, so it cannot prove site 1.

/// (b) Subtree-instantiation path (the `colony.rs` subtree-spawn loop): an
/// `add_nodes` of a SUBTREE template registers each inner cell inactive via
/// `build_boot_inactive_respawn`. With NO per-cell `idle_timeout_ms` override and
/// a colony.json default of 7000, the computed idle must be 7000, NOT the
/// 60_000ms constant.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subtree_spawn_inherits_colony_json_idle_default() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "colony.json",
        r#"{"idle_timeout_default_ms": 7000}"#,
    );
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );

    // Subtree template: root hive + one internal edge + two recorder cells, none
    // with a per-cell idle_timeout_ms override.
    let tpl = td.path().join("templates").join("sub");
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), r#"{"name":"sub"}"#).unwrap();
    std::fs::write(
        tpl.join("config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./inner_a","to":"./inner_b"}]}}}"#,
    )
    .unwrap();
    write(
        &tpl,
        "inner_a/config.json",
        r#"{"cell":{"type":"recorder"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        &tpl,
        "inner_b/config.json",
        r#"{"cell":{"type":"recorder"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let last_idle_ms = Arc::new(AtomicI64::new(-2));
    let factory: Arc<dyn CellFactory> = Arc::new(IdleRecorderFactory {
        last_idle_ms: last_idle_ms.clone(),
    });
    let h = ColonyHandle::new_with_factories_at(&td, vec![("recorder".to_string(), factory)]);
    rescan_templates(&h, td.path().join("templates")).await;

    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name":"m1","template":"sub"}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "add_nodes(subtree) must commit; got {outcome:?}"
    );

    assert_eq!(
        last_idle_ms.load(Ordering::SeqCst),
        7000,
        "subtree-spawn must compute idle_timeout = colony.json default (7000ms), \
         NOT the 60_000ms DEFAULT_IDLE_TIMEOUT_MS constant"
    );

    h.shutdown().await;
}
