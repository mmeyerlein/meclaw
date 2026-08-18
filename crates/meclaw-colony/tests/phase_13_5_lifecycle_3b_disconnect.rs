//! Phase-13.5 Lifecycle-3b Task 4 demo (a): Disconnect wiring in
//! `handle_mutation` — end-to-end via a real stateless cell.
//!
//! `remove_edges` removes the last edge of a cell → the connectivity-recompute
//! deactivates it. The cell-task ends graceful (no `CellDied`/restart), the
//! registry entry STAYS (`active == false`), and the mutation commits because
//! the cell's `death_ack` fires well under `TERM_TIMEOUT`. The persisted
//! `colony.db.registry.status == "inactive"` is read back via a FRESH read-only
//! connection (not the live writer).

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, DbConn, LongRunningCell, MutationOutcome,
    SpawnedCellKind, bootstrap_from_filesystem, cell_task_long_running,
    set_term_timeout_ms_for_test,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{
    Body, CellEmission, JsonValue, Message, MessageBuilder, OriginSink, OutputSink, Path, Uuid,
};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::future::Future;
use std::sync::Arc;
use std::sync::Mutex;
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};

/// Serialize the two term-timeout-sensitive tests in this binary. They drive the
/// process-global `set_term_timeout_ms_for_test` to OPPOSITE values — the happy
/// path wants a generous budget so a slow death-ack does not spuriously reject,
/// the wedged-cell path wants a short budget so the timeout fires fast and
/// deterministically. Running them in parallel would race the shared static.
/// Held across `.await`, so `tokio::sync::Mutex` (yields the task, never blocks a
/// worker thread).
static TERM_TIMEOUT_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn echo_factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "echo".to_string(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    )]
}

fn echo_registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert(
        "echo".into(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    );
    r
}

