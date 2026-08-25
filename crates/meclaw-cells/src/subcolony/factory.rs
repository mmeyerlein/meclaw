//! The factory for the `subcolony` cell type.
//!
//! Structurally identical to `mcp`'s and `harness`'s: open `cell.db`
//! synchronously, restore the β overlay, build the cell, and funnel the spawn
//! through `build_long_running_task`. The `make_build` closure stays sync and
//! await-free between the DB open and the spawn (phase-5 restart-barrier
//! tripwire).
//!
//! No schema of its own: unlike `harness` this cell keeps no task register, so
//! `cell.db` exists here only for the params overlay. Nothing about a request
//! needs to survive a restart, because nothing is ever resumed — see
//! `cell.rs` for why that is not the same claim as "a request is idempotent".
//!
//! The child process is NOT started here. It comes up in the I/O sub-task, so
//! the corridor has nothing to wait on and a respawn always begins from a clean
//! slate.

use crate::subcolony::cell::SubcolonyCell;
use crate::subcolony::params::{SubcolonyOverlay, SubcolonyParams};
use meclaw_colony::persist::cell_db::open_or_create_cell_db_with_status;
use meclaw_colony::{CellFactory, DbConn, RespawnFn, SpawnedCellKind, build_long_running_task};
use meclaw_core::{CellEmission, Message, Path};
use serde_json::Value as JsonValue;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// `subcolony`-cell factory.
pub struct SubcolonyCellFactory;

impl CellFactory for SubcolonyCellFactory {
    /// This type's tables are fixed in its own Rust code, so a seed header --
    /// which describes rows, not a schema -- can never describe them (GH #399,
    /// same class as GH #398). Declaring this keeps the mutation staging seeder
    /// out of the database entirely.
    ///
    /// It carries an obligation: a type that declares this must load its own
    /// seed files, because nobody else will. `subcolony` has no such loader and
    /// wants none, so the default `validate_cell_dir` refuses a `seed/*.jsonl`
    /// beside it by name instead of ignoring it in silence.
    fn owns_schema(&self) -> bool {
        true
    }

    /// The `cell.type` string, so the refusal above names what an operator
    /// wrote in `config.json` rather than a Rust identifier.
    fn type_name(&self) -> &'static str {
        "subcolony"
    }

    /// Pre-spawn validation, through the same parse path as `spawn_cell`
    /// (parser invariant) — including the filesystem check on `root`.
    fn validate_params(&self, params: &JsonValue) -> Result<(), String> {
        SubcolonyParams::parse(params).map(|_| ())
    }

    /// Spawn a `subcolony` cell instance.
    ///
    /// **Corridor duty (phase-5 tripwire)**: `make_build` runs on the initial
    /// spawn AND on every respawn (`RespawnFn` is `Fn`, not `FnOnce`). Between
    /// the spawn via `build_long_running_task` and the registry sender swap in
    /// `colony::handle_cell_died` there must be NO `.await`; every step here is
    /// sync.
    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        cell_dir: PathBuf,
        contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<meclaw_colony::ColonyMsg>,
        _idle_timeout: Option<Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<Duration>,
        blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let respawn_inbox = colony_inbox_tx.clone();
        let respawn_path = path.clone();
        let build = make_build(
            params,
            path,
            outputs_tx,
            cell_dir,
            colony_inbox_tx,
            blob_store,
            mailbox_capacity,
            contract.consumes.clone(),
            contract.transfer_bounds(),
        )?;

        let (sender, join, peace_rx, stop_tx, death_ack_rx, backstop_rx) = build();
        let respawn: RespawnFn = Box::new(move || {
            let (sender, join, peace_rx, stop_tx, death_ack_rx, backstop_rx) = build();
            meclaw_colony::renotify_stop_wiring(
                &respawn_inbox,
                respawn_path.clone(),
                stop_tx,
                death_ack_rx,
            );
            (sender, join, peace_rx, backstop_rx)
        });
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

    /// A cell that booted INACTIVE gets a real `RespawnFn` without an initial
    /// spawn, so boot-gating is preserved: no child process until a reconnect.
    fn build_boot_inactive_respawn(
        self: Arc<Self>,
        path: Path,
        params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        cell_dir: PathBuf,
        contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<meclaw_colony::ColonyMsg>,
        _idle_timeout: Option<Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<Duration>,
        blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Option<RespawnFn> {
        let respawn_inbox = colony_inbox_tx.clone();
        let respawn_path = path.clone();
        let build = make_build(
            params,
            path,
            outputs_tx,
            cell_dir,
            colony_inbox_tx,
            blob_store,
            mailbox_capacity,
            contract.consumes.clone(),
            contract.transfer_bounds(),
        )
        .ok()?;
        Some(Box::new(move || {
            let (sender, join, peace_rx, stop_tx, death_ack_rx, backstop_rx) = build();
            meclaw_colony::renotify_stop_wiring(
                &respawn_inbox,
                respawn_path.clone(),
                stop_tx,
                death_ack_rx,
            );
            (sender, join, peace_rx, backstop_rx)
        }))
    }
}

