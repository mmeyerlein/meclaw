//! Phase-10-D: the factory for the `mcp` cell.
//!
//! Opens `cell.db` synchronously, calls `setup_mcp_schema` (idempotent), builds
//! the `McpClient`. The `make_build` closure is sync and await-free between the
//! DB open and the LR spawn via `build_long_running_task` —
//! conformant with the phase-5 tripwire (cf. `crates/meclaw-cells/src/proxy/factory.rs`).

use crate::mcp::cell::McpCell;
use crate::mcp::db::setup_mcp_schema;
use crate::mcp::params::{McpParams, McpTransport};
use crate::mcp::wire::McpClient;
use meclaw_colony::persist::cell_db::open_or_create_cell_db_with_status;
use meclaw_colony::{CellFactory, DbConn, RespawnFn, SpawnedCellKind, build_long_running_task};
use meclaw_core::{CellEmission, Message, Path};
use serde_json::Value as JsonValue;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// `mcp`-Cell-Factory. Production-Wiring (`built_in_factories` in
/// `meclaw-cli`) is deferred until the first `examples/` topology using `mcp`,
/// analog Phase-10-B/-C-Limitation. Demos in
/// `crates/meclaw-cells/tests/phase_10d_mcp_demo.rs` nutzen die Factory
/// direkt via `ColonyHandle::register_spawned`.
pub struct McpCellFactory;

impl CellFactory for McpCellFactory {
    /// This type's tables are fixed in its own Rust code, so a seed header --
    /// which describes rows, not a schema -- can never describe them (GH #399,
    /// same class as GH #398). Declaring this keeps the mutation staging seeder
    /// out of the database entirely.
    ///
    /// It carries an obligation: a type that declares this must load its own
    /// seed files, because nobody else will. `mcp` has no such loader and
    /// wants none, so the default `validate_cell_dir` refuses a `seed/*.jsonl`
    /// beside it by name instead of ignoring it in silence.
    fn owns_schema(&self) -> bool {
        true
    }

