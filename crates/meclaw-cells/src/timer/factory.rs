//! Phase-10-B: Factory fuer die `timer`-Cell.
//!
//! Oeffnet `cell.db` sync, ruft `setup_timer_schema` (idempotent), seedet bei
//! `OpenStatus::Created` (Phase-9-Pattern, cell-types.md Z.452). Resume-Pfad
//! re-seedet NIE. `make_build`-Closure ist sync + await-frei zwischen DB-Open
//! und dem LR-Spawn via `build_long_running_task` — Phase-5-Tripwire-konform
//! (vgl. `crates/meclaw-cells/src/llm/factory.rs`).

use crate::timer::cell::TimerCell;
use crate::timer::db::{insert_schedule, load_active_filter_past, setup_timer_schema};
use crate::timer::params::TimerParams;
use crate::timer::schedule::ScheduleRow;
use chrono::Utc;
use meclaw_colony::persist::cell_db::{OpenStatus, open_or_create_cell_db_with_status};
use meclaw_colony::{CellFactory, DbConn, RespawnFn, SpawnedCellKind, build_long_running_task};
use meclaw_core::{CellEmission, JsonValue, Message, Path};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// `timer`-Cell-Factory. Production-Wiring lebt in
/// `crates/meclaw-cli/src/factories.rs` (folgt mit den 10-B-`examples/`-
/// Topologien); 10-B-Demo nutzt diese Factory direkt via
/// `ColonyHandle::register_spawned`.
pub struct TimerCellFactory;

impl CellFactory for TimerCellFactory {
    /// Pre-Spawn-Validierung. Routet ueber denselben Parse-Pfad wie
    /// `spawn_cell` (Parser-Invariante per `meclaw_colony::CellFactory`-Doc).
    fn validate_params(&self, params: &JsonValue) -> Result<(), String> {
        TimerParams::parse(params).map(|_| ())
    }

    /// Spawn eine `timer`-Cell-Instanz. Parser-Invariante via `TimerParams::parse`.
    ///
    /// **Korridor-Pflicht (Phase-5-Tripwire)**: Der `make_build`-Closure laeuft
    /// initial UND beim Respawn nach Panic. Zwischen dem LR-Spawn via
    /// `build_long_running_task` und der `RegistryEntry.handle`-Setzung in
    /// `colony::handle_cell_died` DARF KEIN `.await` liegen. Alle vorgelagerten
    /// Ops sind sync (`open_or_create_cell_db_with_status`, `setup_timer_schema`,
    /// `insert_schedule`, `load_active_filter_past`, `DbConn::wrap`,
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

    /// Phase-13.5 Slice 4 T7b: hand out a REAL `RespawnFn` for a `timer` cell
    /// that booted INACTIVE, WITHOUT spawning the initial task (boot-gating
    /// preserved — no schedule-tick loop runs until reconnect). The returned
    /// closure is the SAME construction as `spawn_cell`'s `respawn` (built via
    /// the shared `make_build` helper); an `add_edges` reconnect calls it and
    /// the Long-Running task starts IMMEDIATELY (spec § Konnektivität &
    /// Aktivität: reactivated Long-Running cells start "sofort").
    /// Restart-inert (`build()`) like the normal respawn.
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

/// Build the closure that constructs a fresh `timer` cell-task. Shared by
/// `spawn_cell` (eager initial spawn) and `build_boot_inactive_respawn`
/// (boot-inactive: respawn only, no initial spawn) so the `RespawnFn`
/// construction has ONE definition. The closure is `Fn` (not `FnOnce` —
/// `RespawnFn` may fire twice) and stays sync + await-free between DB-open and
/// `tokio::spawn` (Phase-5-Tripwire). Seed runs only on `OpenStatus::Created`
/// (Phase-9-Pattern, cell-types.md Z.452); Resume never re-seeds.
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
    let parsed = TimerParams::parse(&params)?;
    let seed_cap: Arc<Vec<ScheduleRow>> = Arc::new(parsed.schedules);
    // β: birth params Value, captured for the per-(re)spawn overlay restore of
    // `query_timeout_ms` (Weg C). Schedules are NOT overlay-managed (ops-driven).
    let birth_cap = params;

