//! Phase-6.5 Factory for `MultiUpdateMockCell`. Spawns via `cell_task_stateful`
//! with a Connection opened from `cell.db` per spawn. The `RespawnFn` closure
//! reopens the connection on every restart (M1 resume-with-state path).

use crate::mocks::MultiUpdateMockCell;
use meclaw_colony::{
    CellFactory, RespawnFn, SpawnedCellKind, WakeFn, build_stateful_task_with_peace,
};
use meclaw_core::{CellEmission, JsonValue, Message, Path};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Factory that instantiates `MultiUpdateMockCell` with an eager-opened cell.db.
///
/// Phase-7.5 T3: the per-cell directory is no longer a factory field
/// (Phase-6.5 Test-Hack). The Colony passes `cell_dir` to `spawn_cell` as
/// the substrate-level param introduced in T1, which the `make_pair` closure
/// captures for both initial spawn and every respawn.
///
/// **Ad-hoc-DDL pattern (Phase-9 forward)**: the factory creates the custom
/// `multi_update_log` table via sync `CREATE TABLE IF NOT EXISTS` directly
/// after `open_or_create_cell_db`, before `cell_task_stateful` is spawned.
/// Sync rusqlite call → no new `.await` in the spawn corridor. This is the
/// pattern Phase 9 (`store` / `code` cells with dynamic tables) inherits.
pub struct MultiUpdateMockCellFactory;

impl MultiUpdateMockCellFactory {
    /// Shared parse path for `validate_params` and `spawn_cell`
    /// (parser invariant per the `meclaw_colony::CellFactory` docs).
    fn parse(raw: &JsonValue) -> Result<Path, String> {
        raw.get("sink_target")
            .and_then(|v| v.as_str())
            .map(Path::new)
            .ok_or_else(|| "params.sink_target missing or not a string".to_string())
    }
}

impl CellFactory for MultiUpdateMockCellFactory {
    /// Lazy stateful kind (Dormant) — F1-KH2 kind discriminator: registration
    /// paths may call `spawn_cell` task-free and must install the real WakeFn.
    fn is_lazy(&self) -> bool {
        true
    }

    fn validate_params(&self, raw: &JsonValue) -> Result<(), String> {
        Self::parse(raw).map(|_| ())
    }

    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        raw: JsonValue,
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
        let sink_target = Self::parse(&raw)?;

        // Phase-13-K-2: NO initial spawn — the mailbox pair goes to
        // RegisterDormant; WakeFn/RespawnFn build connection + task on demand.
        let (sender, receiver) = mpsc::channel::<Message>(mailbox_capacity);

        // Shared build-from-receiver closure: open cell.db + DDL + DbConn::wrap +
        // build_stateful_task_with_peace. Used by RespawnFn (fresh channel) and
        // WakeFn (caller-supplied receiver). Ad-hoc-DDL pattern (Phase-9 forward):
        // sync CREATE TABLE IF NOT EXISTS BEFORE DbConn::wrap and BEFORE
        // cell_task_stateful — sync rusqlite call → no .await in spawn corridor.
        let path_for_build = path.clone();
        let sink_target_for_build = sink_target.clone();
        let outputs_tx_for_build = outputs_tx.clone();
        let cell_dir_for_build = cell_dir.clone();
        let inbox_tx_for_build = colony_inbox_tx.clone();
        // Phase-13.5 A8: blob_store captured by the build closure (moved in,
        // cloned per build) → no .await in the await-free respawn corridor.
        let blob_store_for_build = blob_store.clone();
        // Slice 2: the cell's OWN pre-compiled consumes views, captured for
        // every build (initial wake + respawn converge by construction).
        let consumes_for_build = contract.consumes.clone();
        type BuildFromRecv = Arc<
            dyn Fn(
                    mpsc::Receiver<Message>,
                ) -> (
                    JoinHandle<()>,
                    tokio::sync::oneshot::Receiver<()>,
                    tokio::sync::oneshot::Sender<()>,
                    tokio::sync::oneshot::Receiver<()>,
                    tokio::sync::oneshot::Receiver<()>,
                ) + Send
                + Sync,
        >;
        let build_from_recv: BuildFromRecv = Arc::new(move |recv: mpsc::Receiver<Message>| {
            let conn =
                meclaw_colony::persist::open_or_create_cell_db(&cell_dir_for_build.join("cell.db"))
                    .expect("cell.db open_or_create failed");
            conn.execute(
                "CREATE TABLE IF NOT EXISTS multi_update_log (step INTEGER, inserted_at INTEGER)",
                [],
            )
            .expect("multi_update_log schema");
            let db = meclaw_colony::DbConn::wrap(conn, None);
            let cell = MultiUpdateMockCell::new(sink_target_for_build.clone());
            build_stateful_task_with_peace(
                path_for_build.clone(),
                recv,
                outputs_tx_for_build.clone(),
                inbox_tx_for_build.clone(),
                idle_timeout,
                message_timeout,
                cell_timeout,
                cell,
                db,
                blob_store_for_build.clone(),
                consumes_for_build.clone(),
            )
        });

