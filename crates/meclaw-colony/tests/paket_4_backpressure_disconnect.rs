//! Paket-4 P4-A1/A2: the P5-defer-proof backpressure-disconnect pin-demos.
//!
//! These two demos are the empirical proof of the **P5-defer premise** (
//! 2026-06-07): when a cell is disconnected while BLOCKED ON A FULL `outputs_tx`
//! (real backpressure), today's Variante-A gives a clean `Rejected{term_timeout}`
//! (full rollback, no zombie); then the colony loop RESUMES, drains `outputs_rx`,
//! the wedged cell UNBLOCKS NATURALLY, finishes `handle()`, the biased stop-arm
//! fires (stop was sent during the failed disconnect) → `ColonyMsg::Stopped` →
//! `handle_stopped` parks it `NotYetSpawned`; a RETRY disconnect then COMMITS
//! (status != Awake → the Step-10 stop-wiring guard is skipped → plain flip).
//! If this holds for BOTH stateful (A1) and long-running (A2) cells, Variante B
//! (full outputs-drain during the disconnect window) can be deferred to roadmap.
//!
//! ## The deterministic construction (Reviewer-Auflage A2 — MANDATORY ordering)
//! 1. `set_term_timeout_ms_for_test` TIGHT (800 ms) so the term-timeout fires
//!    fast + deterministically; outer failure markers 30 s. Hold
//!    `TERM_TIMEOUT_TEST_LOCK`.
//! 2. The cell's `handle()`: (a) signal "entered handle(), ready", (b) WAIT for a
//!    release oneshot the test holds, (c) on release, emit MORE messages than the
//!    `outputs_tx` capacity can hold to `/sink` → BLOCK on `push().await` once
//!    `outputs_rx` is full.
//! 3. **DISCONNECT FIRST**: probe the cell → it enters `handle()` + signals ready
//!    → test awaits ready → send the disconnect mutation CONCURRENTLY (spawned, so
//!    the test does not block on its ack). The colony processes it, fires
//!    `stop_tx`, enters the inline death-ack-await → the colony loop is now
//!    STALLED and NOT draining `outputs_rx`. A short sleep (< term_timeout) lets
//!    the colony reach that stall BEFORE the release.
//! 4. **THEN release the cell** → it floods > capacity → `outputs_tx` fills
//!    (colony not draining) → the cell BLOCKS in `push().await` → `death_ack`
//!    never fires → after the tight term_timeout → **`Rejected{term_timeout}`**
//!    (edge restored = full rollback, no zombie). Reverse order (flood THEN
//!    disconnect) is race-prone and FORBIDDEN.
//! 5. **Natural unblock**: after the reject, the colony loop RESUMES → drains
//!    `outputs_rx` (into a real `/sink` CaptureCell) → the cell's blocked `push()`
//!    completes → it finishes emitting → `handle()` returns → the biased stop-arm
//!    fires → `ColonyMsg::Stopped` → `handle_stopped` parks it `NotYetSpawned`.
//!    The test does NOT manually unblock — the colony's natural drain does.
//! 6. **RETRY disconnect** → asserts it **COMMITS** (`MutationOutcome::Committed`),
//!    cell ends inactive, NO `stop_wiring_unavailable` reject.
//!
//! ### outputs_tx capacity choice
//! The colony's `outputs_rx` channel capacity is hardcoded at 1000 in the test
//! harness (`ColonyHandle::build_with_blob_store`, `mpsc::channel(1000)`), and the
//! cell MUST emit into the SAME channel the colony drains (that is the whole point
//! of the natural-unblock mechanism) — so a smaller capacity cannot be injected
//! without a harness/production change (forbidden by the HARD RULES). We therefore
//! take the spec's "emit enough to fill (heavy but works)" path: the cell floods
//! `FLOOD = 1100` messages (> the 1000 capacity). Because the release happens only
//! AFTER the colony has stalled in death-ack-await (step 3/4 ordering), the colony
//! drains ZERO of these before the stall, so the cell fills `outputs_rx` to 1000
//! and BLOCKS on message #1001 — a deterministic, real backpressure block.
//!
//! ### Residual wart (DOCUMENTED, not fixed)
//! After the term_timeout rollback (`active=true` restored), the late-stopping
//! task is parked via `handle_stopped` → its mailbox remainder → DLQ
//! `cell_inactive`, even though the node is momentarily active. Minor (diagnostics
//! only), not a correctness hole. We assert the retry-commit OUTCOME regardless.

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, DbConn, LongRunningCell, MutationOutcome,
    RespawnFn, SpawnedCellKind, StatefulCell, bootstrap_from_filesystem,
    build_stateful_task_with_peace, cell_task_long_running, set_term_timeout_ms_for_test,
    spawn_watcher,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{
    Body, CellEmission, CellOutput, JsonValue, Message, MessageBuilder, OutputSink, Path, Uuid,
};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::future::Future;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};

