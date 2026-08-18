//! Phase-13.5 Lifecycle-3b Task 9.3, demo (d) completion: disconnect-while-
//! busy → unread mailbox remainder → DLQ `cell_inactive`, for the two remaining
//! cell kinds with a REAL running task.
//!
//! The kind-agnostic unit contract (`handle_stopped` drains N → DLQ) and the
//! woken-STATEFUL E2E already exist. This file adds the two missing E2E proofs:
//! a real LONG-RUNNING cell, and a real STATELESS dispatcher cell —
//! each wedged inside `handle()` behind a barrier while extra messages queue in
//! its mailbox, then disconnected. On the colony-initiated peace-stop the busy
//! handle() finishes (gate released), the biased stop arm wins over a new recv,
//! the cell returns its mailbox `Receiver` via `ColonyMsg::Stopped`, and the
//! colony drains the remainder to the DLQ as `cell_inactive` (count + order).
//!
//! A deterministic remainder under real timing is achieved with a barrier
//! (gate) cell, mirroring the woken-stateful test — NOT a timing race.

use meclaw_colony::{
    CellFactory, ColonyMsg, DbConn, LongRunningCell, MutationOutcome, RespawnFn, SpawnedCellKind,
    StatelessCell, bootstrap_from_filesystem, cell_task_long_running, stateless_dispatcher,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{
    Body, CellEmission, JsonValue, Message, MessageBuilder, OriginSink, OutputSink, Path, Uuid,
};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use std::future::Future;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};

// ───────────────────────────────────────────────────────────────────────────
// Shared helpers.
// ───────────────────────────────────────────────────────────────────────────

fn echo_factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "echo".to_string(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    )]
}