        // RespawnFn: fresh channel + build_from_recv.
        let respawn_arc = build_from_recv.clone();
        let respawn_path = path.clone();
        let respawn_inbox_tx = colony_inbox_tx.clone();
        let respawn_mailbox_capacity = mailbox_capacity;
        let respawn: RespawnFn = Box::new(
            move || -> (
                mpsc::Sender<Message>,
                JoinHandle<()>,
                tokio::sync::oneshot::Receiver<()>,
                tokio::sync::oneshot::Receiver<()>,
            ) {
                let (s, r) = mpsc::channel::<Message>(respawn_mailbox_capacity);
                let (join, peace_rx, stop_tx, death_ack_rx, backstop_rx) = (respawn_arc)(r);
                // Phase-13.5 Slice 4 T6: re-notify the colony of the fresh stop
                // pair (the frozen RespawnFn 4-tuple cannot return it). try_send,
                // never await — this closure runs in the await-free respawn
                // corridor (see `renotify_stop_wiring`).
                meclaw_colony::renotify_stop_wiring(
                    &respawn_inbox_tx,
                    respawn_path.clone(),
                    stop_tx,
                    death_ack_rx,
                );
                (s, join, peace_rx, backstop_rx)
            },
        );

        // WakeFn: caller-supplied receiver + build_from_recv + spawn_watcher.
        let wake_arc = build_from_recv;
        let wake_path = path.clone();
        let wake_watcher_inbox = colony_inbox_tx.clone();
        let wake: WakeFn = Box::new(move |recv: mpsc::Receiver<Message>| {
            let (join, peace_rx, stop_tx, death_ack_rx, backstop_rx) = (wake_arc)(recv);
            meclaw_colony::spawn_watcher(
                &wake_watcher_inbox,
                wake_path.clone(),
                join,
                peace_rx,
                backstop_rx,
            );
            // Phase-13.5 Lifecycle-3b Task 7.5: return live peace-stop wiring.
            (stop_tx, death_ack_rx)
        });

        // Phase-13.5 Lifecycle-3b Task 3: placeholder peace-stop ends for the
        // Dormant (lazy-wake) variant. The task is spawned later in WakeFn, so
        // these ends are inert until Task 4 wires the lazy-wake peace-stop into
        // the registry; the colony drops them in Task 3.
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
    use meclaw_core::serde_json::json;

    #[test]
    fn parse_extracts_sink_target() {
        let raw = json!({"sink_target": "/dst"});
        let p = MultiUpdateMockCellFactory::parse(&raw).unwrap();
        assert_eq!(p.as_str(), "/dst");
    }

    #[test]
    fn parse_missing_sink_target_returns_error() {
        let raw = json!({});
        let err = MultiUpdateMockCellFactory::parse(&raw).unwrap_err();
        assert!(err.contains("sink_target"));
    }

    #[test]
    fn validate_params_returns_error_on_missing_sink_target() {
        let f = MultiUpdateMockCellFactory;
        assert!(f.validate_params(&json!({})).is_err());
    }

