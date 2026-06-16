//! Paket-1 (b) — colony.json `mailbox_default_capacity` is the fallback AND a
//! per-cell `cell.mailbox_size` override wins, both proven through the real
//! bootstrap-from-filesystem spawn path.
//!
//! `bootstrap_apply` resolves `cell.mailbox_size ?? colony.json
//! mailbox_default_capacity ?? 1000` and hands the result to the factory's
//! `mailbox_capacity` arg (see `bootstrap_apply.rs`). A recorder factory
//! captures that value per spawned path, so we can assert:
//!   - cell X (no override) → factory sees the colony default (3),
//!   - cell Y (`cell.mailbox_size: 1`) → factory sees 1.
//!
//! Default of 3 (≠ the hardcoded 1000) makes the X assertion prove the
//! *colony.json default* specifically, not the fallback fallback.

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ContractView, DbConn, RespawnFn, SpawnedCellKind,
    cell_task_long_running,
};
use meclaw_core::{CellEmission, JsonValue, Message, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::bootstrap_apply::bootstrap_from_filesystem;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// Records the `mailbox_capacity` the bootstrap path handed the factory, keyed
/// by the cell's logical path. A live long-running cell task is spawned so the
/// `Active` registration path is real (positive receipt: the cell exists).
struct MailboxRecorderFactory {
    seen: Arc<Mutex<HashMap<String, usize>>>,
}

impl CellFactory for MailboxRecorderFactory {
    fn validate_params(&self, _params: &JsonValue) -> Result<(), String> {
        Ok(())
    }

    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        _params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        _cell_dir: std::path::PathBuf,
        _contract: ContractView,
        colony_inbox_tx: mpsc::Sender<meclaw_colony::ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<std::time::Duration>,
        _blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        self.seen
            .lock()
            .unwrap()
            .insert(path.as_str().to_string(), mailbox_capacity);

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
                let (cell, inject_tx) = meclaw_testing::mocks::ReceiptMockLongRunningCell::new();
                let conn = rusqlite::Connection::open_in_memory().expect("open_in_memory");
                let db = DbConn::wrap(conn, None);
                let (tx, rx) = mpsc::channel::<Message>(mailbox_capacity);
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
                     None,)
                    .await;
                });
                (tx, join, peace_rx, backstop_rx)
            }
        };
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
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bootstrap_default_applies_and_per_cell_override_wins() {
    let td = tempfile::TempDir::new().unwrap();

    // Colony default = 3 (deliberately ≠ the hardcoded 1000 fallback).
    write(
        td.path(),
        "colony.json",
        r#"{"mailbox_default_capacity": 3}"#,
    );
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    // Cell X: no mailbox_size → must inherit the colony default (3).
    write(
        td.path(),
        "main/x/config.json",
        r#"{"cell":{"type":"recorder"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    // Cell Y: mailbox_size:1 override → must win over the default.
    write(
        td.path(),
        "main/y/config.json",
        r#"{"cell":{"type":"recorder","mailbox_size":1},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let seen = Arc::new(Mutex::new(HashMap::<String, usize>::new()));
    let factory: Arc<dyn CellFactory> = Arc::new(MailboxRecorderFactory { seen: seen.clone() });

    let h =
        ColonyHandle::new_with_factories_at(&td, vec![("recorder".to_string(), factory.clone())]);

    let mut registry = CellFactoryRegistry::new();
    registry.insert("recorder".to_string(), factory);
    let report = bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap must succeed");
    assert_eq!(report.cell_count, 2, "two recorder cells bootstrapped");

    let snapshot = seen.lock().unwrap().clone();
    assert_eq!(
        snapshot.get("/x").copied(),
        Some(3),
        "cell X without override must inherit colony.json mailbox_default_capacity (3); seen: {snapshot:?}"
    );
    assert_eq!(
        snapshot.get("/y").copied(),
        Some(1),
        "cell Y with cell.mailbox_size:1 override must win over the default (3); seen: {snapshot:?}"
    );

    h.shutdown().await;
}
