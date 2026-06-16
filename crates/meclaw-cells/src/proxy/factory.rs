//! Phase-10-C: Factory fuer die `proxy`-Cell.
//!
//! Oeffnet `cell.db` sync, ruft `setup_proxy_schema` (idempotent), laedt
//! `load_offset` (W9: Resume-Pfad — bei `OpenStatus::Resumed` liefert
//! `load_offset` den persistierten Wert; bei `Created` liefert er 0).
//! Baut `TelegramClient`. `make_build`-Closure ist sync + await-frei
//! zwischen DB-Open und dem LR-Spawn via `build_long_running_task` —
//! Phase-5-Tripwire-konform (vgl. `crates/meclaw-cells/src/timer/factory.rs`).

use crate::proxy::cell::ProxyCell;
use crate::proxy::db::{load_offset, setup_proxy_schema};
use crate::proxy::params::ProxyParams;
use crate::proxy::telegram::TelegramClient;
use meclaw_colony::persist::cell_db::open_or_create_cell_db_with_status;
use meclaw_colony::{CellFactory, DbConn, RespawnFn, SpawnedCellKind, build_long_running_task};
use meclaw_core::{CellEmission, JsonValue, Message, Path};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// `proxy`-Cell-Factory. Production-Wiring (`built_in_factories` in
/// `meclaw-cli`) deferred bis erste `examples/`-Topologie mit `proxy`,
/// analog Phase-10-B-Limitation (PROGRESS.md Z.371–390). 10-C-Demo nutzt
/// die Factory direkt via `ColonyHandle::register_spawned`.
pub struct ProxyCellFactory;

impl CellFactory for ProxyCellFactory {
    /// Pre-Spawn-Validierung. Routet ueber denselben Parse-Pfad wie
    /// `spawn_cell` (Parser-Invariante per `meclaw_colony::CellFactory`-Doc).
    fn validate_params(&self, params: &JsonValue) -> Result<(), String> {
        ProxyParams::parse(params).map(|_| ())
    }

    /// Spawn eine `proxy`-Cell-Instanz.
    ///
    /// **Korridor-Pflicht (Phase-5-Tripwire)**: Der `make_build`-Closure laeuft
    /// initial UND beim Respawn (`RespawnFn` ist `Fn`, nicht `FnOnce`).
    /// Zwischen dem LR-Spawn via `build_long_running_task` und der
    /// `RegistryEntry.handle`-Setzung in `colony::handle_cell_died` DARF KEIN
    /// `.await` liegen. Alle vorgelagerten Ops sind sync
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
        let build = make_build(
            params,
            path,
            outputs_tx,
            cell_dir,
            colony_inbox_tx,
            blob_store,
            mailbox_capacity,
            contract.consumes.clone(),
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

    /// Phase-13.5 Slice 4 T7b: hand out a REAL `RespawnFn` for a `proxy` cell
    /// that booted INACTIVE, WITHOUT spawning the initial task (boot-gating
    /// preserved — no I/O loop runs until reconnect). The returned closure is
    /// the SAME construction as `spawn_cell`'s `respawn` (built via the shared
    /// `make_build` helper); an `add_edges` reconnect calls it and the
    /// Long-Running task starts IMMEDIATELY (spec § Konnektivität & Aktivität:
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

/// Build the closure that constructs a fresh `proxy` cell-task. Shared by
/// `spawn_cell` (eager initial spawn) and `build_boot_inactive_respawn`
/// (boot-inactive: respawn only, no initial spawn) so the `RespawnFn`
/// construction has ONE definition. The closure is `Fn` (not `FnOnce` —
/// `RespawnFn` may fire twice) and stays sync + await-free between DB-open and
/// `tokio::spawn` (Phase-5-Tripwire).
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

    Ok(move || -> (
        mpsc::Sender<Message>,
        JoinHandle<()>,
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Receiver<()>,
    ) {
        // 1. Open cell.db (sync). OpenStatus wird nicht ausgewertet —
        //    `load_offset` liefert fuer Created automatisch 0 (W9).
        let (conn, _status) =
            open_or_create_cell_db_with_status(&cell_dir_cap.join("cell.db")).expect("open cell.db");
        // 2. Idempotente DDL (sync, korridor-frei).
        setup_proxy_schema(&conn).expect("setup_proxy_schema");
        // 3. Cursor laden (sync). Resume liefert persistierten Wert;
        //    Created liefert 0.
        let initial_offset = load_offset(&conn).expect("load_offset");
        // 3b. β restore: effective mutable params = birth ⊕ cell.db-Overlay
        //     (incl. base_url — mutable, Weg B).
        let crate::proxy::params::ProxyOverlay {
            base_url,
            long_poll_timeout_ms,
            long_poll_request_secs,
            send_timeout_ms,
            query_timeout_ms,
        } = crate::params_overlay::restore::<crate::proxy::params::ProxyOverlay>(&conn, &birth_cap)
            .expect("restore proxy overlay");
        // 4. TelegramClient bauen (sync) mit effektiver base_url + immutable bot_token.
        let client =
            TelegramClient::new(&base_url, &bot_token_cap).expect("TelegramClient::new");
        // 5. ProxyCell + DbConn bauen (sync), create the mailbox, then funnel the
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