/// Serialize the term-timeout-sensitive tests in this binary — they drive the
/// process-global `set_term_timeout_ms_for_test` static, so a parallel run would
/// race it. Held across `.await`, so `tokio::sync::Mutex` (yields, never blocks a
/// worker thread).
static TERM_TIMEOUT_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Messages the wedged `handle()` emits on release. Strictly greater than the
/// harness `outputs_rx` capacity (1000) so the cell BLOCKS on a full real channel
/// once the colony has stalled (and is therefore draining nothing). See the
/// module doc "outputs_tx capacity choice".
const FLOOD: usize = 1100;

/// Tight death-ack term-timeout for the wedged path (semantic timing
/// discriminator — kept short + justified per CLAUDE.md coding standards). The
/// release-after-stall sleep (100 ms) fits comfortably inside this budget, so the
/// colony is still in death-ack-await when the flood begins.
const TERM_TIMEOUT_MS: u64 = 800;

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

/// Root hive + echo cells `/a` (spare) and `/anchor`. `/anchor -> /sink` is the
/// persistent keep-alive edge the test never removes, so disconnecting the demo
/// cell from `/sink` does NOT also strand `/sink` (a no-stop-wiring CaptureCell)
/// — mirrors the existing 3b-disconnect harness's anti-strand rationale.
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
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/a"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/anchor/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/anchor"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
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

/// Send the disconnect mutation CONCURRENTLY (spawned) and return its
/// `JoinHandle` — the test releases the wedged cell while this is in flight, so
/// the test never blocks on the (slow, term-timeout-bounded) ack.
fn spawn_remove_edge(
    h: &ColonyHandle,
    from: &str,
    to: &str,
) -> tokio::task::JoinHandle<MutationOutcome> {
    let inbox_tx = h.inbox_tx.clone();
    let payload = json!({"scope":"/","diff":{"remove_edges":[{"match":{"from":from,"to":to}}]}});
    tokio::spawn(async move {
        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
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
    })
}

/// Read the RAM registry entry DTO for `path` (or `None` if absent).
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

/// Count edge rows in colony.db with the given from/to (fresh read-only conn).
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

/// One UBF emission body for the flood (valid `messages[]` so the colony's
/// debug-only UBF validator does not DLQ it).
fn flood_output(target: &str) -> CellOutput {
    CellOutput {
        target: Path::new(target),
        content: json!({"messages": []}),
    }
}

/// Probe in the UBF `origin` format (anti-cascade contract).
fn probe(target: &str) -> Message {
    MessageBuilder::new(Path::new(target))
        .body(Body::Inline(json!({"messages": []})))
        .build()
}

// ───────────────────────────────────────────────────────────────────────────
// Ready-then-flood-on-release LONG-RUNNING cell (A2 — make-or-break).
// ───────────────────────────────────────────────────────────────────────────

