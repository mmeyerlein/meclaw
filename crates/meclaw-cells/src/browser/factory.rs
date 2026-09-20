//! The factory for the `browser` cell.
//!
//! Same corridor duty as every long-running type (phase-5 tripwire):
//! everything between the `cell.db` open and the long-running spawn is sync and
//! await-free, because the `RespawnFn` runs inside the colony's restart barrier.
//!
//! `is_lazy` stays `false`, for the reason `web` and `voice` give: the mount
//! must be on the table when the colony is up. A browser cell that waited for
//! its first message would refuse the first `page:` join.

use crate::browser::cell::BrowserCell;
use crate::browser::db::setup_browser_schema;
use crate::browser::io::BrowserIo;
use crate::browser::params::{BrowserParams, cell_id_of, profile_dir_for};
use meclaw_colony::persist::cell_db::open_or_create_cell_db_with_status;
use meclaw_colony::{
    CellFactory, DbConn, RespawnFn, SpawnedCellKind, SurfaceRegistry, build_long_running_task,
};
use meclaw_core::{CellEmission, JsonValue, Message, Path};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// The six-tuple a (re)spawn hands back.
type SpawnTuple = (
    mpsc::Sender<Message>,
    JoinHandle<()>,
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Receiver<()>,
);

/// The `browser` cell factory.
///
/// It carries the process's [`SurfaceRegistry`] so every cell it builds
/// registers its mount on the one table a `page:` join is answered from
/// (ADR-0031). A factory built with [`Default`] gets a table of its own, which
/// is what a fixture wants.
pub struct BrowserCellFactory {
    surfaces: Arc<SurfaceRegistry>,
}

impl BrowserCellFactory {
    /// A factory whose cells mount on `surfaces`.
    pub fn new(surfaces: Arc<SurfaceRegistry>) -> Self {
        Self { surfaces }
    }

    /// The table this factory's cells mount on.
    pub fn surfaces(&self) -> Arc<SurfaceRegistry> {
        Arc::clone(&self.surfaces)
    }
}

impl Default for BrowserCellFactory {
    fn default() -> Self {
        Self::new(Arc::new(SurfaceRegistry::new()))
    }
}

impl CellFactory for BrowserCellFactory {
    fn validate_params(&self, params: &JsonValue) -> Result<(), String> {
        BrowserParams::parse(params).map(|_| ())
    }

    fn type_name(&self) -> &'static str {
        "browser"
    }

    /// The `pages` table is fixed in this type's own Rust code, so a seed header
    /// — which describes rows, not a schema — can never describe it (GH #399).
    fn owns_schema(&self) -> bool {
        true
    }

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
            self.surfaces(),
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
            self.surfaces(),
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

/// Build the closure that constructs a fresh `browser` cell-task.
///
/// Params are parsed once, outside the closure, so a bad document fails the
/// spawn rather than panicking on the respawn path — the parser invariant every
/// factory in this tree keeps.
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
    surfaces: Arc<SurfaceRegistry>,
) -> Result<impl Fn() -> SpawnTuple, String> {
    let birth_parsed = BrowserParams::parse(&params)?;

    let path_cap = path;
    let outputs_cap = outputs_tx;
    let cell_dir_cap = cell_dir;
    let colony_inbox_cap = colony_inbox_tx;
    let blob_cap = blob_store;
    let mailbox_capacity_cap = mailbox_capacity;
    let consumes_cap = consumes;
    let bounds_cap = bounds;
    let surfaces_cap = surfaces;
    let params_cap = birth_parsed;

    Ok(move || -> SpawnTuple {
        // 1. Open cell.db (sync). The `pages` table has to survive this
        //    restart — that is the whole point of it (OR-G17).
        let (conn, _status) = open_or_create_cell_db_with_status(&cell_dir_cap.join("cell.db"))
            .expect("open cell.db");
        // 2. Idempotent DDL (sync, outside the corridor).
        setup_browser_schema(&conn).expect("setup_browser_schema");

        // 3. Both halves (sync). The browser itself is started by the I/O
        //    half, not here: starting it is async, and this closure runs inside
        //    the restart corridor, where an `.await` is a deadlock.
        let profile_dir = profile_dir_for(&params_cap, &cell_id_of(&cell_dir_cap));
        let io = BrowserIo::new(
            params_cap.clone(),
            profile_dir,
            path_cap.clone(),
            Arc::clone(&surfaces_cap),
        );
        let cell = BrowserCell::new(path_cap.clone(), params_cap.clone(), io);
        let db = DbConn::wrap(
            conn,
            Some(Duration::from_millis(params_cap.external_timeout_ms)),
        );
        let (tx, rx) = mpsc::channel::<Message>(mailbox_capacity_cap);
        // 4. The single long-running spawn site. No `.await` reached this line.
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
            bounds_cap.clone(),
        );
        (tx, join, peace_rx, stop_tx, death_ack_rx, backstop_rx)
    })
}
