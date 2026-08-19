//! Phase-13.5 Lifecycle-3b Task 7.5: disconnect of a WOKEN stateful cell.
//!
//! Spec gap closed here (overview Z.1434 "Task ⇔ aktiv", Z.1448 cell_inactive
//! remainder): a stateful cell spawned lazily via wake-pre-send (status `Awake`)
//! previously had NO live `stop_tx`/`death_ack_rx` in its `RegistryEntry` — the
//! `WakeFn` discarded the live stop pair from `build_stateful_task_with_peace`.
//! So disconnecting a woken stateful cell fell through to "just flip
//! active=false": the task kept running while marked inactive AND its mailbox
//! remainder was never drained to the DLQ.
//!
//! This test wakes a stateful cell, queues unread messages behind a blocked
//! `handle()`, then disconnects it. Assertions: the task is peace-stopped (no
//! `CellDied`/restart), `active=false` + persisted `'inactive'`, the mailbox
//! remainder → DLQ `cell_inactive` (count + order), and the registry entry
//! stays with an unchanged `cell_id`.

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, DbConn, MutationOutcome, RespawnFn,
    SpawnedCellKind, StatefulCell, bootstrap_from_filesystem, build_stateful_task_with_peace,
    set_term_timeout_ms_for_test, spawn_watcher,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, JsonValue, Message, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};

// ───────────────────────────────────────────────────────────────────────────
// GateStatefulCell: a stateful cell whose first `handle()` blocks on a barrier
// oneshot until the test releases it. Subsequent messages sit in the mailbox
// while the cell is wedged. The cell never emits and never touches `cell.db`
// beyond opening it — it only needs to be a real stateful (Dormant) cell with
// live peace-stop wiring through `build_stateful_task_with_peace`.
// ───────────────────────────────────────────────────────────────────────────

struct GateStatefulCell {
    /// Fired once when `handle()` is first entered (single-use).
    entered: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    /// Awaited on the first `handle()` to block until released (single-use).
    gate: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
}

impl StatefulCell for GateStatefulCell {
    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a mut self,
        _msg: Message,
        _sink: &'a OutputSink,
        _db: &'a mut DbConn,
    ) -> impl std::future::Future<Output = ()> + Send + 'a {
        async move {
            if let Some(tx) = self.entered.lock().expect("entered lock").take() {
                let _ = tx.send(());
            }
            // Take the receiver and DROP the guard before awaiting (a held
            // MutexGuard across `.await` would make the future `!Send`).
            let maybe_rx = self.gate.lock().expect("gate lock").take();
            if let Some(rx) = maybe_rx {
                // Block until released; the remaining mailbox messages stay
                // queued while this first `handle()` is in flight.
                let _ = rx.await;
            }
        }
    }
}

/// Stateful Dormant factory for `GateStatefulCell`. The `WakeFn` spawns the
/// task via the shared helper and RETURNS the live `(stop_tx, death_ack_rx)`
/// (Task 7.5) so the colony stores it for disconnect.
struct GateStatefulFactory {
    entered: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    gate: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
}

impl CellFactory for GateStatefulFactory {
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
        idle_timeout: Option<Duration>,
        cell_timeout: i64,
        message_timeout: Option<std::time::Duration>,
        _blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        _mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let (sender, receiver) = mpsc::channel::<Message>(1000);

        let wake_path = path.clone();
        let wake_outputs = outputs_tx.clone();
        let wake_inbox = colony_inbox_tx.clone();
        let wake_watcher_inbox = colony_inbox_tx.clone();
        let wake_cell_dir = cell_dir.clone();
        let entered = self.entered.clone();
        let gate = self.gate.clone();
        let wake: meclaw_colony::WakeFn = Box::new(move |recv: mpsc::Receiver<Message>| {
            let conn =
                meclaw_colony::persist::open_or_create_cell_db(&wake_cell_dir.join("cell.db"))
                    .expect("wake: open_or_create_cell_db");
            let db = DbConn::wrap(conn, None);
            let cell = GateStatefulCell {
                entered: entered.clone(),
                gate: gate.clone(),
            };
            let (join, peace_rx, stop_tx, death_ack_rx, backstop_rx) =
                build_stateful_task_with_peace(
                    wake_path.clone(),
                    recv,
                    wake_outputs.clone(),
                    wake_inbox.clone(),
                    idle_timeout,
                    message_timeout,
                    cell_timeout,
                    cell,
                    db,
                    None,
                    None,
                    Default::default(),
                );
            spawn_watcher(
                &wake_watcher_inbox,
                wake_path.clone(),
                join,
                peace_rx,
                backstop_rx,
            );
            (stop_tx, death_ack_rx)
        });

