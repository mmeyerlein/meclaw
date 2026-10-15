//! Paket-1 (c) → P3-B-plumb-2 — `cell.message_timeout` is boot-safe (the cell
//! boots green).
//!
//! `plan_bootstrap` accepts `cell.message_timeout` (it is on the cell-field
//! allow-list) and — since P3-B-plumb-2 — propagates it into
//! `PlannedCell.message_timeout`, which `bootstrap_apply` resolves into the
//! active B-backstop at the spawn call-site. The former deferred-enforcement
//! `tracing::warn` ("parsed but not yet wired") was therefore RETIRED: the field
//! IS now wired, so no such warn fires. This test now proves the boot-safe half
//! only — bootstrap succeeds and the cell is registered (a recorder factory's
//! `spawn_cell` runs → the cell exists). Propagation/resolution wiring is covered
//! by the `bootstrap.rs` / `stage.rs` unit tests.

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ContractView, DbConn, RespawnFn, SpawnedCellKind,
    cell_task_long_running,
};
use meclaw_core::{CellEmission, JsonValue, Message, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::bootstrap_apply::bootstrap_from_filesystem;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use tracing::field::{Field, Visit};
use tracing::subscriber::Subscriber;
use tracing::{Event, Level, Metadata, span};

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// Minimal WARN-only subscriber that records each WARN event's rendered
/// `message` field. Adapted from `colony_config.rs::WarnCapture` — same shape,
/// but it captures the `message` field (the warn here carries a `path` field
/// plus a literal message string, not a `field` field).
struct WarnCapture {
    messages: Arc<Mutex<Vec<String>>>,
}

impl Subscriber for WarnCapture {
    fn enabled(&self, meta: &Metadata<'_>) -> bool {
        *meta.level() == Level::WARN
    }
    fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
        span::Id::from_u64(1)
    }
    fn record(&self, _: &span::Id, _: &span::Record<'_>) {}
    fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}
    fn event(&self, event: &Event<'_>) {
        let mut visitor = MessageVisitor { captured: None };
        event.record(&mut visitor);
        if let Some(m) = visitor.captured {
            self.messages.lock().unwrap().push(m);
        }
    }
    fn enter(&self, _: &span::Id) {}
    fn exit(&self, _: &span::Id) {}
}

struct MessageVisitor {
    captured: Option<String>,
}

impl Visit for MessageVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.captured = Some(value.to_owned());
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        // `tracing::warn!("literal")` renders the message via the Debug path.
        if field.name() == "message" {
            self.captured = Some(format!("{value:?}"));
        }
    }
}

/// Recorder factory: flips a flag when `spawn_cell` runs, so the test has a
/// positive boot receipt (the cell genuinely registered).
struct SpawnRecorderFactory {
    spawned: Arc<AtomicBool>,
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
        _contract: ContractView,
        colony_inbox_tx: mpsc::Sender<meclaw_colony::ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<std::time::Duration>,
        _blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        self.spawned.store(true, Ordering::SeqCst);
        let build = move || -> (
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
                 None,
                 Default::default(),
                 )
                .await;
            });
            (tx, join, peace_rx, backstop_rx)
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
async fn message_timeout_boots_green_and_emits_deferred_warn() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    // The cell under test carries cell.message_timeout: 5000.
    write(
        td.path(),
        "main/c/config.json",
        r#"{"cell":{"type":"recorder","message_timeout":5000},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let spawned = Arc::new(AtomicBool::new(false));
    let factory: Arc<dyn CellFactory> = Arc::new(SpawnRecorderFactory {
        spawned: spawned.clone(),
    });

    let messages = Arc::new(Mutex::new(Vec::<String>::new()));

    // Run the whole bootstrap under the WARN-capturing subscriber. We assert the
    // RETIRED deferred-warn does NOT fire (the field is now wired), and capture
    // any unexpected WARN for diagnostics.
    {
        let h = ColonyHandle::new_with_factories_at(
            &td,
            vec![("recorder".to_string(), factory.clone())],
        );

        let mut registry = CellFactoryRegistry::new();
        registry.insert("recorder".to_string(), factory);

        let subscriber = WarnCapture {
            messages: messages.clone(),
        };
        let report = {
            let _guard = tracing::subscriber::set_default(subscriber);
            bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
                .await
                .expect("bootstrap with cell.message_timeout must succeed (boot-safe)")
        };

        // Positive boot receipt: the cell was planned AND spawned.
        assert_eq!(report.cell_count, 1, "the recorder cell must boot");
        assert!(
            spawned.load(Ordering::SeqCst),
            "the recorder factory's spawn_cell must have run (cell is live)"
        );

        h.shutdown().await;
    }

    // P3-B-plumb-2: the field is now wired → the former "not yet wired" warn must
    // NOT fire any more.
    let msgs = messages.lock().unwrap();
    assert!(
        !msgs
            .iter()
            .any(|m| m.contains("message_timeout") && m.contains("not yet wired")),
        "the retired deferred-warn must not fire once message_timeout is wired; captured: {msgs:?}"
    );
}