    /// The `cell.type` string, so the refusal above names what an operator
    /// wrote in `config.json` rather than a Rust identifier.
    fn type_name(&self) -> &'static str {
        "mcp"
    }

    /// Pre-spawn validation. Routes through the same parse path as
    /// `spawn_cell` (Parser-Invariante per `CellFactory`-Doc).
    fn validate_params(&self, params: &JsonValue) -> Result<(), String> {
        McpParams::parse(params).map(|_| ())
    }

    /// Spawn an `mcp` cell instance.
    ///
    /// **Corridor duty (phase-5 tripwire)**: the `make_build` closure runs on the
    /// initial spawn AND on the respawn (`RespawnFn` is `Fn`, not `FnOnce`).
    /// Between the LR spawn via `build_long_running_task` and setting
    /// `RegistryEntry.handle` in `colony::handle_cell_died` there must be NO
    /// `.await`. All preceding ops are sync
    /// (`open_or_create_cell_db_with_status`, `setup_mcp_schema`,
    /// `McpClient::new`, `McpCell::new`, `DbConn::wrap`,
    /// `mpsc::channel`, `tokio::spawn`).
    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        cell_dir: PathBuf,
        contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<meclaw_colony::ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<std::time::Duration>,
        blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        // Captured for the restart-side stop-wiring re-notify (I1 / T6b) — taken
        // before `make_build` consumes `path`/`colony_inbox_tx`.
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

        // Initial spawn → `build_long_running_task` (inside `build`) creates the
        // live stop/death_ack/peace ends internally and hands them back via the
        // 5-tuple. The funnel is the single LR-spawn site (P6 message_timeout
        // wrapper lands there later).
        let (sender, join, peace_rx, stop_tx, death_ack_rx, backstop_rx) = build();
        // Restart-side (crash-restart / reconnect-eager): re-spawn — `build()`
        // mints a FRESH live stop pair internally — and re-notify the colony so
        // `entry.stop_tx` is restored (I1 / T6b). The frozen `RespawnFn` 3-tuple
        // cannot return the pair, so the closure hands it back via
        // `renotify_stop_wiring` (try_send, non-blocking — runs inside the
        // await-free respawn corridor).
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

    /// Phase-13.5 Slice 4 T7b: hand out a REAL `RespawnFn` for an `mcp` cell
    /// that booted INACTIVE, WITHOUT spawning the initial task (boot-gating
    /// preserved — no MCP I/O loop runs until reconnect). The returned closure
    /// is the SAME construction as `spawn_cell`'s `respawn` (built via the
    /// shared `make_build` helper); an `add_edges` reconnect calls it and the
    /// long-running task starts IMMEDIATELY (spec § Connectivity and activity:
    /// reactivated Long-Running cells start "sofort"). Restart-inert
    /// (`build()`) like the normal respawn.
    fn build_boot_inactive_respawn(
        self: Arc<Self>,
        path: Path,
        params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        cell_dir: PathBuf,
        contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<meclaw_colony::ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<std::time::Duration>,
        blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
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
        // No initial `build(...)` call here → boot-gating: the inactive cell's
        // task is not spawned until the reconnect arm invokes this closure. When
        // invoked, build with a FRESH live stop pair and re-notify so a later
        // disconnect can peace-stop the reconnected cell (I1 / T6b).
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

/// Build the closure that constructs a fresh `mcp` cell-task. Shared by
/// `spawn_cell` (eager initial spawn) and `build_boot_inactive_respawn`
/// (boot-inactive: respawn only, no initial spawn) so the `RespawnFn`
/// construction has ONE definition. The closure is `Fn` (not `FnOnce` —
/// `RespawnFn` may fire twice) and stays sync + await-free between DB-open and
/// `tokio::spawn` (phase-5 tripwire).
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn make_build(
    params: JsonValue,
    path: Path,
    outputs_tx: mpsc::Sender<CellEmission>,
    cell_dir: PathBuf,
    colony_inbox_tx: mpsc::Sender<meclaw_colony::ColonyMsg>,
    blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
    mailbox_capacity: usize,
    consumes: Option<std::sync::Arc<meclaw_core::CompiledConsumes>>,
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
    // The transport (endpoint+bearer resp. the child spec) is immutable
    // credential/identity material and is taken from birth. Only
    // external_timeout_ms + query_timeout_ms are overlay-effective and get
    // rebuilt per (re)spawn from the cell.db overlay (β restore) below.
    let McpParams { transport, .. } = McpParams::parse(&params)?;
    let provider_key = provider_key_from_path(&path);

    // Owned clones moved into the multi-call closure.
    let birth_cap = params;
    let path_cap = path;
    let outputs_cap = outputs_tx;
    let cell_dir_cap = cell_dir;
    let transport_cap = transport;
    let provider_key_cap = provider_key;
    let colony_inbox_cap = colony_inbox_tx;
    let blob_cap = blob_store;
    let mailbox_capacity_cap = mailbox_capacity;
    // Slice 2: the cell's OWN pre-compiled consumes views (Arc-clone).
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
        // 1. Open cell.db (sync). OpenStatus is discarded — the cache is
        //    idempotently re-discoverable (PK overwrite).
        let (conn, _status) =
            open_or_create_cell_db_with_status(&cell_dir_cap.join("cell.db")).expect("open cell.db");
        // 2. Idempotent DDL (sync, outside the corridor).
        setup_mcp_schema(&conn).expect("setup_mcp_schema");
        // 2b. β restore: effective timeouts = birth ⊕ cell.db-Overlay.
        let crate::mcp::params::McpOverlay {
            external_timeout_ms,
            query_timeout_ms,
        } = crate::params_overlay::restore::<crate::mcp::params::McpOverlay>(&conn, &birth_cap)
            .expect("restore mcp timeouts overlay");
        // 3. Build the cell for the configured transport (sync). http builds
        //    its reqwest client here; stdio builds NOTHING — the child process
        //    is spawned by the I/O sub-task, so a respawn always starts from a
        //    clean slate and the corridor stays await-free.
        let cell = match &transport_cap {
            McpTransport::Http { endpoint, bearer } => {
                let client = McpClient::new(endpoint, bearer.clone()).expect("McpClient::new");
                McpCell::new(
                    client,
                    external_timeout_ms,
                    query_timeout_ms,
                    provider_key_cap.clone(),
                )
            }
            McpTransport::Stdio { spec } => McpCell::new_stdio(
                spec.clone(),
                external_timeout_ms,
                query_timeout_ms,
                provider_key_cap.clone(),
            ),
            // GH #489: no provider named. Builds no client and starts no
            // handshake — the cell exists, holds its `cell.db` and answers
            // `endpoint_unset`.
            McpTransport::Unset => McpCell::new_unset(
                external_timeout_ms,
                query_timeout_ms,
                provider_key_cap.clone(),
            ),
        };
        // 4. DbConn bauen (sync), create the mailbox, then funnel the LR spawn
        //    through `build_long_running_task` — the single LR-spawn site. The
        //    helper mints the peace/stop/death_ack oneshot pairs internally and
        //    returns `(join, peace_rx, stop_tx, death_ack_rx)`. No `.await`
        //    inside the helper → await-free respawn corridor preserved.
        let db = DbConn::wrap(conn, Some(Duration::from_millis(query_timeout_ms)));
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

/// Derives the provider key from the cell path. A leading `/` is removed, inner
/// `/` characters are replaced by `_`.
///
/// Example: `/main/mcp` → `main_mcp`. Used in T21 in `spawn_cell`
/// konsumiert (System-Tools-Slot-Prefix).
pub(crate) fn provider_key_from_path(path: &Path) -> String {
    path.as_str().trim_start_matches('/').replace('/', "_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validate_params_delegates_to_parse() {
        let f = Arc::new(McpCellFactory);
        f.clone()
            .validate_params(&json!({"endpoint": "https://x.example/rpc"}))
            .unwrap();
        // GH #489: no endpoint is the unnamed-provider state, not an error.
        f.validate_params(&json!({})).unwrap();
        // A transport nobody implements is still a refusal.
        let err = f
            .validate_params(&json!({"transport": "carrier-pigeon"}))
            .unwrap_err();
        assert!(err.contains("transport"));
    }

    #[test]
    fn provider_key_strips_leading_slash_and_replaces_inner() {
        let p = Path::new("/main/mcp");
        assert_eq!(provider_key_from_path(&p), "main_mcp");
        let p2 = Path::new("/mcp");
        assert_eq!(provider_key_from_path(&p2), "mcp");
    }
}
