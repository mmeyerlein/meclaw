//! GH #1253: a `catch_up` one-shot whose strike comes due while the colony
//! drains for shutdown is not lost.
//!
//! The colony refuses every new source emission during the drain (GH #47). The
//! timer used to mark the row `completed` first and emit second, so the strike
//! was marked fired and thrown away: the next boot's plan never saw the row
//! again. Now the timer reads the drain (`DrainView`) and holds the strike back
//! before the mark, so the row stays `active` and the restarted timer fires it,
//! late, exactly as after a standstill.
//!
//! Event-bound: `handle_event` emits before it returns, so each strike is judged
//! on its own; no wall clock decides anything.
//!
//! The second test pins the wiring behind it: a timer the boot spawns from the
//! filesystem graph holds the colony's own drain view (review C1).

use chrono::TimeZone;
use meclaw_cells::timer::cell::TimerCell;
use meclaw_cells::timer::db::{insert_schedule, load_active_filter_past, setup_timer_schema};
use meclaw_cells::timer::factory::TimerCellFactory;
use meclaw_cells::timer::io::TimerEvent;
use meclaw_cells::timer::schedule::{ActiveSchedule, ScheduleKind, ScheduleRow};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ContractView, DbConn, DrainView, LongRunningCell,
    SpawnedCellKind, bootstrap_from_filesystem_with_env,
};
use meclaw_core::{CellEmission, JsonValue, OriginSink, Path, Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

fn missed_catch_up_row(id: Uuid, at: chrono::DateTime<chrono::Utc>) -> ScheduleRow {
    ScheduleRow {
        schedule_id: id,
        schedule_name: "gh1253-probe".into(),
        kind: ScheduleKind::At(at),
        emit_to: Path::new("/sink"),
        emit_body: json!({"messages": []}),
        emit_headers: serde_json::Map::new(),
        status: "active".into(),
        iteration_n: 0,
        catch_up: true,
    }
}

fn status_of(db_path: &std::path::Path, id: Uuid) -> String {
    rusqlite::Connection::open(db_path)
        .unwrap()
        .query_row(
            "SELECT status FROM schedules WHERE schedule_id = ?1",
            [id.to_string()],
            |r| r.get(0),
        )
        .unwrap()
}

#[tokio::test]
async fn a_catch_up_strike_in_the_drain_fires_after_the_restart() {
    let td = TempDir::new().unwrap();
    let db_path = td.path().join("cell.db");
    let id = Uuid::now_v7();
    // A fixed moment: the restart below boots a minute after it, so the strike
    // is past due by the plan's own clock, not by the machine's.
    let at = chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        setup_timer_schema(&conn).unwrap();
        insert_schedule(&conn, &missed_catch_up_row(id, at)).unwrap();
    }
    let strike = || TimerEvent::Fire {
        schedule_id: id,
        scheduled_at: at,
        forced: false,
    };
    let (origin_tx, mut origin_rx) = mpsc::channel::<CellEmission>(8);
    let sink = OriginSink::new(origin_tx, Path::new("/timer"), 32);

    // 1. Incarnation one: the colony has entered its drain when the strike
    //    comes due. The strike is held back, the row stays `active`.
    let (drain_tx, drain) = DrainView::pair();
    drain_tx.send_replace(true);
    let plan = vec![ActiveSchedule {
        schedule_id: id,
        kind: ScheduleKind::At(at),
    }];
    let mut cell = TimerCell::new(Path::new("/timer"), plan, 5_000).with_drain(drain);
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let mut db = DbConn::wrap(conn, Some(Duration::from_secs(5)));
    cell.handle_event(strike(), &sink, &mut db).await;
    assert!(
        origin_rx.try_recv().is_err(),
        "a strike that falls into the drain is held back, not emitted into the refusal"
    );
    assert_eq!(
        status_of(&db_path, id),
        "active",
        "a held-back strike leaves its row active for the next start"
    );
    drop(cell);
    drop(db);

    // 2. The restart: the next boot's plan still holds the row, and its strike
    //    fires once, late.
    let booted_at = at + chrono::Duration::seconds(60);
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let active = load_active_filter_past(&conn, booted_at).unwrap();
    assert!(
        active.iter().any(|a| a.schedule_id == id),
        "the restarted plan holds the strike the drain withheld"
    );
    let mut cell = TimerCell::new(Path::new("/timer"), active, 5_000).with_booted_at(booted_at);
    let mut db = DbConn::wrap(conn, Some(Duration::from_secs(5)));
    cell.handle_event(strike(), &sink, &mut db).await;
    let out = origin_rx
        .try_recv()
        .expect("the restarted timer fires the strike the drain held back");
    assert_eq!(out.content["header"]["schedule_id"], id.to_string());
    assert_eq!(out.content["header"]["late"], true, "{:?}", out.content);
    assert_eq!(status_of(&db_path, id), "completed");
}

