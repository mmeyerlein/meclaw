//! Paket-1 T20 — `cell.mailbox_size` reaches the mutation single-cell spawn
//! path, and a `cell.mailbox_size: 0` is rejected pre-destructively during
//! staging (before the atomic rename — no live change).
//!
//! Deterministic proof (mirrors the A7 `IdleRecorderFactory` pattern in
//! `phase_13_5_a7_idle_default_mutation.rs`): a recorder factory captures the
//! `mailbox_capacity: usize` each `spawn_cell` call hands it. A template with
//! `cell.mailbox_size: 12` (override) must flow through `StagedDir.mailbox_size`
//! into the factory's `mailbox_capacity` arg; without an override the colony.json
//! `mailbox_default_capacity` must be used.
//!
//! Cases:
//! - `mutation_spawn_uses_per_cell_mailbox_size_override` — `add_nodes` of a
//!   template carrying `cell.mailbox_size: 12` → factory sees `mailbox_capacity == 12`.
//! - `mutation_spawn_zero_mailbox_size_rejected_pre_destructive` — `add_nodes`
//!   with `cell.mailbox_size: 0` → mutation `Rejected` (error_code "schema"),
//!   registry unchanged, NO `.staging` remnant, NO live cell directory created.

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

/// Build the closure that constructs a fresh long-running cell-task. Shared so
/// the recorder only differs in WHEN/WHAT it captures.
fn make_build(
    path: Path,
    outputs_tx: mpsc::Sender<CellEmission>,
    colony_inbox_tx: mpsc::Sender<ColonyMsg>,
    mailbox_capacity: usize,
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

async fn read_registry(h: &ColonyHandle) -> meclaw_colony::api_dto::ReadRegistryReply {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 1000,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap()
}

/// Records the `mailbox_capacity` the spawn path handed the factory (-1 = never).
struct MailboxRecorderFactory {
    last_capacity: Arc<AtomicI64>,
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
        _contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<std::time::Duration>,
        _blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        self.last_capacity
            .store(mailbox_capacity as i64, Ordering::SeqCst);
        let build = make_build(path, outputs_tx, colony_inbox_tx, mailbox_capacity);
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
async fn mutation_spawn_uses_per_cell_mailbox_size_override() {
    let td = tempfile::TempDir::new().unwrap();
    // colony.json default (≠ 12) so the assert proves the override, not the default.
    write(
        td.path(),
        "colony.json",
        r#"{"mailbox_default_capacity": 500}"#,
    );
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );

    // Template carries the per-cell mailbox_size override.
    let tpl_dir = td.path().join("templates").join("recorder");
    std::fs::create_dir_all(&tpl_dir).unwrap();
    std::fs::write(tpl_dir.join("template.json"), r#"{"name":"recorder"}"#).unwrap();
    std::fs::write(
        tpl_dir.join("config.json"),
        r#"{"cell":{"type":"recorder","mailbox_size":12},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    let last_capacity = Arc::new(AtomicI64::new(-1));
    let factory: Arc<dyn CellFactory> = Arc::new(MailboxRecorderFactory {
        last_capacity: last_capacity.clone(),
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
        last_capacity.load(Ordering::SeqCst),
        12,
        "mutation-spawn must hand the factory mailbox_capacity = cell.mailbox_size override (12), \
         NOT the colony.json default (500)"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutation_spawn_zero_mailbox_size_rejected_pre_destructive() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );

    // Template carries an illegal mailbox_size: 0.
    let tpl_dir = td.path().join("templates").join("recorder");
    std::fs::create_dir_all(&tpl_dir).unwrap();
    std::fs::write(tpl_dir.join("template.json"), r#"{"name":"recorder"}"#).unwrap();
    std::fs::write(
        tpl_dir.join("config.json"),
        r#"{"cell":{"type":"recorder","mailbox_size":0},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    let last_capacity = Arc::new(AtomicI64::new(-1));
    let factory: Arc<dyn CellFactory> = Arc::new(MailboxRecorderFactory {
        last_capacity: last_capacity.clone(),
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
    match outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(
                error_code, "schema",
                "mailbox_size:0 must be rejected with error_code 'schema'"
            );
        }
        other => panic!("expected Rejected for mailbox_size:0, got {other:?}"),
    }

    // No spawn happened (pre-destructive reject during staging).
    assert_eq!(
        last_capacity.load(Ordering::SeqCst),
        -1,
        "no cell must have been spawned for a rejected mutation"
    );

    // Registry unchanged: the rejected cell is NOT present.
    let reg = read_registry(&h).await;
    assert!(
        !reg.entries.iter().any(|e| e.path == "/rec"),
        "rejected cell must NOT be in the registry; entries: {:?}",
        reg.entries
            .iter()
            .map(|e| e.path.clone())
            .collect::<Vec<_>>()
    );

    // No live cell directory was created (no atomic rename happened).
    assert!(
        !td.path().join("main").join("rec").exists(),
        "no live cell directory may be created for a pre-destructive reject"
    );

    // No `.staging` remnant survives (staging tree discarded on reject).
    let staging_root = td.path().join(".staging");
    if staging_root.exists() {
        let leftover: Vec<_> = std::fs::read_dir(&staging_root)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .filter(|e| {
                // A staging mutation dir that still contains the `rec` cell dir.
                e.path().join("rec").exists()
            })
            .collect();
        assert!(
            leftover.is_empty(),
            "no `.staging/<id>/rec` remnant may survive a reject"
        );
    }

    h.shutdown().await;
}
