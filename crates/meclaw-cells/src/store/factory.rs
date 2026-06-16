//! Phase-9 StoreCellFactory: open_or_create_cell_db_with_status → DDL
//! → seed-if-fresh → DbConn::wrap → cell_task_stateful.

use crate::store::{StoreCell, StoreParams, ddl, seed};
use meclaw_colony::persist::{OpenStatus, open_or_create_cell_db_with_status};
use meclaw_colony::{
    CellFactory, DbConn, RespawnFn, SpawnedCellKind, WakeFn, build_stateful_task_with_peace,
    renotify_stop_wiring,
};
use meclaw_core::{CellEmission, JsonValue, Message, Path};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Phase-9 `store`-Cell factory. Unit-struct (no fields) — all
/// per-instance config lives in `params`.
pub struct StoreCellFactory;

impl CellFactory for StoreCellFactory {
    /// Lazy stateful kind (Dormant) — F1-KH2 kind discriminator: registration
    /// paths may call `spawn_cell` task-free and must install the real WakeFn.
    fn is_lazy(&self) -> bool {
        true
    }

    /// Pre-spawn validation. Same parse path as `spawn_cell`.
    fn validate_params(&self, raw: &JsonValue) -> Result<(), String> {
        StoreParams::parse(raw).map(|_| ())
    }

    /// Spawn a `store`-Cell instance.
    ///
    /// Sequence (per Brainstorm E1+E3+E4):
    /// 1. Parse params (`StoreParams::parse`).
    /// 2. `open_or_create_cell_db_with_status` → `(Connection, OpenStatus)`.
    /// 3. `apply_schema_ddl` (Ad-hoc-DDL from `params.schema`, sync).
    /// 4. If `OpenStatus::Created` → `load_seed_if_present` (sync, fresh-only).
    /// 5. `DbConn::wrap` (with optional `query_timeout` from params).
    /// 6. `tokio::spawn(cell_task_stateful(...))`.
    ///
    /// `RespawnFn` replays steps 2/3/5/6 (NEVER step 4 — fresh-only seed,
    /// brainstorm E4). DDL is idempotent (`CREATE TABLE IF NOT EXISTS`).
    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        raw_params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        cell_dir: std::path::PathBuf,
        contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<meclaw_colony::ColonyMsg>,
        idle_timeout: Option<std::time::Duration>,
        cell_timeout: i64,
        message_timeout: Option<std::time::Duration>,
        blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        // Spawn-time validate (parser-invariant). The effective params are
        // rebuilt from the cell.db overlay at each wake/respawn (β restore);
        // the closures capture the BIRTH params Value (config.json snapshot).
        let _params = StoreParams::parse(&raw_params)?;

        // Phase-13-K-2: KEIN initialer cell-task-Spawn — Mailbox-Paar an Dormant.
        // Seed-on-Created bleibt korrekt: WakeFn ruft `open_or_create_cell_db_with_status`,
        // erster Wake gegen leeres `cell.db` → `OpenStatus::Created` → seed läuft;
        // jeder spätere Wake → `Resumed` → kein Re-Seed. RespawnFn skipt Seed
        // intentional (Restart ≠ fresh, sonst M1-Spec-Verstoß).
        let (sender, receiver) = mpsc::channel::<Message>(mailbox_capacity);

        // RespawnFn — Crash-Restart-Pfad. Connection re-open, DDL reapply
        // (idempotent), Seed-Skip (Resume ≠ fresh).
        let respawn_mailbox_capacity = mailbox_capacity;
        let respawn_cell_dir = cell_dir.clone();
        let respawn_path = path.clone();
        let respawn_outputs = outputs_tx.clone();
        let respawn_birth = raw_params.clone();
        let respawn_inbox_tx = colony_inbox_tx.clone();
        let respawn_blob = blob_store.clone();
        // Slice 2: the cell's OWN pre-compiled consumes views (Arc-clone).
        let respawn_consumes = contract.consumes.clone();
        let respawn: RespawnFn = Box::new(
            move || -> (
                mpsc::Sender<Message>,
                JoinHandle<()>,
                tokio::sync::oneshot::Receiver<()>,
                tokio::sync::oneshot::Receiver<()>,
            ) {
                let (conn, _status) =
                    open_or_create_cell_db_with_status(&respawn_cell_dir.join("cell.db"))
                        .expect("respawn: open_or_create_cell_db_with_status failed");
                // β restore: replay the cell.db params-overlay over birth-params.
                let effective =
                    crate::params_overlay::restore::<StoreParams>(&conn, &respawn_birth)
                        .expect("respawn: restore params from cell.db overlay");
                ddl::apply_schema_ddl(&conn, &effective.schema)
                    .expect("respawn: apply_schema_ddl failed");
                let to = effective
                    .query_timeout_ms
                    .map(std::time::Duration::from_millis);
                let db = DbConn::wrap(conn, to);
                let cell = StoreCell::new(effective);
                let (s, r) = mpsc::channel::<Message>(respawn_mailbox_capacity);
                let (j, peace_rx, stop_tx, death_ack_rx, backstop_rx) = build_stateful_task_with_peace(
                    respawn_path.clone(),
                    r,
                    respawn_outputs.clone(),
                    respawn_inbox_tx.clone(),
                    idle_timeout,
                    message_timeout,
                    cell_timeout,
                    cell,
                    db,
                    respawn_blob.clone(),
                    respawn_consumes.clone(),
                );
                // Phase-13.5 Slice 4 T6: re-notify the colony of the fresh stop
                // pair (the frozen RespawnFn 3-tuple cannot return it). try_send,
                // never await — this closure runs in the await-free respawn
                // corridor (see `renotify_stop_wiring`).
                renotify_stop_wiring(
                    &respawn_inbox_tx,
                    respawn_path.clone(),
                    stop_tx,
                    death_ack_rx,
                );
                (s, j, peace_rx, backstop_rx)
            },
        );