/// Review C1 probe: forwards everything to the timer factory and reports the
/// drain view each timer it spawns was handed.
struct DrainProbe {
    inner: Arc<dyn CellFactory>,
    seen: mpsc::UnboundedSender<DrainView>,
}

impl CellFactory for DrainProbe {
    fn validate_params(&self, params: &JsonValue) -> Result<(), String> {
        self.inner.validate_params(params)
    }

    fn validate_declared_params(&self, declared: &JsonValue) -> Result<(), String> {
        self.inner.validate_declared_params(declared)
    }

    fn connection_setup(&self) -> Option<meclaw_colony::db_conn::ReopenSetup> {
        self.inner.connection_setup()
    }

    fn validate_cell_dir(
        &self,
        params: &JsonValue,
        cell_dir: &std::path::Path,
    ) -> Result<(), String> {
        self.inner.validate_cell_dir(params, cell_dir)
    }

    fn type_name(&self) -> &'static str {
        self.inner.type_name()
    }

    fn is_lazy(&self) -> bool {
        self.inner.is_lazy()
    }

    fn owns_schema(&self) -> bool {
        self.inner.owns_schema()
    }

    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        cell_dir: std::path::PathBuf,
        contract: ContractView,
        colony_inbox_tx: mpsc::Sender<meclaw_colony::ColonyMsg>,
        idle_timeout: Option<Duration>,
        cell_timeout: i64,
        message_timeout: Option<Duration>,
        blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let _ = self.seen.send(contract.drain.clone());
        self.inner.clone().spawn_cell(
            path,
            params,
            outputs_tx,
            cell_dir,
            contract,
            colony_inbox_tx,
            idle_timeout,
            cell_timeout,
            message_timeout,
            blob_store,
            mailbox_capacity,
        )
    }
}

/// Review C1: the boot path. A timer declared in the filesystem graph is
/// spawned by the boot apply, not by hand; unless it holds the colony's own
/// drain view (and not the never-draining default) it holds nothing back in
/// production. Event-bound: the boot has spawned the timer when it returns,
/// and the shutdown ack comes after the drain entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_timer_booted_from_the_tree_sees_the_colony_drain() {
    let td = TempDir::new().unwrap();
    for (rel, body) in [
        (
            "main/config.json",
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
        ),
        (
            "main/t/config.json",
            r#"{"cell":{"type":"timer","timeout":-1},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        ),
    ] {
        let p = td.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
    let (seen_tx, mut seen_rx) = mpsc::unbounded_channel();
    let probe: Arc<dyn CellFactory> = Arc::new(DrainProbe {
        inner: Arc::new(TimerCellFactory),
        seen: seen_tx,
    });
    let colony =
        ColonyHandle::new_with_factories_at(&td, vec![("timer".to_string(), probe.clone())]);
    let mut factories = CellFactoryRegistry::new();
    factories.insert("timer".to_string(), probe);
    bootstrap_from_filesystem_with_env(td.path(), &factories, &colony.runtime(), None)
        .await
        .expect("the tree boots");
    let booted = seen_rx
        .try_recv()
        .expect("the boot spawned the timer through the probe");
    colony.shutdown().await;
    assert!(
        booted.is_draining(),
        "a timer booted from the tree sees the colony's drain (else it holds nothing back)"
    );
}
