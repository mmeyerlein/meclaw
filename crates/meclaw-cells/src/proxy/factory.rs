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
use crate::proxy::meclaw::cell::MeclawCell;
use crate::proxy::meclaw::client::PeerClient;
use crate::proxy::meclaw::mount::MeclawIo;
use crate::proxy::meclaw::params::MeclawParams;
use crate::proxy::params::ProxyParams;
use crate::proxy::platform::ProxyPlatform;
use crate::proxy::slack::params::SlackParams;
use crate::proxy::telegram::TelegramClient;
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

/// The `proxy` cell factory.
///
/// It carries the process's [`SurfaceRegistry`], because the `meclaw` platform
/// holds a mount on the colony's one listener (ADR-0031); Telegram and Slack
/// ignore it. A fixture hands it a table of its own.
pub struct ProxyCellFactory {
    surfaces: Arc<SurfaceRegistry>,
}

impl ProxyCellFactory {
    /// A factory whose `meclaw` cells mount on `surfaces`.
    pub fn new(surfaces: Arc<SurfaceRegistry>) -> Self {
        Self { surfaces }
    }

    /// The table this factory's `meclaw` cells mount on.
    pub fn surfaces(&self) -> Arc<SurfaceRegistry> {
        Arc::clone(&self.surfaces)
    }
}

impl CellFactory for ProxyCellFactory {
    /// This type's tables are fixed in its own Rust code, so a seed header --
    /// which describes rows, not a schema -- can never describe them (GH #399,
    /// same class as GH #398). Declaring this keeps the mutation staging seeder
    /// out of the database entirely.
    ///
    /// It carries an obligation: a type that declares this must load its own
    /// seed files, because nobody else will. `proxy` has no such loader and
    /// wants none, so the default `validate_cell_dir` refuses a `seed/*.jsonl`
    /// beside it by name instead of ignoring it in silence.
    fn owns_schema(&self) -> bool {
        true
    }

    /// The `cell.type` string, so the refusal above names what an operator
    /// wrote in `config.json` rather than a Rust identifier.
    fn type_name(&self) -> &'static str {
        "proxy"
    }

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
            ProxyPlatform::Meclaw => MeclawParams::parse(params).map(|_| ()),
        }
    }

    /// GH #828: a `meclaw` proxy's secrets are `${VAR}` in the file, never a
    /// literal. The chat platforms keep their convention unchecked here; their
    /// tokens predate the hook, and changing what boots for them is not this
    /// issue's to decide.
    ///
    /// Skipped only where the FILE names a chat platform (literally, or by
    /// leaving `platform` out). A `platform` that is itself `${VAR}` does not
    /// parse here and may resolve to `meclaw`; the check runs then too, and
    /// only looks at `auth`, which no other platform has (review M3).
    fn validate_declared_params(&self, declared: &JsonValue) -> Result<(), String> {
        match crate::proxy::platform::parse_platform(declared) {
            Ok(ProxyPlatform::Telegram | ProxyPlatform::Slack) => Ok(()),
            Ok(ProxyPlatform::Meclaw) | Err(_) => {
                crate::proxy::meclaw::params::validate_declared(declared)
            }
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
                contract.transfer_bounds(),
                contract.ingress_carries_trace,
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
                contract.transfer_bounds(),
                contract.ingress_carries_trace,
            )?),
            ProxyPlatform::Meclaw => Box::new(make_build_meclaw(
                params,
                path,
                outputs_tx,
                cell_dir,
                colony_inbox_tx,
                blob_store,
                mailbox_capacity,
                contract.consumes.clone(),
                contract.transfer_bounds(),
                contract.ingress_carries_trace,
                Arc::clone(&self.surfaces),
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
                    contract.transfer_bounds(),
                    contract.ingress_carries_trace,
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
                    contract.transfer_bounds(),
                    contract.ingress_carries_trace,
                )
                .ok()?,
            ),
            ProxyPlatform::Meclaw => Box::new(
                make_build_meclaw(
                    params,
                    path,
                    outputs_tx,
                    cell_dir,
                    colony_inbox_tx,
                    blob_store,
                    mailbox_capacity,
                    contract.consumes.clone(),
                    contract.transfer_bounds(),
                    contract.ingress_carries_trace,
                    Arc::clone(&self.surfaces),
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
    bounds: meclaw_core::TransferBounds,
    // GH #617 — `contract.ingress.carries_trace`, handed to the LR funnel.
    carries_trace: bool,
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
    let bounds_cap = bounds;
    let carries_cap = carries_trace;

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
            bounds_cap.clone(),
            carries_cap,
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
    bounds: meclaw_core::TransferBounds,
    // GH #617 — `contract.ingress.carries_trace`, handed to the LR funnel.
    carries_trace: bool,
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
    let bounds_cap = bounds;
    let carries_cap = carries_trace;

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
            bounds_cap.clone(),
            carries_cap,
        );
        (tx, join, peace_rx, stop_tx, death_ack_rx, backstop_rx)
    })
}

