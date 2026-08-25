//! W8 (GH #380): the factory for the `web` cell.
//!
//! Mirrors the proxy factory position for position, because the corridor rule
//! is the same: everything between the `cell.db` open and the long-running
//! spawn is sync and await-free (phase-5 tripwire — the `RespawnFn` runs inside
//! the colony's restart barrier, and an `.await` in there is a deadlock waiting
//! for a restart).
//!
//! The one deliberate difference from the lazy cell types: `is_lazy` stays
//! `false`. A display must be up when the colony is. A `web` cell that waited
//! for its first message would answer a browser with a blank page until
//! something else happened to talk to it.

use crate::web::assets::AssetMap;
use crate::web::cell::WebCell;
use crate::web::db::setup_web_schema;
use crate::web::io::WebIo;
use crate::web::params::WebParams;
use crate::web::render::PageMap;
use crate::web::seed;
use meclaw_colony::persist::cell_db::{OpenStatus, open_or_create_cell_db_with_status};
use meclaw_colony::{CellFactory, DbConn, RespawnFn, SpawnedCellKind, build_long_running_task};
use meclaw_core::{CellEmission, JsonValue, Message, Path};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

/// The six-tuple a (re)spawn hands back: mailbox sender, join handle, and the
/// four lifecycle oneshot ends minted by `build_long_running_task`.
type SpawnTuple = (
    mpsc::Sender<Message>,
    JoinHandle<()>,
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Receiver<()>,
);

/// The `web` cell factory.
pub struct WebCellFactory;

impl CellFactory for WebCellFactory {
    fn validate_params(&self, params: &JsonValue) -> Result<(), String> {
        WebParams::parse(params).map(|_| ())
    }

    /// The on-disk half of the pre-spawn validation (issue #56 shape): a
    /// `seed/<table>.jsonl` is statically parseable configuration, so a mistake
    /// in one must surface during `--validate` and at spawn — not as a surprise
    /// on the first boot of a display nobody is watching.
    ///
    /// Routes through the same parse path the loader uses, which is what keeps
    /// validate-equals-spawn honest.
    fn validate_cell_dir(
        &self,
        params: &JsonValue,
        cell_dir: &std::path::Path,
    ) -> Result<(), String> {
        WebParams::parse(params)?;
        seed::check_seed_files(cell_dir).map_err(|e| format!("web: {e}"))
    }

    /// A display's tables are fixed in [`crate::web::db`] and cannot be
    /// described by a seed header, so the mutation staging seeder stays out of
    /// this cell's database (GH #398).
    ///
    /// The obligation that comes with the declaration is discharged in
    /// [`make_build`]: every spawn runs `setup_web_schema`, and a spawn that
    /// created the file runs [`seed::load_seed_if_present`] behind it. That is
    /// the same path a display instantiated from the filesystem at boot has
    /// always taken; before this declaration, one grown by mutation took a
    /// different one and got constraint-free tables — no `pages.route` key, so
    /// `page.set` was refused by SQLite, an `ord` that sorted as text, and no
    /// `idx_objects_parent`.
    fn owns_schema(&self) -> bool {
        true
    }

    /// Spawn a `web` cell instance.
    ///
    /// **Corridor duty (phase-5 tripwire)**: the build closure runs on the
    /// initial spawn AND on every respawn. Between the long-running spawn via
    /// `build_long_running_task` and the handle swap in
    /// `colony::handle_cell_died` there must be NO `.await`; every step here is
    /// sync (`open_or_create_cell_db_with_status`, `WebIo::new`, `WebCell::new`,
    /// `DbConn::wrap`, `mpsc::channel`, `tokio::spawn`).
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
            // The frozen `RespawnFn` 3-tuple cannot carry the fresh stop pair
            // back, so the colony is re-notified out of band (I1 / T6b).
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

    /// Hand out a real `RespawnFn` for a `web` cell that booted inactive,
    /// without spawning the task yet (boot-gating: no listener is opened until
    /// a reconnect asks for one). Same construction as `spawn_cell`'s respawn.
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

/// Build the closure that constructs a fresh `web` cell-task.
///
/// Params are parsed once, outside the closure: a params error must surface as
/// a spawn failure, not as a panic on the respawn path.
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
) -> Result<impl Fn() -> SpawnTuple, String> {
    let parsed = WebParams::parse(&params)?;

    let path_cap = path;
    let outputs_cap = outputs_tx;
    let cell_dir_cap = cell_dir;
    let colony_inbox_cap = colony_inbox_tx;
    let blob_cap = blob_store;
    let mailbox_capacity_cap = mailbox_capacity;
    let consumes_cap = consumes;
    let bounds_cap = bounds;

    Ok(move || -> SpawnTuple {
        // 1. Open cell.db (sync).
        let (conn, status) = open_or_create_cell_db_with_status(&cell_dir_cap.join("cell.db"))
            .expect("open cell.db");
        // 1b. Idempotent DDL (sync, every spawn and every respawn).
        setup_web_schema(&conn).expect("setup_web_schema");
        // 1c. Seed, fresh-only. `Created` means this process just made the file,
        //     so there is nothing of anyone's to overwrite; on `Resumed` the
        //     seed is deliberately skipped, or a display would resurrect
        //     objects an operator deleted (the store cell's lesson).
        //
        //     A failure here must not panic: this closure runs on the respawn
        //     path inside the colony's await-free restart barrier, and a panic
        //     there takes the whole colony task, not one cell (the panic-free
        //     hot-path invariant). Every statically detectable seed defect was
        //     already refused by `validate_cell_dir`, so reaching this arm
        //     means the file changed on disk after validation — report loudly
        //     and start with an unseeded but schema-correct database.
        if status == OpenStatus::Created
            && let Err(e) = seed::load_seed_if_present(&conn, &cell_dir_cap)
        {
            tracing::error!(
                path = path_cap.as_str(),
                error = %e,
                "web: seed load failed — the cell starts WITHOUT its seed rows \
                 (the colony is kept alive; fix the seed file and re-create cell.db)"
            );
        }
        // 2. Build the I/O state and the cell (sync). The pages channel is the
        //    one seam between the two halves: the handler publishes rendered
        //    pages, the listener serves whatever was last published. It starts
        //    empty, and `on_start` fills it before the first request can be
        //    answered with anything but a 404.
        let (pages_tx, pages_rx) = watch::channel(Arc::new(PageMap::new()));
        //    The files the cell serves travel the same seam and start empty for
        //    the same reason (GH #393); `on_start` publishes them once, since
        //    no op writes that table.
        let (assets_tx, assets_rx) = watch::channel(Arc::new(AssetMap::new()));
        // Diffs run the other way: the handler pushes, the listener fans out to
        // the sockets it owns. Separate from the substrate's reconfig channel,
        // whose closing is the shutdown signal.
        let (push_tx, push_rx) = mpsc::channel(64);
        let io = WebIo::new(
            parsed.bind.clone(),
            parsed.port,
            path_cap.as_str(),
            pages_rx,
            assets_rx,
            push_rx,
        );
        let cell = WebCell::new(
            path_cap.as_str().to_string(),
            io,
            pages_tx,
            assets_tx,
            push_tx,
        );
        let db = DbConn::wrap(
            conn,
            Some(Duration::from_millis(parsed.external_timeout_ms)),
        );
        let (tx, rx) = mpsc::channel::<Message>(mailbox_capacity_cap);
        // 3. The single long-running spawn site. No `.await` reached this line.
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