/// Root hive + echo cells `/a` and `/anchor`. `/anchor` exists only to keep the
/// `/sink` CaptureCell connected via a persistent edge that the test never
/// removes, so that disconnecting `/a` does NOT also strand `/sink` (an Awake
/// test sink with no stop-wiring → would otherwise trip the Step-10 interim
/// stop-wiring guard).
fn write_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main/a")).unwrap();
    std::fs::create_dir_all(td.join("main/anchor")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/a/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/a"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/anchor/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/anchor"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

async fn send_mutation(h: &ColonyHandle, payload: Value) -> MutationOutcome {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
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

/// Read the RAM registry entry DTO for `path` (or `None` if absent).
async fn ram_entry(
    h: &ColonyHandle,
    path: &str,
) -> Option<meclaw_colony::api_dto::RegistryEntryDto> {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel::<ReadRegistryReply>();
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

/// Fresh read-only connection probe: persisted `registry.status` for `path`.
fn db_registry_status(db_dir: &std::path::Path, path: &str) -> Option<String> {
    let conn = rusqlite::Connection::open_with_flags(
        db_dir.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open colony.db read-only");
    conn.query_row("SELECT status FROM registry WHERE path = ?1", [path], |r| {
        r.get::<_, String>(0)
    })
    .ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remove_last_edge_disconnects_cell_active_false_persisted() {
    let _guard = TERM_TIMEOUT_TEST_LOCK.lock().await;
    // Happy path: generous budget (30 s) so a load-slow death-ack never
    // spuriously fires `term_timeout`.
    set_term_timeout_ms_for_test(30_000);
    let td = TempDir::new().unwrap();
    write_topology(td.path());
    let db_dir = td.path().to_path_buf();

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    // /sink BEFORE bootstrap (anti-cascade).
    let (sink_tx, _sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap");

    // Add /a -> /sink so /a is connected → active, plus a persistent
    // /anchor -> /sink edge that keeps /sink connected after /a's edge is
    // removed (so the disconnect only deactivates /a, not the no-stop-wiring
    // /sink sink — which would otherwise trip the Step-10 stop-wiring guard).
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[
            {"from":"a","to":"sink"},
            {"from":"anchor","to":"sink"}
        ]}}),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));
    let a = ram_entry(&h, "/a").await.expect("/a registered");
    assert!(a.active, "/a must be active after add_edges");

    // Remove the last edge of /a → recompute deactivates it.
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"remove_edges":[{"match":{"from":"a","to":"sink"}}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "remove_edges must commit (death-ack fires < TERM_TIMEOUT), got {outcome:?}"
    );

    // Registry entry STAYS, active == false.
    let a = ram_entry(&h, "/a")
        .await
        .expect("/a entry must STAY per No-Delete");
    assert!(
        !a.active,
        "/a must be inactive after its last edge is removed"
    );

    h.shutdown().await;

    // Persisted status == "inactive" (fresh read-only connection).
    assert_eq!(
        db_registry_status(&db_dir, "/a").as_deref(),
        Some("inactive"),
        "colony.db.registry.status must be persisted as inactive"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// 4.5 — F5-Variante-A: a backpressured cell (stuck mid-`handle()`) cannot fire
// `death_ack` within `TERM_TIMEOUT` → the disconnect mutation is Rejected with
// `error_code == "term_timeout"` and FULLY rolled back.
//
// The cell hangs deterministically: on its first mailbox message it signals
// "entered handle()" via a barrier oneshot, then `pending().await`s forever
// (a stand-in for a `handle()` blocked on a full `outputs_tx`). The biased stop
// arm only re-evaluates after the in-flight `handle()` returns — which never
// happens — so the task never tears down and `death_ack` never fires.
//
// This test waits one term-timeout budget once. It overrides the budget to a
// short 1 s (via `set_term_timeout_ms_for_test`) so the wait is fast and
// deterministic; the outer failure marker is generous (30 s).
// ───────────────────────────────────────────────────────────────────────────

/// Long-running cell that hangs forever inside `handle()` after signalling.
struct HangCell {
    /// Fired once when `handle()` is first entered (single-use).
    entered: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

impl LongRunningCell for HangCell {
    type Event = ();
    type Reconfig = ();
    type Io = ();

    fn split_io(&mut self) -> Self::Io {}

    #[allow(clippy::manual_async_fn)]
    fn run_io(
        io: Self::Io,
        events_tx: mpsc::Sender<Self::Event>,
        reconfig_rx: mpsc::Receiver<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send {
        async move {
            // Keep the channels alive so the handler_loop does not exit on a
            // closed events channel; block forever like a real I/O sub-task.
            let _keep = (io, events_tx, reconfig_rx);
            std::future::pending::<()>().await
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
        async move {
            if let Some(tx) = self.entered.lock().expect("entered lock").take() {
                let _ = tx.send(());
            }
            // Block forever — stands in for a cell wedged on a full outputs_tx.
            std::future::pending::<()>().await
        }
    }

    #[allow(clippy::manual_async_fn)]
    fn handle_event<'a>(
        &'a mut self,
        _event: Self::Event,
        _sink: &'a OriginSink,
        _db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async {}
    }
}

/// Factory that spawns a single live `HangCell` with real stop/death_ack wiring.
struct HangCellFactory {
    entered: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

impl CellFactory for HangCellFactory {
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
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let (death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
        let (peace_tx, peace_rx) = oneshot::channel::<()>();
        let (_backstop_tx, backstop_rx) = oneshot::channel::<()>();
        let (tx, rx) = mpsc::channel::<Message>(1000);
        let cell = HangCell {
            entered: self.entered.clone(),
        };
        let conn = rusqlite::Connection::open_in_memory().expect("open_in_memory");
        let db = DbConn::wrap(conn, None);
        let join = tokio::spawn(async move {
            cell_task_long_running(
                path,
                rx,
                outputs_tx,
                64,
                cell,
                db,
                Some(peace_tx),
                Some(colony_inbox_tx),
                Some(stop_rx),
                Some(death_ack_tx),
                None,
                None,
            )
            .await;
        });
        // Single-spawn factory (no restart in this test) → respawn unreachable.
        let respawn: meclaw_colony::RespawnFn = Box::new(|| unreachable!("HangCell no restart"));
        Ok(SpawnedCellKind::Active {
            sender: tx,
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
async fn disconnect_term_timeout_rejects_and_rolls_back() {
    let _guard = TERM_TIMEOUT_TEST_LOCK.lock().await;
    // Wedged-cell path: the death-ack NEVER fires, so the test depends on the
    // budget elapsing. A short override (1 s) makes that fast and deterministic
    // instead of waiting the 5 s production default. The outer failure marker
    // (30 s) stays generous.
    set_term_timeout_ms_for_test(1_000);
    let td = TempDir::new().unwrap();
    write_topology(td.path());
    let db_dir = td.path().to_path_buf();

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap");

    // Register a HangCell at /hang via its factory (live stop/death_ack wiring).
    let (entered_tx, entered_rx) = oneshot::channel::<()>();
    let factory = Arc::new(HangCellFactory {
        entered: Arc::new(Mutex::new(Some(entered_tx))),
    });
    let spawned = factory
        .spawn_cell(
            Path::new("/hang"),
            json!({}),
            h.runtime().outputs_tx,
            td.path().join("hang"),
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            0,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned(Path::new("/hang"), spawned).await;

    // Connect /hang via an edge /hang -> /a so it is active.
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"hang","to":"a"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "add_edges hang->a must commit, got {outcome:?}"
    );
    assert!(ram_entry(&h, "/hang").await.expect("/hang").active);
    let edges_before = db_edge_count(&db_dir, "/hang", "/a");
    assert_eq!(edges_before, 1, "edge must be persisted before reject");

    // Wedge /hang inside handle() so its stop cannot complete.
    h.send(
        MessageBuilder::new(Path::new("/hang"))
            .body(Body::Inline(json!({"messages":[]})))
            .build(),
    )
    .await;
    entered_rx.await.expect("HangCell must enter handle()");

    // Remove the edge → recompute deactivates /hang → fires stop → death_ack
    // never arrives → TERM_TIMEOUT → Rejected{term_timeout} + full rollback.
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"remove_edges":[{"match":{"from":"hang","to":"a"}}]}}),
    )
    .await;
    match outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(error_code, "term_timeout", "must reject with term_timeout");
        }
        other => panic!("expected Rejected{{term_timeout}}, got {other:?}"),
    }

    // (a) in-RAM edge restored.
    assert!(
        ram_entry(&h, "/hang").await.is_some(),
        "/hang entry must survive"
    );
    // (b) target node still active (no flip persisted).
    assert!(
        ram_entry(&h, "/hang").await.expect("/hang").active,
        "/hang must stay active after rollback"
    );
    // (c) colony.db unchanged: edge still present, status still active.
    h.shutdown().await;
    assert_eq!(
        db_edge_count(&db_dir, "/hang", "/a"),
        1,
        "edge must remain in colony.db after term_timeout rollback"
    );
    assert_eq!(
        db_registry_status(&db_dir, "/hang").as_deref(),
        Some("active"),
        "colony.db status must stay active after rollback"
    );
}

/// Count edge rows in colony.db with the given from/to (fresh connection).
fn db_edge_count(db_dir: &std::path::Path, from: &str, to: &str) -> i64 {
    let conn = rusqlite::Connection::open_with_flags(
        db_dir.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open colony.db read-only");
    conn.query_row(
        "SELECT count(*) FROM edges WHERE from_path = ?1 AND to_path = ?2",
        [from, to],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0)
}