fn echo_registry() -> meclaw_colony::CellFactoryRegistry {
    let mut r = meclaw_colony::CellFactoryRegistry::new();
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

/// Build a probe message tagged with a `correlation_id` for DLQ-order checks.
fn probe(target: &str, corr: Uuid) -> Message {
    MessageBuilder::new(Path::new(target))
        .correlation_id(corr)
        .body(Body::Inline(json!({"messages": []})))
        .build()
}

/// Collect the `cell_inactive` dead-letters resolved to `path`, in DLQ order.
async fn inactive_dlq_for(h: &ColonyHandle, path: &str) -> Vec<(Option<Uuid>,)> {
    let dlq = h.drain_dead_letters().await;
    dlq.iter()
        .filter(|d| {
            matches!(
                d.reason,
                meclaw_colony::dead_letter::DeadLetterReason::CellInactive
            ) && d.resolved_target == Path::new(path)
        })
        .map(|d| (d.message.correlation_id,))
        .collect()
}

// ───────────────────────────────────────────────────────────────────────────
// (d.1) Real LONG-RUNNING cell.
// ───────────────────────────────────────────────────────────────────────────

/// Long-running cell whose first `handle()` blocks on a gate oneshot. `run_io`
/// just parks (keeping its channels alive) so the handler loop never exits on a
/// closed events channel.
struct GateLrCell {
    entered: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    gate: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
}

impl LongRunningCell for GateLrCell {
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
            let maybe_rx = self.gate.lock().expect("gate lock").take();
            if let Some(rx) = maybe_rx {
                let _ = rx.await;
            }
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

struct GateLrFactory {
    entered: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    gate: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
}

impl CellFactory for GateLrFactory {
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
        _idle_timeout: Option<Duration>,
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
        let cell = GateLrCell {
            entered: self.entered.clone(),
            gate: self.gate.clone(),
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
        let respawn: RespawnFn = Box::new(|| unreachable!("GateLr no restart in this test"));
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

/// Demo (d), long-running part: a real long-running cell disconnected while busy
/// drains its unread mailbox remainder to the DLQ as `cell_inactive`, in order.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disconnect_long_running_drains_remainder_to_dlq() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());

    let entered_slot: Arc<Mutex<Option<oneshot::Sender<()>>>> = Arc::new(Mutex::new(None));
    let (entered_tx, entered_rx) = oneshot::channel::<()>();
    *entered_slot.lock().unwrap() = Some(entered_tx);
    let (gate_tx, gate_rx) = oneshot::channel::<()>();
    let gate_slot: Arc<Mutex<Option<oneshot::Receiver<()>>>> = Arc::new(Mutex::new(Some(gate_rx)));

    let lr_factory: Arc<dyn CellFactory> = Arc::new(GateLrFactory {
        entered: entered_slot.clone(),
        gate: gate_slot.clone(),
    });

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap");

    // Register the long-running cell /lr (Active, live stop wiring).
    let lr_dir = td.path().join("main/lr");
    std::fs::create_dir_all(&lr_dir).unwrap();
    let lr_spawned = lr_factory
        .spawn_cell(
            Path::new("/lr"),
            json!({}),
            h.runtime().outputs_tx,
            lr_dir,
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            -1,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned(Path::new("/lr"), lr_spawned).await;

    // Connect /lr -> /peer so /lr is active.
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"lr","to":"peer"}]}}),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));

    // msg1 wedges handle() on the gate; msg2/msg3 queue behind it.
    let corr1 = Uuid::now_v7();
    h.send(probe("/lr", corr1)).await;
    entered_rx.await.expect("/lr must enter handle()");
    let corr2 = Uuid::now_v7();
    let corr3 = Uuid::now_v7();
    h.send(probe("/lr", corr2)).await;
    h.send(probe("/lr", corr3)).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Disconnect concurrently; release the gate so handle()#1 returns → biased
    // stop wins → mailbox remainder returned via ColonyMsg::Stopped → DLQ.
    let inbox_tx = h.inbox_tx.clone();
    let mutation = tokio::spawn(async move {
        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Mutation {
                payload: json!({"scope":"/","diff":{"remove_edges":[{"match":{"from":"lr","to":"peer"}}]}}),
                reply_to: None,
                trace_id: Uuid::now_v7(),
                parent_message_id: Uuid::now_v7(),
                ack: ack_tx,
            })
            .await
            .unwrap();
        ack_rx.await.unwrap()
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let _ = gate_tx.send(());

    let outcome = mutation.await.expect("mutation task");
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "disconnect must commit (death-ack < TERM_TIMEOUT), got {outcome:?}"
    );

    let inactive = inactive_dlq_for(&h, "/lr").await;
    assert_eq!(
        inactive.len(),
        2,
        "the two queued remainder messages must hit the DLQ as cell_inactive, got {inactive:?}"
    );
    assert_eq!(inactive[0].0, Some(corr2), "DLQ order: msg2 first");
    assert_eq!(inactive[1].0, Some(corr3), "DLQ order: msg3 second");

    h.shutdown().await;
}

// ───────────────────────────────────────────────────────────────────────────
// (d.2) Real STATELESS dispatcher cell.
// ───────────────────────────────────────────────────────────────────────────

/// Stateless cell whose `handle()` blocks on a gate the first time it runs.
/// With `max_concurrency = 1` the single worker holds the only semaphore permit
/// while blocked, so the dispatcher parks on `acquire_owned()` after consuming
/// exactly one message — leaving the rest in the mailbox as the remainder.
struct GateStatelessCell {
    entered: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    gate: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
}

impl StatelessCell for GateStatelessCell {
    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a self,
        _msg: Message,
        _sink: &'a OutputSink,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            if let Some(tx) = self.entered.lock().expect("entered lock").take() {
                let _ = tx.send(());
            }
            let maybe_rx = self.gate.lock().expect("gate lock").take();
            if let Some(rx) = maybe_rx {
                let _ = rx.await;
            }
        }
    }
}

struct GateStatelessFactory {
    cell: Arc<GateStatelessCell>,
}

