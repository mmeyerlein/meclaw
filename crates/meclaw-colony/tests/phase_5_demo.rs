//! Phase-5-Demo (T37): End-to-End-Restore über die Boot-Grenze.
//!
//! Topologie: 3 PersistMockCells (/a, /b, /c) als 3-Hop-Chain via echo_to.
//! /c terminal (kein Output). FirstBoot 3 Inputs → counter=3 in allen drei.
//! Reboot 3 Inputs → counter=6 (Restore-Beweis: Overlay aus FirstBoot-Snapshot,
//! NICHT Bootstrap-0).
//!
//! Trace/CTE-Breite ist an Q4/Q6/Q7 delegiert; Demo macht hier nur Counter-Restore.

use meclaw_colony::factory::CellFactory;
use meclaw_core::{MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::PersistCellFactory;
use meclaw_testing::wait::wait_for_cell_db_value;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;

async fn spawn_persist_cell(
    h: &ColonyHandle,
    path: Path,
    cell_dir: &std::path::Path,
    params: meclaw_core::JsonValue,
) {
    std::fs::create_dir_all(cell_dir).unwrap();
    let factory = Arc::new(PersistCellFactory {
        spawn_count: Arc::new(AtomicU32::new(0)),
    });
    let spawned = factory
        .spawn_cell(
            path.clone(),
            params,
            h.runtime().outputs_tx,
            cell_dir.to_path_buf(),
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            0,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned(path, spawned).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_5_demo_first_boot_to_reboot_restore() {
    let td = tempfile::TempDir::new().unwrap();
    let dir_a = td.path().join("a");
    let dir_b = td.path().join("b");
    let dir_c = td.path().join("c");

    // ---- FirstBoot ----
    let h1 = ColonyHandle::new();
    spawn_persist_cell(
        &h1,
        Path::new("/a"),
        &dir_a,
        meclaw_core::serde_json::json!({"echo_to": "/b"}),
    )
    .await;
    spawn_persist_cell(
        &h1,
        Path::new("/b"),
        &dir_b,
        meclaw_core::serde_json::json!({"echo_to": "/c"}),
    )
    .await;
    spawn_persist_cell(
        &h1,
        Path::new("/c"),
        &dir_c,
        meclaw_core::serde_json::json!({"terminal": true}),
    )
    .await;

    // A1: the /a->/b->/c echo chain needs explicit catch-all out-edges per hop
    // (the implicit identity-fallback is gone). Wired on every boot (no edge
    // persistence — colony.db is fresh per ColonyHandle).
    h1.add_edge(Uuid::now_v7(), Path::new("/a"), Path::new("/b"))
        .await;
    h1.add_edge(Uuid::now_v7(), Path::new("/b"), Path::new("/c"))
        .await;

    // 3 Source-Messages an /a → 3-Hop-Cascade.
    for _ in 0..3 {
        h1.send(MessageBuilder::new(Path::new("/a")).build()).await;
    }
    // Counter == 3 in allen drei Cells nach 3 Inputs.
    wait_for_cell_db_value(&dir_a, "counter", "3", std::time::Duration::from_secs(10)).await;
    wait_for_cell_db_value(&dir_b, "counter", "3", std::time::Duration::from_secs(10)).await;
    wait_for_cell_db_value(&dir_c, "counter", "3", std::time::Duration::from_secs(10)).await;
    h1.shutdown().await;

    // ---- Reboot ----
    // Neue ColonyHandle (neue colony.db). Cells re-spawnen aus cell.db (overlay).
    let h2 = ColonyHandle::new();
    spawn_persist_cell(
        &h2,
        Path::new("/a"),
        &dir_a,
        meclaw_core::serde_json::json!({"echo_to": "/b"}),
    )
    .await;
    spawn_persist_cell(
        &h2,
        Path::new("/b"),
        &dir_b,
        meclaw_core::serde_json::json!({"echo_to": "/c"}),
    )
    .await;
    spawn_persist_cell(
        &h2,
        Path::new("/c"),
        &dir_c,
        meclaw_core::serde_json::json!({"terminal": true}),
    )
    .await;
    // A1: re-wire the chain edges on reboot (fresh colony.db, no edge persistence).
    h2.add_edge(Uuid::now_v7(), Path::new("/a"), Path::new("/b"))
        .await;
    h2.add_edge(Uuid::now_v7(), Path::new("/b"), Path::new("/c"))
        .await;
    // 3 weitere Sends — Cells starten mit overlay=3 aus FirstBoot.
    for _ in 0..3 {
        h2.send(MessageBuilder::new(Path::new("/a")).build()).await;
    }
    wait_for_cell_db_value(&dir_a, "counter", "6", std::time::Duration::from_secs(10)).await;
    wait_for_cell_db_value(&dir_b, "counter", "6", std::time::Duration::from_secs(10)).await;
    wait_for_cell_db_value(&dir_c, "counter", "6", std::time::Duration::from_secs(10)).await;
    h2.shutdown().await;

    // ---- Final Assert: counter==6 in allen drei ----
    for dir in &[&dir_a, &dir_b, &dir_c] {
        let conn = rusqlite::Connection::open(dir.join("cell.db")).unwrap();
        let v: String = conn
            .query_row(
                "SELECT value FROM system WHERE slot_path = 'counter'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            v, "6",
            "cell.db.counter at {dir:?} must be 6 (3 FirstBoot + 3 Reboot)"
        );
    }
}