/// Build the closure that constructs a fresh `meclaw`-variant `proxy` cell-task.
///
/// Mirrors `make_build_slack` position for position, with two differences: no
/// DDL and no overlay restore, because the `cell.db` of this variant stays
/// empty and every key is immutable (A9); and the mount table travels in, as
/// for `web`, because the I/O half registers the mount at the top of each life.
/// Everything between the `cell.db` open and `build_long_running_task` is sync
/// and await-free (phase-5 respawn-corridor tripwire).
#[allow(clippy::too_many_arguments)]
fn make_build_meclaw(
    params: JsonValue,
    path: Path,
    outputs_tx: mpsc::Sender<CellEmission>,
    cell_dir: PathBuf,
    colony_inbox_tx: mpsc::Sender<meclaw_colony::ColonyMsg>,
    blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
    mailbox_capacity: usize,
    consumes: Option<std::sync::Arc<meclaw_core::CompiledConsumes>>,
    bounds: meclaw_core::TransferBounds,
    // GH #617: the arrival of a frame carries the frame's trace and budget.
    carries_trace: bool,
    surfaces: Arc<SurfaceRegistry>,
) -> Result<impl Fn() -> SpawnTuple, String> {
    // Parsed once, outside the closure: a params error is a spawn failure,
    // never a panic on the respawn path. The client too, for the same reason
    // (a TLS init failure); it is cheap to clone per life.
    let parsed = MeclawParams::parse(&params)?;
    let client = PeerClient::with_auth(parsed.auth.as_ref())?;

    let path_cap = path;
    let outputs_cap = outputs_tx;
    let cell_dir_cap = cell_dir;
    let colony_inbox_cap = colony_inbox_tx;
    let blob_cap = blob_store;
    let mailbox_capacity_cap = mailbox_capacity;
    let consumes_cap = consumes;
    let bounds_cap = bounds;
    let carries_cap = carries_trace;
    let surfaces_cap = surfaces;

    Ok(move || -> SpawnTuple {
        // 1. Open cell.db (sync). Nothing is written to it: no schema, no cursor.
        let (conn, _status) = open_or_create_cell_db_with_status(&cell_dir_cap.join("cell.db"))
            .expect("open cell.db");
        // 2. The cell and its mount half (sync).
        let io = MeclawIo::new(&parsed, path_cap.as_str(), Arc::clone(&surfaces_cap));
        let cell = MeclawCell::with_client(&parsed, client.clone()).with_io(io);
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
            bounds_cap.clone(),
            carries_cap,
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
        let f = Arc::new(ProxyCellFactory::new(Arc::new(SurfaceRegistry::new())));
        f.clone()
            .validate_params(&json!({"bot_token": "t", "emit_to": "/x"}))
            .unwrap();
        let err = f.validate_params(&json!({"emit_to": "/x"})).unwrap_err();
        assert!(err.contains("bot_token"));
    }

    /// GH #828 fix round 1 (review M3): the `${VAR}` duty on `auth` secrets
    /// holds whenever the file does not name a chat platform literally. A
    /// `platform` that is itself `${VAR}` resolves to `meclaw` at boot, and
    /// the literal secret beside it would otherwise boot unchecked.
    #[test]
    fn a_literal_secret_is_refused_unless_the_file_names_a_chat_platform() {
        let f = ProxyCellFactory::new(Arc::new(SurfaceRegistry::new()));
        let with = |platform: Option<&str>| {
            let mut p = json!({"auth": {"header": "X-A", "value": "lit-828"}});
            if let Some(v) = platform {
                p["platform"] = json!(v);
            }
            f.validate_declared_params(&p)
        };
        for platform in [Some("meclaw"), Some("${PEER_PLATFORM}"), Some("bogus")] {
            let e = with(platform).expect_err("refused");
            assert!(e.starts_with("auth.value"), "{platform:?}: {e}");
            assert!(!e.contains("lit-828"), "{e}");
        }
        for platform in [None, Some("telegram"), Some("slack")] {
            assert_eq!(
                with(platform),
                Ok(()),
                "{platform:?}: the chat platforms stay unchecked"
            );
        }
    }
}