impl CellFactory for GateStatelessFactory {
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
        _idle_timeout: Option<Duration>,
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
        let cell = self.cell.clone();
        let join = tokio::spawn(async move {
            // cap concurrency at 1 → a blocked worker parks the dispatcher.
            stateless_dispatcher(
                path,
                rx,
                outputs_tx,
                cell,
                1,
                None,           // message_timeout
                Some(peace_tx), // Paket-8: dispatcher fires peace on the stop arm.
                Some(stop_rx),
                Some(colony_inbox_tx),
                Some(death_ack_tx),
                None,
                None,
            )
            .await;
        });
        let respawn: RespawnFn = Box::new(|| unreachable!("GateStateless no restart in this test"));
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

/// Demo (d), stateless part: a real stateless dispatcher disconnected while a
/// worker is wedged drains its unread mailbox remainder to the DLQ as
/// `cell_inactive`, in order.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disconnect_stateless_drains_remainder_to_dlq() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());

    let entered_slot: Arc<Mutex<Option<oneshot::Sender<()>>>> = Arc::new(Mutex::new(None));
    let (entered_tx, entered_rx) = oneshot::channel::<()>();
    *entered_slot.lock().unwrap() = Some(entered_tx);
    let (gate_tx, gate_rx) = oneshot::channel::<()>();
    let gate_slot: Arc<Mutex<Option<oneshot::Receiver<()>>>> = Arc::new(Mutex::new(Some(gate_rx)));

    let sl_factory: Arc<dyn CellFactory> = Arc::new(GateStatelessFactory {
        cell: Arc::new(GateStatelessCell {
            entered: entered_slot.clone(),
            gate: gate_slot.clone(),
        }),
    });

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap");

    let sl_dir = td.path().join("main/sl");
    std::fs::create_dir_all(&sl_dir).unwrap();
    let sl_spawned = sl_factory
        .spawn_cell(
            Path::new("/sl"),
            json!({}),
            h.runtime().outputs_tx,
            sl_dir,
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            0,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned(Path::new("/sl"), sl_spawned).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"sl","to":"peer"}]}}),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));

    // msg1 → consumed, worker wedged on the gate (holds the only permit).
    // The dispatcher then recvs msg2 and parks on `acquire_owned()` holding it
    // (one in-flight message past the mailbox). msg3, msg4 stay in the mailbox.
    // When the gate releases, worker#1 finishes → permit frees → msg2 is
    // processed (gate already consumed, returns immediately) → dispatcher loops
    // → biased stop wins → the mailbox remainder (msg3, msg4) → DLQ. So the
    // deterministic remainder is the last TWO queued messages.
    let corr1 = Uuid::now_v7();
    h.send(probe("/sl", corr1)).await;
    entered_rx.await.expect("/sl worker must enter handle()");
    let corr2 = Uuid::now_v7();
    let corr3 = Uuid::now_v7();
    let corr4 = Uuid::now_v7();
    h.send(probe("/sl", corr2)).await;
    h.send(probe("/sl", corr3)).await;
    h.send(probe("/sl", corr4)).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Disconnect concurrently; release the gate so the worker finishes and frees
    // the permit → the dispatcher loops → biased stop wins → remainder → DLQ.
    let inbox_tx = h.inbox_tx.clone();
    let mutation = tokio::spawn(async move {
        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Mutation {
                payload: json!({"scope":"/","diff":{"remove_edges":[{"match":{"from":"sl","to":"peer"}}]}}),
                reply_to: None,
                trace_id: Uuid::now_v7(),
                parent_message_id: Uuid::now_v7(),
                ack: ack_tx,
            })
            .await
            .unwrap();
        ack_rx.await.unwrap()
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let _ = gate_tx.send(());

    let outcome = mutation.await.expect("mutation task");
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "disconnect must commit (death-ack fires on dispatcher end), got {outcome:?}"
    );

    let inactive = inactive_dlq_for(&h, "/sl").await;
    assert_eq!(
        inactive.len(),
        2,
        "the two mailbox-remainder messages (msg3, msg4) must hit the DLQ as cell_inactive, got {inactive:?}"
    );
    assert_eq!(inactive[0].0, Some(corr3), "DLQ order: msg3 first");
    assert_eq!(inactive[1].0, Some(corr4), "DLQ order: msg4 second");

    h.shutdown().await;
}