/// Long-running cell that, on its first `handle()`, signals "entered", waits for
/// a release oneshot, then floods `FLOOD` emissions to `/sink` — blocking on the
/// full `outputs_tx` once the colony stalls. Subsequent (retry-path) messages are
/// never processed (the cell is parked after the natural unblock).
struct FloodLongRunningCell {
    entered: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    release: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
}

impl LongRunningCell for FloodLongRunningCell {
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
            // Keep the channels alive so the handler_loop does not exit early.
            let _keep = (io, events_tx, reconfig_rx);
            std::future::pending::<()>().await
        }
    }

    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a mut self,
        _msg: Message,
        sink: &'a OutputSink,
        _db: &'a mut DbConn,
        _reconfig_tx: &'a mpsc::Sender<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            // (a) signal "entered handle(), ready".
            if let Some(tx) = self.entered.lock().expect("entered lock").take() {
                let _ = tx.send(());
            }
            // (b) wait for release (drop the guard before awaiting — a held
            // MutexGuard across `.await` would make the future `!Send`).
            let maybe_rx = self.release.lock().expect("release lock").take();
            if let Some(rx) = maybe_rx {
                let _ = rx.await;
            } else {
                // No release slot → not the first handle() (retry path). The cell
                // is parked before any retry message is processed, so this is
                // defensive; just return.
                return;
            }
            // (c) flood > capacity → block on the full outputs_tx. On a closed
            // channel (colony shutdown) stop early.
            for _ in 0..FLOOD {
                if sink.push(flood_output("/sink")).await.is_err() {
                    break;
                }
            }
        }
    }

    #[allow(clippy::manual_async_fn)]
    fn handle_event<'a>(
        &'a mut self,
        _event: Self::Event,
        _sink: &'a meclaw_core::OriginSink,
        _db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async {}
    }
}

/// Factory spawning a single live `FloodLongRunningCell` with real stop/death_ack
/// wiring (status Awake) — mirrors the existing `HangCellFactory` but with the
/// flood handler. Single-spawn (no restart in these demos).
struct FloodLrFactory {
    entered: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    release: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
}

impl CellFactory for FloodLrFactory {
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
        _message_timeout: Option<Duration>,
        _blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
        _mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let (death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
        let (peace_tx, peace_rx) = oneshot::channel::<()>();
        let (_backstop_tx, backstop_rx) = oneshot::channel::<()>();
        let (tx, rx) = mpsc::channel::<Message>(1000);
        let cell = FloodLongRunningCell {
            entered: self.entered.clone(),
            release: self.release.clone(),
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
        let respawn: RespawnFn = Box::new(|| unreachable!("FloodLr no restart"));
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

// ───────────────────────────────────────────────────────────────────────────
// Ready-then-flood-on-release STATEFUL (Dormant) cell (A1).
// ───────────────────────────────────────────────────────────────────────────

/// Stateful cell with the same ready-then-flood-on-release handler. Woken lazily
/// via wake-pre-send; its `WakeFn` returns the live stop pair so the disconnect
/// can peace-stop it (Task-7.5 pattern).
struct FloodStatefulCell {
    entered: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    release: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
}

impl StatefulCell for FloodStatefulCell {
    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a mut self,
        _msg: Message,
        sink: &'a OutputSink,
        _db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            if let Some(tx) = self.entered.lock().expect("entered lock").take() {
                let _ = tx.send(());
            }
            let maybe_rx = self.release.lock().expect("release lock").take();
            if let Some(rx) = maybe_rx {
                let _ = rx.await;
            } else {
                return;
            }
            for _ in 0..FLOOD {
                if sink.push(flood_output("/sink")).await.is_err() {
                    break;
                }
            }
        }
    }
}

/// Dormant factory for `FloodStatefulCell` (Task-7.5 pattern: `WakeFn` returns the
/// live `(stop_tx, death_ack_rx)` so the colony can peace-stop the woken cell).
struct FloodStatefulFactory {
    entered: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    release: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
}

impl CellFactory for FloodStatefulFactory {
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
        message_timeout: Option<Duration>,
        _blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
        _mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let (sender, receiver) = mpsc::channel::<Message>(1000);

