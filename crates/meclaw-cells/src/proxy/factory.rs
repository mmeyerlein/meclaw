//! Phase-10-C: the factory for the `proxy` cell.
//!
//! Opens `cell.db` synchronously, calls `setup_proxy_schema` (idempotent), loads
//! `load_offset` (W9 resume path — on `OpenStatus::Resumed` `load_offset` returns
//! the persisted value, on `Created` it returns 0). Builds the `TelegramClient`.
//! The `make_build` closure is sync and await-free between the DB open and the LR
//! spawn via `build_long_running_task` — conformant with the phase-5 tripwire
//! (cf. `crates/meclaw-cells/src/timer/factory.rs`).

use crate::proxy::cell::ProxyCell;
use crate::proxy::db::{load_offset, setup_proxy_schema};
use crate::proxy::params::ProxyParams;
use crate::proxy::platform::ProxyPlatform;
use crate::proxy::slack::params::SlackParams;
use crate::proxy::telegram::TelegramClient;
use meclaw_colony::persist::cell_db::open_or_create_cell_db_with_status;
use meclaw_colony::{CellFactory, DbConn, RespawnFn, SpawnedCellKind, build_long_running_task};
use meclaw_core::{CellEmission, JsonValue, Message, Path};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
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

/// A platform-erased build closure.
///
/// P12: `make_build` and `make_build_slack` return different opaque `impl Fn`
/// types, so the platform seam boxes them into one type. The boxing is the
/// entire cost of the dispatch — the Telegram branch still calls the unchanged
/// `make_build` and behaves exactly as before.
type BuildFn = Box<dyn Fn() -> SpawnTuple + Send + Sync>;

/// The `proxy` cell factory. Production wiring (`built_in_factories` in
/// `meclaw-cli`) is deferred until the first `examples/` topology using `proxy`,
/// analogous to the phase-10-B limitation (PROGRESS.md l.371-390). The 10-C demo
/// uses the factory directly via `ColonyHandle::register_spawned`.
pub struct ProxyCellFactory;

impl CellFactory for ProxyCellFactory {
    /// Pre-spawn validation. Routes through the same parse path as `spawn_cell`
    /// (parser invariant per the `meclaw_colony::CellFactory` docs).
    ///
    /// P12: the platform seam. `params.platform` selects the parser; absent
    /// means Telegram, so every pre-P12 config validates exactly as before.
    /// The parser invariant holds per branch — the branch chosen here is the
    /// branch `spawn_cell` will take.
    fn validate_params(&self, params: &JsonValue) -> Result<(), String> {
        match crate::proxy::platform::parse_platform(params)? {
            ProxyPlatform::Telegram => ProxyParams::parse(params).map(|_| ()),
            ProxyPlatform::Slack => SlackParams::parse(params).map(|_| ()),
        }
    }

    /// Spawn a `proxy` cell instance.
    ///
    /// **Corridor duty (phase-5 tripwire)**: the `make_build` closure runs on the
    /// initial spawn AND on the respawn (`RespawnFn` is `Fn`, not `FnOnce`).
    /// Between the LR spawn via `build_long_running_task` and setting
    /// `RegistryEntry.handle` in `colony::handle_cell_died` there must be NO
    /// `.await`. All preceding ops are sync
    /// (`open_or_create_cell_db_with_status`, `setup_proxy_schema`,
    /// `load_offset`, `TelegramClient::new`, `ProxyCell::new`, `DbConn::wrap`,
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
        // P12: the platform seam. Telegram takes the unchanged `make_build`.
        let build: BuildFn = match crate::proxy::platform::parse_platform(&params)? {
            ProxyPlatform::Telegram => Box::new(make_build(
                params,
                path,
                outputs_tx,
                cell_dir,
                colony_inbox_tx,
                blob_store,
                mailbox_capacity,
                contract.consumes.clone(),
                contract.write_surface,
            )?),
            ProxyPlatform::Slack => Box::new(make_build_slack(
                params,
                path,
                outputs_tx,
                cell_dir,
                colony_inbox_tx,
                blob_store,
                mailbox_capacity,
                contract.consumes.clone(),
                contract.write_surface,
            )?),
        };

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