        // WakeFn — Open + DDL (idempotent) + Seed-on-Created + Spawn + Watcher.
        let wake_cell_dir = cell_dir.clone();
        let wake_path = path.clone();
        let wake_outputs = outputs_tx.clone();
        let wake_birth = raw_params.clone();
        let wake_inbox_tx = colony_inbox_tx.clone();
        let wake_watcher_inbox = colony_inbox_tx.clone();
        let wake_blob = blob_store.clone();
        // Slice 2: the cell's OWN pre-compiled consumes views (Arc-clone).
        let wake_consumes = contract.consumes.clone();
        let wake: WakeFn = Box::new(move |recv: mpsc::Receiver<Message>| {
            let (conn, status) = open_or_create_cell_db_with_status(&wake_cell_dir.join("cell.db"))
                .expect("wake: open_or_create_cell_db_with_status failed");
            // β restore: replay the cell.db params-overlay over birth-params.
            let effective = crate::params_overlay::restore::<StoreParams>(&conn, &wake_birth)
                .expect("wake: restore params from cell.db overlay");
            ddl::apply_schema_ddl(&conn, &effective.schema).expect("wake: apply_schema_ddl");
            if status == OpenStatus::Created {
                seed::load_seed_if_present(&conn, &wake_cell_dir, &effective.schema)
                    .expect("wake: load_seed_if_present");
            }
            let to = effective
                .query_timeout_ms
                .map(std::time::Duration::from_millis);
            let db = DbConn::wrap(conn, to);
            let cell = StoreCell::new(effective);
            let (join, peace_rx, stop_tx, death_ack_rx, backstop_rx) =
                build_stateful_task_with_peace(
                    wake_path.clone(),
                    recv,
                    wake_outputs.clone(),
                    wake_inbox_tx.clone(),
                    idle_timeout,
                    message_timeout,
                    cell_timeout,
                    cell,
                    db,
                    wake_blob.clone(),
                    wake_consumes.clone(),
                );
            meclaw_colony::spawn_watcher(
                &wake_watcher_inbox,
                wake_path.clone(),
                join,
                peace_rx,
                backstop_rx,
            );
            // Phase-13.5 Lifecycle-3b Task 7.5: return the woken task's live
            // peace-stop wiring so the colony stores it in the RegistryEntry and
            // a later disconnect can peace-stop the cell + drain its mailbox.
            (stop_tx, death_ack_rx)
        });

        // Phase-13.5 Lifecycle-3b Task 3: placeholder peace-stop ends for the
        // Dormant (lazy-wake) variant — these belong to the PRE-wake state
        // (NotYetSpawned). They are inert (their counterparts are dropped here):
        // a disconnect BEFORE wake just flips active=false. After wake, the
        // colony overwrites the registry's stop wiring with the live pair the
        // WakeFn returns (Task 7.5).
        let (stop_tx, _stop_rx) = tokio::sync::oneshot::channel::<()>();
        let (_death_ack_tx, death_ack_rx) = tokio::sync::oneshot::channel::<()>();
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

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_colony::CellFactory;
    use meclaw_core::Path;
    use std::sync::Arc;