        let wake_path = path.clone();
        let wake_outputs = outputs_tx.clone();
        let wake_inbox = colony_inbox_tx.clone();
        let wake_watcher_inbox = colony_inbox_tx.clone();
        let wake_cell_dir = cell_dir.clone();
        let entered = self.entered.clone();
        let release = self.release.clone();
        let wake: meclaw_colony::WakeFn = Box::new(move |recv: mpsc::Receiver<Message>| {
            let conn =
                meclaw_colony::persist::open_or_create_cell_db(&wake_cell_dir.join("cell.db"))
                    .expect("wake: open_or_create_cell_db");
            let db = DbConn::wrap(conn, None);
            let cell = FloodStatefulCell {
                entered: entered.clone(),
                release: release.clone(),
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
        let respawn: RespawnFn = Box::new(|| unreachable!("FloodStateful no restart"));
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
// A2 — long-running variant ⚠️ MAKE-OR-BREAK.
// ───────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lr_backpressure_disconnect_term_timeout_then_natural_unblock_retry_commits() {
    let _guard = TERM_TIMEOUT_TEST_LOCK.lock().await;
    set_term_timeout_ms_for_test(TERM_TIMEOUT_MS);
    let td = TempDir::new().unwrap();
    write_topology(td.path());
    let db_dir = td.path().to_path_buf();

    let entered_slot: Arc<Mutex<Option<oneshot::Sender<()>>>> = Arc::new(Mutex::new(None));
    let (entered_tx, entered_rx) = oneshot::channel::<()>();
    *entered_slot.lock().unwrap() = Some(entered_tx);
    let (release_tx, release_rx) = oneshot::channel::<()>();
    let release_slot: Arc<Mutex<Option<oneshot::Receiver<()>>>> =
        Arc::new(Mutex::new(Some(release_rx)));

    let factory: Arc<dyn CellFactory> = Arc::new(FloodLrFactory {
        entered: entered_slot.clone(),
        release: release_slot.clone(),
    });

    let (h, mut death_ack_wait_rx) =
        ColonyHandle::new_with_factories_and_death_ack_signal_at(&td, echo_factories());
    // /sink BEFORE bootstrap (anti-cascade) — a REAL CaptureCell that consumes
    // from outputs so the colony's natural drain (step 5) has somewhere to go.
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(FLOOD + 64);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap");

    // Register the live LR cell /lr (Active, real stop/death_ack wiring).
    let lr_spawned = factory
        .spawn_cell(
            Path::new("/lr"),
            json!({}),
            h.runtime().outputs_tx,
            td.path().join("lr"),
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            0,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned(Path::new("/lr"), lr_spawned).await;

    // Connect /lr -> /sink so it is active, plus a persistent /anchor -> /sink
    // edge that keeps /sink connected (active) after /lr's edge is removed — so a
    // disconnect of /lr never deactivates the no-stop-wiring /sink (which would
    // otherwise trip the Step-10 stop-wiring guard). /sink is also the flood
    // target, so the colony routes the flood there (CaptureCell consumes it).
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[
            {"from":"lr","to":"sink"},
            {"from":"anchor","to":"sink"}
        ]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "add_edges lr->sink must commit, got {outcome:?}"
    );
    assert!(ram_entry(&h, "/lr").await.expect("/lr").active);
    assert_eq!(db_edge_count(&db_dir, "/lr", "/sink"), 1);

    // Probe /lr → it enters handle() + signals ready, then waits for release.
    h.send(probe("/lr")).await;
    entered_rx.await.expect("/lr must enter handle()");

    // DISCONNECT FIRST (concurrent): the colony fires the peace-stop + enters the
    // inline death-ack-await → loop STALLED. The cell is still waiting on release,
    // so nothing has been drained yet.
    let mutation = spawn_remove_edge(&h, "lr", "sink");
    // Let the colony reach the death-ack-await stall BEFORE releasing.
    // Deterministic sync (replaces the old wall-clock sleep that flaked in
    // release): wait until the colony has sent the peace-stop and is about to
    // block on the inline death-ack-wait. Only THEN release the wedged cell, so
    // the term-timeout ordering holds regardless of build profile or CPU load.
    // 30s is a generous failure marker, not a semantic discriminator.
    tokio::time::timeout(Duration::from_secs(30), death_ack_wait_rx.recv())
        .await
        .expect("colony must reach the inline death-ack-wait within 30s")
        .expect("death-ack-wait signal channel closed unexpectedly");
    // THEN release → cell floods > capacity → blocks on the full outputs_tx →
    // death_ack never fires → term_timeout reject.
    let _ = release_tx.send(());

    let outcome = mutation.await.expect("mutation task");
    match outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(
                error_code, "term_timeout",
                "wedged-on-backpressure disconnect must reject with term_timeout, got {error_code}"
            );
        }
        other => panic!("expected Rejected{{term_timeout}}, got {other:?}"),
    }

    // Full rollback: edge restored in colony.db, /lr stays active in RAM.
    assert_eq!(
        db_edge_count(&db_dir, "/lr", "/sink"),
        1,
        "edge must remain after term_timeout rollback"
    );
    // No-Delete after term_timeout rollback: the entry must STILL EXIST (a real
    // zombie would be deleted or broken). The `active` flag is transient here —
    // the natural unblock below immediately re-parks the cell — so we assert the
    // durable no-delete invariant; the clean park (`drain_until_parked`) and the
    // committing retry disconnect below prove no-zombie non-racily.
    assert!(
        ram_entry(&h, "/lr").await.is_some(),
        "/lr entry must still exist after term_timeout rollback (no-delete)"
    );

    // NATURAL UNBLOCK: the colony loop resumed, is draining outputs_rx into /sink
    // → the cell's blocked push() completes → handle() returns → biased stop-arm
    // fires → ColonyMsg::Stopped → handle_stopped parks NotYetSpawned. Prove the
    // drain happened (positive receipt: /sink received the flood) and the cell
    // parked, by polling the lifecycle_status.
    drain_until_parked(&h, "/lr", sink_rx).await;

    // ── RETRY disconnect → MUST COMMIT (status NotYetSpawned → guard skipped). ──
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"remove_edges":[{"match":{"from":"lr","to":"sink"}}]}}),
    )
    .await;
    match outcome {
        MutationOutcome::Committed { .. } => {}
        other => panic!(
            "MAKE-OR-BREAK: retry disconnect must COMMIT after natural unblock, got {other:?}"
        ),
    }

    let e = ram_entry(&h, "/lr").await.expect("/lr entry must STAY");
    assert!(!e.active, "/lr must be inactive after the committed retry");

    h.shutdown().await;
    assert_eq!(
        db_edge_count(&db_dir, "/lr", "/sink"),
        0,
        "edge must be gone in colony.db after the committed retry disconnect"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// A1 — stateful variant.
// ───────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stateful_backpressure_disconnect_term_timeout_then_natural_unblock_retry_commits() {
    let _guard = TERM_TIMEOUT_TEST_LOCK.lock().await;
    set_term_timeout_ms_for_test(TERM_TIMEOUT_MS);
    let td = TempDir::new().unwrap();
    write_topology(td.path());
    let db_dir = td.path().to_path_buf();

    let entered_slot: Arc<Mutex<Option<oneshot::Sender<()>>>> = Arc::new(Mutex::new(None));
    let (entered_tx, entered_rx) = oneshot::channel::<()>();
    *entered_slot.lock().unwrap() = Some(entered_tx);
    let (release_tx, release_rx) = oneshot::channel::<()>();
    let release_slot: Arc<Mutex<Option<oneshot::Receiver<()>>>> =
        Arc::new(Mutex::new(Some(release_rx)));

    let factory: Arc<dyn CellFactory> = Arc::new(FloodStatefulFactory {
        entered: entered_slot.clone(),
        release: release_slot.clone(),
    });

    let (h, mut death_ack_wait_rx) =
        ColonyHandle::new_with_factories_and_death_ack_signal_at(&td, echo_factories());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(FLOOD + 64);
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
    let sf_spawned = factory
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
    assert_eq!(
        ram_entry(&h, "/sf").await.expect("/sf").lifecycle_status,
        "NotYetSpawned"
    );

    // Connect /sf -> /sink (still NotYetSpawned, no task yet) + persistent
    // /anchor -> /sink keep-alive (same rationale as the LR demo).
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[
            {"from":"sf","to":"sink"},
            {"from":"anchor","to":"sink"}
        ]}}),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));
    assert!(ram_entry(&h, "/sf").await.expect("/sf").active);
    assert_eq!(db_edge_count(&db_dir, "/sf", "/sink"), 1);

    // WAKE /sf: route a probe → wake-pre-send spawns the task (status → Awake),
    // stores the live stop pair; handle() signals ready + waits for release.
    h.send(probe("/sf")).await;
    entered_rx.await.expect("/sf must enter handle()");
    assert_eq!(
        ram_entry(&h, "/sf").await.expect("/sf").lifecycle_status,
        "Awake",
        "/sf must be Awake after wake-pre-send"
    );

    // DISCONNECT FIRST (concurrent) → colony stalls in death-ack-await → THEN
    // release → flood blocks on full outputs_tx → term_timeout reject.
    let mutation = spawn_remove_edge(&h, "sf", "sink");
    // Deterministic sync (replaces the old wall-clock sleep that flaked in
    // release): wait until the colony has sent the peace-stop and is about to
    // block on the inline death-ack-wait. Only THEN release the wedged cell, so
    // the term-timeout ordering holds regardless of build profile or CPU load.
    // 30s is a generous failure marker, not a semantic discriminator.
    tokio::time::timeout(Duration::from_secs(30), death_ack_wait_rx.recv())
        .await
        .expect("colony must reach the inline death-ack-wait within 30s")
        .expect("death-ack-wait signal channel closed unexpectedly");
    let _ = release_tx.send(());

    let outcome = mutation.await.expect("mutation task");
    match outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(
                error_code, "term_timeout",
                "wedged-on-backpressure disconnect must reject with term_timeout, got {error_code}"
            );
        }
        other => panic!("expected Rejected{{term_timeout}}, got {other:?}"),
    }
    assert_eq!(
        db_edge_count(&db_dir, "/sf", "/sink"),
        1,
        "edge must remain after term_timeout rollback"
    );
    // No-Delete after term_timeout rollback: the entry must STILL EXIST (a real
    // zombie would be deleted or broken). The `active` flag is transient here —
    // the natural unblock below immediately re-parks the cell — so we assert the
    // durable no-delete invariant; the clean park (`drain_until_parked`) and the
    // committing retry disconnect below prove no-zombie non-racily.
    assert!(
        ram_entry(&h, "/sf").await.is_some(),
        "/sf entry must still exist after term_timeout rollback (no-delete)"
    );

    // NATURAL UNBLOCK → cell parks NotYetSpawned.
    drain_until_parked(&h, "/sf", sink_rx).await;

    // ── RETRY disconnect → MUST COMMIT. ──
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"remove_edges":[{"match":{"from":"sf","to":"sink"}}]}}),
    )
    .await;
    match outcome {
        MutationOutcome::Committed { .. } => {}
        other => panic!("retry disconnect must COMMIT after natural unblock, got {other:?}"),
    }
    let e = ram_entry(&h, "/sf").await.expect("/sf entry must STAY");
    assert!(!e.active, "/sf must be inactive after the committed retry");

    h.shutdown().await;
    assert_eq!(
        db_edge_count(&db_dir, "/sf", "/sink"),
        0,
        "edge must be gone in colony.db after the committed retry disconnect"
    );
}