    /// Phase-13.5 Slice 4 T7b: hand out a REAL `RespawnFn` for a `proxy` cell
    /// that booted INACTIVE, WITHOUT spawning the initial task (boot-gating
    /// preserved — no I/O loop runs until reconnect). The returned closure is
    /// the SAME construction as `spawn_cell`'s `respawn` (built via the shared
    /// `make_build` helper); an `add_edges` reconnect calls it and the
    /// long-running task starts IMMEDIATELY (spec § Connectivity and activity:
    /// reactivated long-running cells start "immediately"). Restart-inert
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
        let build: BuildFn = match crate::proxy::platform::parse_platform(&params).ok()? {
            ProxyPlatform::Telegram => Box::new(
                make_build(
                    params,
                    path,
                    outputs_tx,
                    cell_dir,
                    colony_inbox_tx,
                    blob_store,
                    mailbox_capacity,
                    contract.consumes.clone(),
                    contract.write_surface,
                )
                .ok()?,
            ),
            ProxyPlatform::Slack => Box::new(
                make_build_slack(
                    params,
                    path,
                    outputs_tx,
                    cell_dir,
                    colony_inbox_tx,
                    blob_store,
                    mailbox_capacity,
                    contract.consumes.clone(),
                    contract.write_surface,
                )
                .ok()?,
            ),
        };
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

/// Build the closure that constructs a fresh `proxy` cell-task. Shared by
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
    write_surface: meclaw_core::WriteSurface,
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
    // bot_token + emit_to are immutable (credential / routing identity) → taken
    // from birth. The mutable fields (base_url, long_poll_timeout_ms,
    // long_poll_request_secs, send_timeout_ms, query_timeout_ms) are rebuilt per
    // (re)spawn from the cell.db overlay (β restore) inside the closure.
    let ProxyParams {
        bot_token, emit_to, ..
    } = ProxyParams::parse(&params)?;

    // Owned clones moved into the multi-call closure.
    let birth_cap = params;
    let path_cap = path;
    let outputs_cap = outputs_tx;
    let cell_dir_cap = cell_dir;
    let bot_token_cap = bot_token;
    let emit_to_cap = emit_to;
    let colony_inbox_cap = colony_inbox_tx;
    let blob_cap = blob_store;
    let mailbox_capacity_cap = mailbox_capacity;
    // Slice 2: the cell's OWN pre-compiled consumes views (Arc-clone).
    let consumes_cap = consumes;
    // GH #260: the substrate half of the write boundary, captured like the
    // consumes views so restart and reconnect carry the same declaration.
    let write_surface_cap = write_surface;

    Ok(move || -> (
        mpsc::Sender<Message>,
        JoinHandle<()>,
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Receiver<()>,
    ) {
        // 1. Open cell.db (sync). OpenStatus is not evaluated — `load_offset`
        //    automatically returns 0 for Created (W9).
        let (conn, _status) =
            open_or_create_cell_db_with_status(&cell_dir_cap.join("cell.db")).expect("open cell.db");
        // 2. Idempotent DDL (sync, outside the corridor).
        setup_proxy_schema(&conn).expect("setup_proxy_schema");
        // 3. Load the cursor (sync). Resume returns the persisted value, Created
        //    returns 0.
        let initial_offset = load_offset(&conn).expect("load_offset");
        // 3b. β restore: effective mutable params = birth ⊕ cell.db overlay
        //     (incl. base_url — mutable, path B).
        let crate::proxy::params::ProxyOverlay {
            base_url,
            long_poll_timeout_ms,
            long_poll_request_secs,
            send_timeout_ms,
            query_timeout_ms,
        } = crate::params_overlay::restore::<crate::proxy::params::ProxyOverlay>(&conn, &birth_cap)
            .expect("restore proxy overlay");
        // 4. Build the TelegramClient (sync) with the effective base_url + the immutable bot_token.
        let client =
            TelegramClient::new(&base_url, &bot_token_cap).expect("TelegramClient::new");
        // 5. Build ProxyCell + DbConn (sync), create the mailbox, then funnel the
        //    LR spawn through `build_long_running_task` — the single LR-spawn
        //    site. The helper mints the peace/stop/death_ack oneshot pairs
        //    internally and returns `(join, peace_rx, stop_tx, death_ack_rx)`. No
        //    `.await` inside the helper → await-free respawn corridor preserved.
        let cell = ProxyCell::new(
            client,
            emit_to_cap.clone(),
            initial_offset,
            long_poll_timeout_ms,
            long_poll_request_secs,
            send_timeout_ms,
            query_timeout_ms,
            base_url,
        );
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
            write_surface_cap,
        );
        (tx, join, peace_rx, stop_tx, death_ack_rx, backstop_rx)
    })
}