    // Owned clones moved into the multi-call closure.
    let path_cap = path;
    let outputs_cap = outputs_tx;
    let cell_dir_cap = cell_dir;
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
        // 1. Open cell.db + OpenStatus (sync).
        let (conn, status) =
            open_or_create_cell_db_with_status(&cell_dir_cap.join("cell.db")).expect("open cell.db");
        // 2. Idempotente DDL (sync, korridor-frei).
        setup_timer_schema(&conn).expect("setup_timer_schema");
        // 3. Seed NUR bei Created (Phase-9-Pattern, cell-types.md Z.452).
        if matches!(status, OpenStatus::Created) {
            for row in seed_cap.iter() {
                insert_schedule(&conn, row).expect("seed insert");
            }
        }
        // 3b. β restore: effective query_timeout_ms = birth ⊕ cell.db-Overlay.
        let query_timeout_ms = crate::params_overlay::restore::<crate::timer::params::TimerOverlay>(
            &conn, &birth_cap,
        )
        .expect("restore timer query_timeout overlay")
        .query_timeout_ms;
        // 4. Active-Set laden + past-onces rausfiltern (sync).
        let active = load_active_filter_past(&conn, Utc::now()).expect("load_active_filter_past");
        // 5. Build TimerCell + DbConn (sync), create the mailbox, then funnel the
        //    LR spawn through `build_long_running_task` — the single LR-spawn
        //    site. The helper mints the peace/stop/death_ack oneshot pairs
        //    internally and returns `(join, peace_rx, stop_tx, death_ack_rx)`. No
        //    `.await` inside the helper → await-free respawn corridor preserved.
        let cell = TimerCell::new(path_cap.clone(), active, query_timeout_ms);
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
    fn validate_params_accepts_empty_and_rejects_cron_at_together() {
        let f = Arc::new(TimerCellFactory);
        f.clone().validate_params(&json!({})).unwrap();
        let err = f
            .validate_params(&json!({
                "schedules": [{ "schedule_id":"0190a3f2-0000-7000-8000-000000000001",
                                "schedule_name":"x", "cron":"* * * * * *", "at":"2099-01-01T00:00:00Z",
                                "emit_to":"/x", "emit_body":{} }]
            }))
            .unwrap_err();
        assert!(err.contains("cron") && err.contains("at"));
    }

    /// T17 — Resume-Pfad: Spawn-Drop-Spawn auf derselben `cell_dir` mit
    /// Seed → genau 1 Row. Beweist: Resumed re-seedet NICHT (Phase-9-Pattern,
    /// cell-types.md Z.452). Sonst lägen nach zweitem Spawn 2 Rows in der DB.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn factory_seeds_only_on_first_spawn_resume_keeps_existing_rows() {
        let td = tempfile::TempDir::new().unwrap();
        let cell_dir = td.path().join("timer");
        std::fs::create_dir_all(&cell_dir).unwrap();

        let params = json!({ "schedules": [
            { "schedule_id":"0190a3f2-0000-7000-8000-0000000000aa",
              "schedule_name":"seed-1", "cron":"*/10 * * * * *",
              "emit_to":"/dst", "emit_body":{} }
        ]});
        let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);

        // Spawn 1: Created → Seed läuft.
        let f = Arc::new(TimerCellFactory);
        let (inbox_tx, _inbox_rx) = mpsc::channel(8);
        let s1 = f
            .clone()
            .spawn_cell(
                Path::new("/t"),
                params.clone(),
                out_tx.clone(),
                cell_dir.clone(),
                meclaw_colony::ContractView::default(),
                inbox_tx.clone(),
                None,
                0,
                None,
                None,
                1000,
            )
            .unwrap();
        let (sender1, join1) = match s1 {
            SpawnedCellKind::Active { sender, join, .. } => (sender, join),
            SpawnedCellKind::Dormant { .. } => unreachable!("Phase-13-G-2: only Active"),
        };
        drop(sender1); // Mailbox-Close → Shutdown.
        tokio::time::timeout(Duration::from_secs(30), join1)
            .await
            .unwrap()
            .unwrap();

        // Spawn 2: Resumed → KEIN Re-Seed (sonst doppelt).
        let s2 = f
            .spawn_cell(
                Path::new("/t"),
                params,
                out_tx,
                cell_dir.clone(),
                meclaw_colony::ContractView::default(),
                inbox_tx,
                None,
                0,
                None,
                None,
                1000,
            )
            .unwrap();
        let (sender2, join2) = match s2 {
            SpawnedCellKind::Active { sender, join, .. } => (sender, join),
            SpawnedCellKind::Dormant { .. } => unreachable!("Phase-13-G-2: only Active"),
        };
        drop(sender2);
        tokio::time::timeout(Duration::from_secs(30), join2)
            .await
            .unwrap()
            .unwrap();

        // Probe direkt mit fresh-Connection: genau 1 Row.
        let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM schedules", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "Resumed darf NICHT re-seeden");
    }
}
