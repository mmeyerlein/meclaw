//! Paket-7 schema-load demo (e) — `contract.emits` is compiled on BOTH the
//! boot path and the mutation path, and a malformed schema fails LOUDLY at
//! either entry (never a silent "validation off").
//!
//! Anchor: Boot-Strict-Kultur (config.md Z.37). The shared
//! `compile_contract_view` helper backs both paths.
//!
//! Tests in this file:
//!   - B4 (pre-existing): a MUTATION instantiating a cell with a malformed
//!     `contract.emits` is rejected PRE-DESTRUCTIVELY during staging
//!     (`MutationError::Schema`, no live FS change, no orphan rename, no
//!     `.staging` remnant). Mirrors the `mailbox_size:0` reject pattern.
//!   - F2 boot-good: BOOT of a topology with a WELL-FORMED `contract.emits`
//!     code cell → `bootstrap_from_filesystem` succeeds, the cell is spawned
//!     and registered (validator active in debug builds).
//!   - F2 boot-bad: BOOT with a MALFORMED `contract.emits` → boot fails LOUDLY
//!     (`Err(BootstrapErrors)` carrying `BootstrapError::InvalidEmitsSchema`),
//!     NOT a silent start; no cell is spawned.

use meclaw_colony::{
    BootstrapError, CellFactory, CellFactoryRegistry, ColonyMsg, DbConn, MutationOutcome,
    RespawnFn, SpawnedCellKind, cell_task_long_running,
};
use meclaw_core::{CellEmission, JsonValue, Message, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::bootstrap_apply::bootstrap_from_filesystem;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

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
        let (tx, rx) = mpsc::channel::<Message>(mailbox_capacity.max(1));
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

/// Records whether `spawn_cell` was ever reached (-1 = never).
struct SpawnRecorderFactory {
    last_capacity: Arc<AtomicI64>,
}

impl CellFactory for SpawnRecorderFactory {
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
async fn mutation_with_malformed_contract_emits_rejected_pre_destructive() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );

    // Template carries a code cell whose `contract.emits` has an invalid
    // JSON-Schema type token ("stringg") → `CompiledEmits::compile` fails.
    let tpl_dir = td.path().join("templates").join("badcode");
    std::fs::create_dir_all(&tpl_dir).unwrap();
    std::fs::write(tpl_dir.join("template.json"), r#"{"name":"badcode"}"#).unwrap();
    std::fs::write(
        tpl_dir.join("config.json"),
        r#"{"cell":{"type":"code"},"params":{"runner":"python3"},
           "contract":{"version":"0.1.0","settings":{},"consumes":{},"emits":{"body":{"x":{"type":"stringg"}},"hop":{}}}}"#,
    )
    .unwrap();

    let last_capacity = Arc::new(AtomicI64::new(-1));
    let factory: Arc<dyn CellFactory> = Arc::new(SpawnRecorderFactory {
        last_capacity: last_capacity.clone(),
    });
    let h = ColonyHandle::new_with_factories_at(&td, vec![("code".to_string(), factory)]);
    rescan_templates(&h, td.path().join("templates")).await;

    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name":"bad","template":"badcode"}]}
        }),
    )
    .await;
    match outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(
                error_code, "schema",
                "malformed contract.emits must be rejected with error_code 'schema'"
            );
        }
        other => panic!("expected Rejected for malformed contract.emits, got {other:?}"),
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
        !reg.entries.iter().any(|e| e.path == "/bad"),
        "rejected cell must NOT be in the registry; entries: {:?}",
        reg.entries
            .iter()
            .map(|e| e.path.clone())
            .collect::<Vec<_>>()
    );

    // No live cell directory was created (no atomic rename happened).
    assert!(
        !td.path().join("main").join("bad").exists(),
        "no live cell directory may be created for a pre-destructive reject"
    );

    // No `.staging` remnant survives (staging tree discarded on reject).
    let staging_root = td.path().join(".staging");
    if staging_root.exists() {
        let leftover: Vec<_> = std::fs::read_dir(&staging_root)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .filter(|e| e.path().join("bad").exists())
            .collect();
        assert!(
            leftover.is_empty(),
            "no `.staging/<id>/bad` remnant may survive a reject"
        );
    }

    h.shutdown().await;
}

// ── F2: boot path — well-formed compiles + starts; malformed rejects loudly ──

/// Write a root hive + a single `code` cell carrying `contract.emits = emits`.
fn boot_tree_with_code_emits(td: &tempfile::TempDir, emits: &str) {
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/coder/config.json",
        &format!(
            r#"{{"cell":{{"type":"code"}},"params":{{"runner":"python3"}},
               "contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}},"emits":{emits}}}}}"#
        ),
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn boot_with_well_formed_contract_emits_starts_and_validates() {
    // Boot path (B3): a well-formed `contract.emits` compiles and the code cell
    // spawns. The SpawnRecorderFactory records that `spawn_cell` was reached
    // (last_capacity != -1) — proof the cell actually started with a compiled
    // validator (debug-build → validate_emits resolves true).
    let td = tempfile::TempDir::new().unwrap();
    boot_tree_with_code_emits(
        &td,
        r#"{"body":{"messages":{"type":"array"}},"hop":{"finish_reason":{"type":"string"}}}"#,
    );

    let last_capacity = Arc::new(AtomicI64::new(-1));
    let factory: Arc<dyn CellFactory> = Arc::new(SpawnRecorderFactory {
        last_capacity: last_capacity.clone(),
    });
    let h = ColonyHandle::new_with_factories_at(&td, vec![("code".to_string(), factory.clone())]);

    let mut registry = CellFactoryRegistry::new();
    registry.insert("code".to_string(), factory);
    let report = bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("boot with well-formed contract.emits must succeed");
    assert_eq!(report.cell_count, 1, "the code cell bootstrapped");

    // The cell was actually spawned (validator wired, not a silent skip).
    assert_ne!(
        last_capacity.load(Ordering::SeqCst),
        -1,
        "well-formed boot must reach spawn_cell for the code cell"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn boot_with_malformed_contract_emits_fails_loudly_no_spawn() {
    // Boot path (B3): a malformed `contract.emits` (invalid JSON-Schema type
    // token "stringg") must surface as a LOUD BootstrapError::InvalidEmitsSchema
    // — boot returns Err, the cell is NOT silently started.
    let td = tempfile::TempDir::new().unwrap();
    boot_tree_with_code_emits(&td, r#"{"body":{"x":{"type":"stringg"}},"hop":{}}"#);

    let last_capacity = Arc::new(AtomicI64::new(-1));
    let factory: Arc<dyn CellFactory> = Arc::new(SpawnRecorderFactory {
        last_capacity: last_capacity.clone(),
    });
    let h = ColonyHandle::new_with_factories_at(&td, vec![("code".to_string(), factory.clone())]);

    let mut registry = CellFactoryRegistry::new();
    registry.insert("code".to_string(), factory);
    let errs = bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect_err("boot with malformed contract.emits must FAIL loudly");

    // The error is specifically an emits-schema failure naming the bad section.
    assert!(
        errs.items().iter().any(|e| matches!(
            e,
            BootstrapError::InvalidEmitsSchema { reason, .. } if reason.contains("emits.body")
        )),
        "boot error list must contain InvalidEmitsSchema naming emits.body; got: {:?}",
        errs.items()
    );

    // Atomic-strict boot: nothing was spawned (reject happens in plan_bootstrap,
    // before any apply/spawn).
    assert_eq!(
        last_capacity.load(Ordering::SeqCst),
        -1,
        "malformed boot must NOT reach spawn_cell — no silent start"
    );

    h.shutdown().await;
}