/// After the term_timeout reject, prove the wedged `handle()` unblocked
/// NATURALLY: the colony loop resumed → drained `outputs_rx` → the cell's blocked
/// `push()` completed → it finished the flood → `handle()` returned → the biased
/// stop-arm fired (stop was sent during the failed disconnect) → `ColonyMsg::
/// Stopped` → `handle_stopped` parked it `NotYetSpawned`. We poll the cell's
/// `lifecycle_status` until it flips away from `Awake` — that flip is the
/// positive receipt of the whole chain (it is impossible without the flood having
/// drained, since `handle()` cannot return while blocked on a full `outputs_tx`).
///
/// The colony routes the drained flood to `/sink` (the cell's emitted target ==
/// the restored edge target), where the `CaptureCell` consumes it into `sink_rx`.
/// We keep draining `sink_rx` here so `/sink`'s mailbox never back-pressures the
/// colony's drain; the `lifecycle_status` flip is the real proof of natural
/// unblock. Generous 30 s failure marker.
async fn drain_until_parked(h: &ColonyHandle, path: &str, mut sink_rx: mpsc::Receiver<Message>) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let status = ram_entry(h, path)
            .await
            .expect("entry must stay")
            .lifecycle_status;
        if status != "Awake" {
            assert_eq!(
                status, "NotYetSpawned",
                "cell must park as NotYetSpawned after natural unblock + Stopped"
            );
            break;
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "MAKE-OR-BREAK: cell {path} did not park (still Awake) after the reject — \
                 it did NOT unblock naturally; P5-defer premise broken"
            );
        }
        // Drain anything that did land on /sink so it never back-pressures.
        while sink_rx.try_recv().is_ok() {}
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    while sink_rx.try_recv().is_ok() {}
}