/// Build the closure that constructs a fresh `subcolony` cell-task. Shared by
/// both spawn paths so the `RespawnFn` construction has ONE definition.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn make_build(
    params: JsonValue,
    path: Path,
    outputs_tx: mpsc::Sender<CellEmission>,
    cell_dir: PathBuf,
    colony_inbox_tx: mpsc::Sender<meclaw_colony::ColonyMsg>,
    blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
    mailbox_capacity: usize,
    consumes: Option<Arc<meclaw_core::CompiledConsumes>>,
    bounds: meclaw_core::TransferBounds,
) -> Result<
    impl Fn() -> (
        mpsc::Sender<Message>,
        JoinHandle<()>,
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Receiver<()>,
    ),
    String,
> {
    // Parse once here so a bad config fails the spawn instead of the task.
    SubcolonyParams::parse(&params)?;

    let birth_cap = params;
    let path_cap = path;
    let outputs_cap = outputs_tx;
    let cell_dir_cap = cell_dir;
    let colony_inbox_cap = colony_inbox_tx;
    let blob_cap = blob_store;
    let mailbox_capacity_cap = mailbox_capacity;
    let consumes_cap = consumes;
    // GH #260: the substrate half of the write boundary, captured like the
    // consumes views so restart and reconnect carry the same declaration.
    let bounds_cap = bounds;

    Ok(move || -> (
        mpsc::Sender<Message>,
        JoinHandle<()>,
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Receiver<()>,
    ) {
        // 1. Open cell.db (sync). Only the params overlay lives here.
        let (conn, _status) = open_or_create_cell_db_with_status(&cell_dir_cap.join("cell.db"))
            .expect("open cell.db");
        // 2. β restore: effective tunables = birth ⊕ cell.db overlay.
        let overlay = crate::params_overlay::restore::<SubcolonyOverlay>(&conn, &birth_cap)
            .expect("restore subcolony overlay");
        let mut params = SubcolonyParams::parse(&birth_cap).expect("validated in plan-phase");
        params.boot_timeout_ms = overlay.boot_timeout_ms;
        params.request_timeout_ms = overlay.request_timeout_ms;
        params.external_timeout_ms = overlay.external_timeout_ms;
        params.query_timeout_ms = overlay.query_timeout_ms;

        let query_timeout = Duration::from_millis(params.query_timeout_ms);
        let cell = SubcolonyCell::new(params);
        let db = DbConn::wrap(conn, Some(query_timeout));
        let (tx, rx) = mpsc::channel::<Message>(mailbox_capacity_cap);
        let (join, peace_rx, stop_tx, death_ack_rx, backstop_rx) = build_long_running_task(
            path_cap.clone(),
            rx,
            outputs_cap.clone(),
            64,
            cell,
            db,
            Some(colony_inbox_cap.clone()),
            blob_cap.clone(),
            consumes_cap.clone(),
            bounds_cap,
        );
        (tx, join, peace_rx, stop_tx, death_ack_rx, backstop_rx)
    })
}
