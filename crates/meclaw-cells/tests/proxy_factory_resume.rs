//! Phase-10-C T17 / W9: Cursor ueberlebt Spawn-Drop-Spawn. Beweis-Pattern
//! analogous to timer T17. A direct DB touch between the spawns (instead of a
//! handle_event trigger through a real mailbox), because the test checks the
//! factory's resume semantics, not the event path.

use meclaw_cells::proxy::db::{load_offset, save_offset, setup_proxy_schema};
use meclaw_cells::proxy::factory::ProxyCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::{CellEmission, Path, serde_json::json};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn factory_resumes_cursor_after_spawn_drop_spawn() {
    let td = TempDir::new().unwrap();
    let cell_dir = td.path().join("proxy");
    std::fs::create_dir_all(&cell_dir).unwrap();
    let params = json!({
        "bot_token": "T", "emit_to": "/x",
        "base_url": "http://127.0.0.1:1",  // unused (not a long-poll test)
    });
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let (inbox_tx, _inbox_rx) = mpsc::channel(8);
    let f = Arc::new(ProxyCellFactory);

    // Spawn 1: Created -> load_offset = 0.
    let s1 = f
        .clone()
        .spawn_cell(
            Path::new("/p"),
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
        meclaw_colony::SpawnedCellKind::Active { sender, join, .. } => (sender, join),
        meclaw_colony::SpawnedCellKind::Dormant { .. } => unreachable!(),
    };
    drop(sender1);
    tokio::time::timeout(Duration::from_secs(30), join1)
        .await
        .unwrap()
        .unwrap();

    // Direkter DB-Touch: simulate ein paar Updates-Persists.
    let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).unwrap();
    setup_proxy_schema(&conn).unwrap();
    save_offset(&conn, 4711).unwrap();
    drop(conn);

    // Spawn 2: Resumed -> load_offset = 4711.
    let s2 = f
        .spawn_cell(
            Path::new("/p"),
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
        meclaw_colony::SpawnedCellKind::Active { sender, join, .. } => (sender, join),
        meclaw_colony::SpawnedCellKind::Dormant { .. } => unreachable!(),
    };
    drop(sender2);
    tokio::time::timeout(Duration::from_secs(30), join2)
        .await
        .unwrap()
        .unwrap();

    // Probe directly with a fresh connection.
    let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).unwrap();
    assert_eq!(
        load_offset(&conn).unwrap(),
        4711,
        "Resume must load the persisted cursor"
    );
}