// ───────────────────────────────────────────────────────────────────────────
// Pre-14 Pass-2 backstop A2 — explicit No-Delete IDENTITY across term_timeout.
// ───────────────────────────────────────────────────────────────────────────
//
// The two demos above pin `Rejected{term_timeout}` + full rollback + no-zombie +
// retry-commit. This backstop adds the one No-Delete facet they assert only via
// `active`/entry-presence, not verbatim: the node's `cell_id` (its IDENTITY) is
// STABLE across the WHOLE term_timeout → rollback → natural-unblock → park →
// reconnect-retry cycle. A stable cell_id proves the node was never deleted and
// never re-created — only disconnected/parked, exactly the No-Delete + atomic-
// rollback contract (spec § No-Delete-Policy, Z.1397). It reuses the same LR
// flood/disconnect machinery (deterministic ordering); the only new assertions
// are the cell_id chain and the explicit lifecycle_status at each stage.

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lr_term_timeout_rollback_preserves_node_identity_no_delete() {
    let _guard = TERM_TIMEOUT_TEST_LOCK.lock().await;
    set_term_timeout_ms_for_test(TERM_TIMEOUT_MS);
    let td = TempDir::new().unwrap();
    write_topology(td.path());
    let db_dir = td.path().to_path_buf();

    let entered_slot: Arc<Mutex<Option<oneshot::Sender<()>>>> = Arc::new(Mutex::new(None));
    let (entered_tx, entered_rx) = oneshot::channel::<()>();
    *entered_slot.lock().unwrap() = Some(entered_tx);
    let (release_tx, release_rx) = oneshot::channel::<()>();
    let release_slot: Arc<Mutex<Option<oneshot::Receiver<()>>>> =
        Arc::new(Mutex::new(Some(release_rx)));

    let factory: Arc<dyn CellFactory> = Arc::new(FloodLrFactory {
        entered: entered_slot.clone(),
        release: release_slot.clone(),
    });

    let (h, mut death_ack_wait_rx) =
        ColonyHandle::new_with_factories_and_death_ack_signal_at(&td, echo_factories());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(FLOOD + 64);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap");

    let lr_spawned = factory
        .spawn_cell(
            Path::new("/lr"),
            json!({}),
            h.runtime().outputs_tx,
            td.path().join("lr"),
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            0,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned(Path::new("/lr"), lr_spawned).await;

    // Connect /lr -> /sink + persistent /anchor -> /sink keep-alive. Capture the
    // node IDENTITY at the live baseline — every later stage must match it.
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[
            {"from":"lr","to":"sink"},
            {"from":"anchor","to":"sink"}
        ]}}),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));
    let before = ram_entry(&h, "/lr").await.expect("/lr registered");
    assert!(before.active, "/lr active at baseline");
    let cell_id = before.cell_id.clone();

    // Probe → enter handle() → DISCONNECT FIRST (concurrent) → colony stalls in
    // death-ack-await → THEN release → flood blocks on full outputs_tx → reject.
    h.send(probe("/lr")).await;
    entered_rx.await.expect("/lr must enter handle()");
    let mutation = spawn_remove_edge(&h, "lr", "sink");
    // Deterministic sync (replaces the old wall-clock sleep that flaked in
    // release): wait until the colony has sent the peace-stop and is about to
    // block on the inline death-ack-wait. Only THEN release the wedged cell, so
    // the term-timeout ordering holds regardless of build profile or CPU load.
    // 30s is a generous failure marker, not a semantic discriminator.
    tokio::time::timeout(Duration::from_secs(30), death_ack_wait_rx.recv())
        .await
        .expect("colony must reach the inline death-ack-wait within 30s")
        .expect("death-ack-wait signal channel closed unexpectedly");
    let _ = release_tx.send(());

    match mutation.await.expect("mutation task") {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(error_code, "term_timeout", "must reject with term_timeout");
        }
        other => panic!("expected Rejected{{term_timeout}}, got {other:?}"),
    }

    // After the atomic rollback: SAME entry, SAME identity, edge back. The
    // `active` flag is transient (the natural unblock below re-parks the cell),
    // so we assert the durable No-Delete invariants instead — entry STAYS and
    // cell_id is stable — plus the clean park + committing retry below.
    let after_reject = ram_entry(&h, "/lr")
        .await
        .expect("/lr entry must STAY after rollback (No-Delete)");
    assert_eq!(
        after_reject.cell_id, cell_id,
        "cell_id STABLE across the term_timeout rollback (No-Delete: same node, not re-created)"
    );
    assert_eq!(db_edge_count(&db_dir, "/lr", "/sink"), 1, "edge restored");

    // Natural unblock → park NotYetSpawned. Identity STILL stable (parked, not gone).
    drain_until_parked(&h, "/lr", sink_rx).await;
    let parked = ram_entry(&h, "/lr")
        .await
        .expect("/lr entry must STAY after park (No-Delete)");
    assert_eq!(parked.lifecycle_status, "NotYetSpawned", "/lr parked");
    assert_eq!(
        parked.cell_id, cell_id,
        "cell_id STABLE across the natural-unblock park (No-Delete)"
    );

    // Reconnectable: a retry disconnect COMMITS (the node is still a live, mutable
    // registry target) and the identity is STILL the same parked entry.
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"remove_edges":[{"match":{"from":"lr","to":"sink"}}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "retry disconnect must COMMIT after natural unblock, got {outcome:?}"
    );
    let final_entry = ram_entry(&h, "/lr")
        .await
        .expect("/lr entry must STAY after the committed retry (No-Delete)");
    assert!(!final_entry.active, "/lr inactive after committed retry");
    assert_eq!(
        final_entry.cell_id, cell_id,
        "cell_id STABLE across the ENTIRE term_timeout→rollback→park→retry cycle (No-Delete)"
    );

    h.shutdown().await;
}