        // Pre-wake placeholder pair (inert; replaced by the WakeFn's live pair).
        let (stop_tx, _stop_rx) = oneshot::channel::<()>();
        let (_death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
        let respawn: RespawnFn = Box::new(|| unreachable!("GateStateful no restart in this test"));
        Ok(SpawnedCellKind::Dormant {
            sender,
            receiver,
            wake,
            stop_tx,
            death_ack_rx,
            respawn,
        })
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Helpers (mirrored from phase_13_5_lifecycle_3b_disconnect.rs).
// ───────────────────────────────────────────────────────────────────────────

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

/// Root hive + one echo cell `/peer` (a live endpoint for the gating edge).
fn write_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main/peer")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/peer/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/peer"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

async fn send_mutation(h: &ColonyHandle, payload: Value) -> MutationOutcome {
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

async fn ram_entry(
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

/// Build a probe message tagged with a `correlation_id` for DLQ-order checks.
fn probe(target: &str, corr: Uuid) -> Message {
    MessageBuilder::new(Path::new(target))
        .correlation_id(corr)
        .body(Body::Inline(json!({"messages": []})))
        .build()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disconnect_woken_stateful_drains_remainder_to_dlq() {
    // Generous death-ack term-timeout (30 s): the disconnect peace-stops a woken
    // Awake stateful cell; under load a slow death-ack must not spuriously fire
    // `term_timeout`.
    set_term_timeout_ms_for_test(30_000);
    let td = TempDir::new().unwrap();
    write_topology(td.path());
    let db_dir = td.path().to_path_buf();

    let entered_tx_slot: Arc<Mutex<Option<oneshot::Sender<()>>>> = Arc::new(Mutex::new(None));
    let (entered_tx, entered_rx) = oneshot::channel::<()>();
    *entered_tx_slot.lock().unwrap() = Some(entered_tx);

    let (gate_tx, gate_rx) = oneshot::channel::<()>();
    let gate_slot: Arc<Mutex<Option<oneshot::Receiver<()>>>> = Arc::new(Mutex::new(Some(gate_rx)));

    let sf_factory: Arc<dyn CellFactory> = Arc::new(GateStatefulFactory {
        entered: entered_tx_slot.clone(),
        gate: gate_slot.clone(),
    });

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

    // Register the stateful cell /sf (Dormant) via its factory.
    let sf_dir = td.path().join("main/sf");
    std::fs::create_dir_all(&sf_dir).unwrap();
    let sf_spawned = sf_factory
        .spawn_cell(
            Path::new("/sf"),
            json!({}),
            h.runtime().outputs_tx,
            sf_dir.clone(),
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            Some(Duration::from_millis(60_000)),
            0,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned(Path::new("/sf"), sf_spawned).await;

    let sf0 = ram_entry(&h, "/sf").await.expect("/sf registered");
    assert_eq!(sf0.lifecycle_status, "NotYetSpawned");
    let sf_cell_id = sf0.cell_id;

    // Connect /sf -> /peer so /sf is active (still NotYetSpawned, no task yet).
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"sf","to":"peer"}]}}),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));
    assert!(ram_entry(&h, "/sf").await.expect("/sf").active);

    // WAKE /sf: route msg1 → wake-pre-send spawns the task (status → Awake) and
    // stores the live stop pair. msg1's `handle()` blocks on the gate.
    let corr1 = Uuid::now_v7();
    h.send(probe("/sf", corr1)).await;
    entered_rx
        .await
        .expect("GateStatefulCell must enter handle()");
    assert_eq!(
        ram_entry(&h, "/sf").await.expect("/sf").lifecycle_status,
        "Awake",
        "/sf must be Awake after wake-pre-send"
    );

    // Queue two more messages behind the blocked handle() — these are the
    // mailbox remainder that must be drained to the DLQ on disconnect.
    let corr2 = Uuid::now_v7();
    let corr3 = Uuid::now_v7();
    h.send(probe("/sf", corr2)).await;
    h.send(probe("/sf", corr3)).await;
    // Give the messages a beat to land in the cell's mailbox.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Disconnect /sf (remove its last edge) CONCURRENTLY: the recompute fires
    // the peace-stop and then awaits death_ack. The cell is wedged in handle(),
    // so we release the gate so the in-flight handle() returns → the biased
    // stop arm fires → mailbox remainder is returned via ColonyMsg::Stopped.
    let inbox_tx = h.inbox_tx.clone();
    let mutation = tokio::spawn(async move {
        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Mutation {
                payload: json!({"scope":"/","diff":{"remove_edges":[{"match":{"from":"sf","to":"peer"}}]}}),
                reply_to: None,
                trace_id: Uuid::now_v7(),
                parent_message_id: Uuid::now_v7(),
                ack: ack_tx,
            })
            .await
            .unwrap();
        ack_rx.await.unwrap()
    });
    // Release the gate so handle()#1 returns and the stop can complete.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let _ = gate_tx.send(());

    let outcome = mutation.await.expect("mutation task");
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "disconnect must commit (death-ack fires < TERM_TIMEOUT), got {outcome:?}"
    );

    // Registry entry STAYS, active == false, cell_id unchanged.
    let sf1 = ram_entry(&h, "/sf").await.expect("/sf entry must STAY");
    assert!(!sf1.active, "/sf must be inactive after disconnect");
    assert_eq!(
        sf1.cell_id, sf_cell_id,
        "cell_id unchanged across disconnect"
    );

    // Mailbox remainder drained to DLQ as cell_inactive, in order (msg2, msg3).
    let dlq = h.drain_dead_letters().await;
    let inactive: Vec<&meclaw_colony::DeadLetter> = dlq
        .iter()
        .filter(|d| {
            matches!(
                d.reason,
                meclaw_colony::dead_letter::DeadLetterReason::CellInactive
            ) && d.resolved_target == Path::new("/sf")
        })
        .collect();
    assert_eq!(
        inactive.len(),
        2,
        "exactly the two queued remainder messages must hit the DLQ as cell_inactive, got {inactive:?}"
    );
    assert_eq!(
        inactive[0].message.correlation_id,
        Some(corr2),
        "DLQ order: first drained message must be msg2"
    );
    assert_eq!(
        inactive[1].message.correlation_id,
        Some(corr3),
        "DLQ order: second drained message must be msg3"
    );

    h.shutdown().await;

    // Persisted status == "inactive".
    assert_eq!(
        db_registry_status(&db_dir, "/sf").as_deref(),
        Some("inactive"),
        "colony.db.registry.status must be persisted as inactive"
    );
}