    /// Phase-13-K-2: factory returns `Dormant` — invoke the WakeFn directly to
    /// drive the open/DDL/seed pipeline, then verify the fresh `items` count.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_applies_ddl_and_seed_on_fresh() {
        let td = tempfile::TempDir::new().unwrap();
        let cell_dir = td.path().to_path_buf();
        std::fs::create_dir_all(cell_dir.join("seed")).unwrap();
        std::fs::write(
            cell_dir.join("seed/items.jsonl"),
            r#"{"schema":{"id":"int","name":"text"}}
{"id":1,"name":"a"}
"#,
        )
        .unwrap();
        let raw = meclaw_core::serde_json::json!({
            "schema":{"items":{"id":"int","name":"text"}}
        });
        let (otx, _orx) = tokio::sync::mpsc::channel(8);
        let (itx, _irx) = tokio::sync::mpsc::channel(8);
        let spawned = Arc::new(StoreCellFactory)
            .spawn_cell(
                Path::new("/store"),
                raw,
                otx,
                cell_dir.clone(),
                meclaw_colony::ContractView::default(),
                itx,
                None,
                0,
                None,
                None,
                1000,
            )
            .unwrap();
        let (sender, receiver, wake) = match spawned {
            SpawnedCellKind::Dormant {
                sender,
                receiver,
                wake,
                ..
            } => (sender, receiver, wake),
            SpawnedCellKind::Active { .. } => {
                unreachable!("Phase-13-K-2: stateful Factory liefert Dormant")
            }
        };
        // Wake spawns cell_task_stateful with the supplied receiver. Dropping
        // the sender then closes the mailbox → task ends cleanly.
        wake(receiver);
        drop(sender);
        // Give the spawned task time to flush + close. Poll cell.db until the
        // seed row is visible (DDL + seed run inside the Wake closure synchronously
        // before the task is spawned, so the row is visible immediately).
        for _ in 0..50 {
            if let Ok(conn) = rusqlite::Connection::open(cell_dir.join("cell.db"))
                && let Ok(n) =
                    conn.query_row::<i64, _, _>("SELECT COUNT(*) FROM items", [], |r| r.get(0))
                && n == 1
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("seed not loaded on fresh within 500 ms");
    }

    /// Phase-13-K-2: two consecutive Dormant-Spawn-+-Wake cycles against the same
    /// cell_dir. The second Wake opens the DB with `OpenStatus::Resumed` →
    /// `load_seed_if_present` is skipped. Row count stays at 1.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_skips_seed_on_resume() {
        let td = tempfile::TempDir::new().unwrap();
        let cell_dir = td.path().to_path_buf();
        std::fs::create_dir_all(cell_dir.join("seed")).unwrap();
        std::fs::write(
            cell_dir.join("seed/items.jsonl"),
            r#"{"schema":{"id":"int"}}
{"id":1}
"#,
        )
        .unwrap();
        let raw = meclaw_core::serde_json::json!({
            "schema":{"items":{"id":"int"}}
        });
        // ---- Cycle 1 (fresh): Wake → DDL + seed. ----
        let (otx, _orx) = tokio::sync::mpsc::channel(8);
        let (itx, _irx) = tokio::sync::mpsc::channel(8);
        let s1 = Arc::new(StoreCellFactory)
            .spawn_cell(
                Path::new("/s"),
                raw.clone(),
                otx,
                cell_dir.clone(),
                meclaw_colony::ContractView::default(),
                itx,
                None,
                0,
                None,
                None,
                1000,
            )
            .unwrap();
        let (sender1, receiver1, wake1) = match s1 {
            SpawnedCellKind::Dormant {
                sender,
                receiver,
                wake,
                ..
            } => (sender, receiver, wake),
            SpawnedCellKind::Active { .. } => unreachable!("Dormant expected"),
        };
        wake1(receiver1);
        drop(sender1);
        // Wait until the seed has hit cell.db.
        for _ in 0..50 {
            if let Ok(conn) = rusqlite::Connection::open(cell_dir.join("cell.db"))
                && let Ok(n) =
                    conn.query_row::<i64, _, _>("SELECT COUNT(*) FROM items", [], |r| r.get(0))
                && n == 1
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        // ---- Cycle 2 (resume): Wake → OpenStatus::Resumed → no re-seed. ----
        let (otx2, _orx2) = tokio::sync::mpsc::channel(8);
        let (itx2, _irx2) = tokio::sync::mpsc::channel(8);
        let s2 = Arc::new(StoreCellFactory)
            .spawn_cell(
                Path::new("/s"),
                raw,
                otx2,
                cell_dir.clone(),
                meclaw_colony::ContractView::default(),
                itx2,
                None,
                0,
                None,
                None,
                1000,
            )
            .unwrap();
        let (sender2, receiver2, wake2) = match s2 {
            SpawnedCellKind::Dormant {
                sender,
                receiver,
                wake,
                ..
            } => (sender, receiver, wake),
            SpawnedCellKind::Active { .. } => unreachable!("Dormant expected"),
        };
        wake2(receiver2);
        drop(sender2);
        // Allow some time for the second task to start + close. No new seed row
        // should appear.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "seed runs only on fresh");
    }
}