    /// Phase-13-K-2: factory returns `Dormant`. Cell.db + DDL happen inside the
    /// WakeFn. Test invokes wake(receiver) directly to drive the open/DDL path.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn spawn_cell_opens_cell_db_and_creates_log_table() {
        let td = tempfile::TempDir::new().unwrap();
        let cell_dir = td.path().join("a");
        std::fs::create_dir(&cell_dir).unwrap();
        let factory: Arc<MultiUpdateMockCellFactory> = Arc::new(MultiUpdateMockCellFactory);
        let (out_tx, _out_rx) = mpsc::channel(8);
        let (inbox_tx, _inbox_rx) = mpsc::channel(8);
        let spawned = factory
            .spawn_cell(
                Path::new("/a"),
                json!({"sink_target": "/sink"}),
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
        let (sender, receiver, wake) = match spawned {
            SpawnedCellKind::Dormant {
                sender,
                receiver,
                wake,
                ..
            } => (sender, receiver, wake),
            SpawnedCellKind::Active { .. } => {
                unreachable!("Phase-13-K-2: Dormant expected")
            }
        };
        wake(receiver);
        let cell_db = cell_dir.join("cell.db");
        assert!(cell_db.exists(), "cell.db was opened by Wake closure");

        // Ad-hoc-DDL-Pattern proof (Phase-9 forward): the custom
        // `multi_update_log` table must exist after Wake — created sync
        // directly after open_or_create_cell_db, before cell_task_stateful.
        let conn = rusqlite::Connection::open(&cell_db).unwrap();
        let table_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='multi_update_log'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            table_exists, 1,
            "multi_update_log table must exist after Wake"
        );
        drop(conn);
        drop(sender);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn spawn_cell_returns_error_on_invalid_params() {
        let td = tempfile::TempDir::new().unwrap();
        let cell_dir = td.path().join("a");
        std::fs::create_dir(&cell_dir).unwrap();
        let f: Arc<MultiUpdateMockCellFactory> = Arc::new(MultiUpdateMockCellFactory);
        let (out_tx, _out_rx) = mpsc::channel(8);
        let (inbox_tx, _inbox_rx) = mpsc::channel(8);
        let result = f.spawn_cell(
            Path::new("/a"),
            json!({}),
            out_tx,
            cell_dir,
            meclaw_colony::ContractView::default(),
            inbox_tx,
            None,
            0,
            None,
            None,
            1000,
        );
        assert!(result.is_err());
    }

    /// Production-path E2E proof of the Ad-hoc-DDL pattern (Phase-9 forward):
    /// the factory runs through open_or_create_cell_db → sync CREATE TABLE
    /// IF NOT EXISTS multi_update_log → cell_task_stateful, and the cell's
    /// handle() actually INSERTs rows into the custom table.
    /// Phase-13-K-2: factory returns Dormant — test drives wake(receiver)
    /// directly, then sends a message.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn spawn_cell_handle_inserts_row_into_multi_update_log() {
        use meclaw_core::{Body, MessageBuilder};
        let td = tempfile::TempDir::new().unwrap();
        let cell_dir = td.path().join("m");
        std::fs::create_dir(&cell_dir).unwrap();
        let factory: Arc<MultiUpdateMockCellFactory> = Arc::new(MultiUpdateMockCellFactory);
        let (out_tx, mut out_rx) = mpsc::channel(8);
        let (inbox_tx, _inbox_rx) = mpsc::channel(8);
        let spawned = factory
            .spawn_cell(
                Path::new("/m"),
                json!({"sink_target": "/sink"}),
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
        let (sender, receiver, wake) = match spawned {
            SpawnedCellKind::Dormant {
                sender,
                receiver,
                wake,
                ..
            } => (sender, receiver, wake),
            SpawnedCellKind::Active { .. } => unreachable!("Phase-13-K-2: Dormant expected"),
        };
        wake(receiver);
        // Send a message that triggers handle() — MultiUpdateMockCell::handle
        // inserts 2 rows (step 1 + step 2) into multi_update_log around an
        // await emit.
        let msg = MessageBuilder::new(Path::new("/m"))
            .body(Body::Inline(json!({"messages": []})))
            .build();
        sender.send(msg).await.unwrap();
        // Drain the 2 expected outputs from the cell.
        let _ = out_rx.recv().await.unwrap();
        let _ = out_rx.recv().await.unwrap();
        drop(sender);
        // Allow the spawned task to flush + close before the DB read.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // Probe: cell.db has 2 rows in multi_update_log.
        let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM multi_update_log", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            count, 2,
            "MultiUpdateMockCell handle inserts 2 rows via production-spawn path"
        );
    }
}