/// Build the closure that constructs a fresh Slack-variant `proxy` cell-task.
///
/// Mirrors `make_build` position for position: everything between the `cell.db`
/// open and `build_long_running_task` is sync and await-free, which is what the
/// phase-5 respawn-corridor tripwire requires.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn make_build_slack(
    params: JsonValue,
    path: Path,
    outputs_tx: mpsc::Sender<CellEmission>,
    cell_dir: PathBuf,
    colony_inbox_tx: mpsc::Sender<meclaw_colony::ColonyMsg>,
    blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
    mailbox_capacity: usize,
    consumes: Option<std::sync::Arc<meclaw_core::CompiledConsumes>>,
    write_surface: meclaw_core::WriteSurface,
) -> Result<impl Fn() -> SpawnTuple, String> {
    // Parsed once, outside the closure: a params error must surface as a spawn
    // failure, not as a panic on the respawn path.
    let parsed = SlackParams::parse(&params)?;

    let path_cap = path;
    let outputs_cap = outputs_tx;
    let cell_dir_cap = cell_dir;
    let colony_inbox_cap = colony_inbox_tx;
    let blob_cap = blob_store;
    let mailbox_capacity_cap = mailbox_capacity;
    let consumes_cap = consumes;
    // GH #260: the substrate half of the write boundary, captured like the
    // consumes views so restart and reconnect carry the same declaration.
    let write_surface_cap = write_surface;

    Ok(move || -> SpawnTuple {
        // 1. Open cell.db (sync).
        let (conn, _status) = open_or_create_cell_db_with_status(&cell_dir_cap.join("cell.db"))
            .expect("open cell.db");
        // 2. Idempotent DDL (sync, outside the corridor).
        crate::proxy::slack::db::setup_slack_schema(&conn).expect("setup_slack_schema");
        // 3. Drop dedup rows past their retention window. Spawn is the natural
        //    place: it is sync, it runs on every (re)start, and it keeps the
        //    table from growing without bound over a long-lived bot's life.
        let cutoff = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
            - parsed.envelope_dedup_secs as i64;
        let _ = crate::proxy::slack::db::prune_envelopes(&conn, cutoff);
        // 4. Build the client + cell (sync).
        let client =
            crate::proxy::slack::client::SlackClient::new(&parsed).expect("SlackClient::new");
        let cell = crate::proxy::slack::cell::SlackCell::new(&parsed, client);
        let db = DbConn::wrap(conn, Some(Duration::from_millis(parsed.query_timeout_ms)));
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
            write_surface_cap,
        );
        (tx, join, peace_rx, stop_tx, death_ack_rx, backstop_rx)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    #[test]
    fn validate_params_delegates_to_parse() {
        let f = Arc::new(ProxyCellFactory);
        f.clone()
            .validate_params(&json!({"bot_token": "t", "emit_to": "/x"}))
            .unwrap();
        let err = f.validate_params(&json!({"emit_to": "/x"})).unwrap_err();
        assert!(err.contains("bot_token"));
    }
}
