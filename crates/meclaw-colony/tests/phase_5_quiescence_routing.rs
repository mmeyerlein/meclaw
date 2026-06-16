//! Phase-5-Quieszenz: Routing-Verifikations-Tests (T34/T35/T36/T29).

use meclaw_core::{MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mocks::EchoMockCell;
use meclaw_testing::wait::wait_for_message_log_count;

/// T34: Outputs-Arm-Hop muss em.sender_path tragen, NICHT @external.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t34_outputs_arm_logs_with_sender_path_not_external() {
    let h = ColonyHandle::new();
    h.spawn(Path::new("/a"), || {
        EchoMockCell::new(Path::new("/a")).echo_to(Path::new("/b"))
    })
    .await;
    h.spawn(Path::new("/b"), || EchoMockCell::new(Path::new("/b")))
        .await;
    // A1: /a's echo to /b needs an explicit catch-all out-edge (identity-fallback gone).
    h.add_edge(Uuid::now_v7(), Path::new("/a"), Path::new("/b"))
        .await;
    let msg = MessageBuilder::new(Path::new("/a")).build();
    let trace_id = msg.trace_id.to_string();
    h.send(msg).await;
    let db_path = h.tempdir_path().join("colony.db");
    wait_for_message_log_count(&db_path, &trace_id, 2, std::time::Duration::from_secs(5)).await;
    h.shutdown().await;
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let from_to_b: String = conn
        .query_row(
            "SELECT from_path FROM message_log WHERE trace_id = ? AND to_path = '/b' LIMIT 1",
            [&trace_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        from_to_b, "/a",
        "Outputs-Arm-Hop muss em.sender_path tragen, NICHT @external"
    );
}

/// T35: in 2-Hop-Trace genau 1 @external-Row (die Source-Message).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t35_at_external_appears_only_for_parent_null_messages() {
    let h = ColonyHandle::new();
    h.spawn(Path::new("/a"), || {
        EchoMockCell::new(Path::new("/a")).echo_to(Path::new("/b"))
    })
    .await;
    h.spawn(Path::new("/b"), || EchoMockCell::new(Path::new("/b")))
        .await;
    // A1: /a's echo to /b needs an explicit catch-all out-edge (identity-fallback gone).
    h.add_edge(Uuid::now_v7(), Path::new("/a"), Path::new("/b"))
        .await;
    let msg = MessageBuilder::new(Path::new("/a")).build();
    let trace_id = msg.trace_id.to_string();
    h.send(msg).await;
    let db_path = h.tempdir_path().join("colony.db");
    wait_for_message_log_count(&db_path, &trace_id, 2, std::time::Duration::from_secs(5)).await;
    h.shutdown().await;
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let externals: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM message_log WHERE trace_id = ? AND from_path = '@external'",
            [&trace_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        externals, 1,
        "exactly one @external row in 2-hop trace (the source message)"
    );
}

/// T36: 3-Hop-Trace via CTE-Replay-Logik. Total=3, eine NULL-Root, keine Orphans.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t36_cte_replay_returns_full_chain_no_orphan_one_null_root() {
    let h = ColonyHandle::new();
    h.spawn(Path::new("/a"), || {
        EchoMockCell::new(Path::new("/a")).echo_to(Path::new("/b"))
    })
    .await;
    h.spawn(Path::new("/b"), || {
        EchoMockCell::new(Path::new("/b")).echo_to(Path::new("/c"))
    })
    .await;
    h.spawn(Path::new("/c"), || EchoMockCell::new(Path::new("/c")))
        .await;
    // A1: the /a->/b->/c chain needs explicit catch-all out-edges per hop.
    h.add_edge(Uuid::now_v7(), Path::new("/a"), Path::new("/b"))
        .await;
    h.add_edge(Uuid::now_v7(), Path::new("/b"), Path::new("/c"))
        .await;
    let msg = MessageBuilder::new(Path::new("/a")).build();
    let trace_id = msg.trace_id.to_string();
    h.send(msg).await;
    let db_path = h.tempdir_path().join("colony.db");
    wait_for_message_log_count(&db_path, &trace_id, 3, std::time::Duration::from_secs(5)).await;
    h.shutdown().await;
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    // 1. Total count
    let total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM message_log WHERE trace_id = ?",
            [&trace_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(total, 3, "3-hop trace must have 3 rows");
    // 2. Exactly one NULL-Root
    let null_roots: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM message_log WHERE trace_id = ? AND parent_message_id IS NULL",
            [&trace_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        null_roots, 1,
        "exactly one trace root (parent_message_id IS NULL)"
    );
    // 3. No orphans (non-NULL parents must reference existing message_log.id)
    let orphans: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM message_log m
         WHERE m.trace_id = ?1 AND m.parent_message_id IS NOT NULL
           AND NOT EXISTS (SELECT 1 FROM message_log p WHERE p.id = m.parent_message_id)",
            [&trace_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(orphans, 0, "no orphan hops in trace chain");
}

/// T31: nach Panic+Restart sieht Cell den persistierten Snapshot, NICHT Bootstrap.
/// panic_after=3, 4 Sends, assert counter==4.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t31_cell_sees_last_persisted_state_after_restart_not_bootstrap() {
    use meclaw_colony::factory::CellFactory;
    use meclaw_core::{MessageBuilder, Path, serde_json::json};
    use meclaw_testing::factories::PersistCellFactory;
    use meclaw_testing::wait::{wait_for_cell_db_value, wait_for_spawn_count};
    use std::sync::Arc;
    use std::sync::atomic::AtomicU32;

    let h = ColonyHandle::new();
    let td = tempfile::TempDir::new().unwrap();
    let cell_dir = td.path().join("a");
    std::fs::create_dir(&cell_dir).unwrap();
    let factory = Arc::new(PersistCellFactory {
        spawn_count: Arc::new(AtomicU32::new(0)),
    });
    let spawn_count = factory.spawn_count.clone();
    let spawned = factory
        .clone()
        .spawn_cell(
            Path::new("/a"),
            json!({"panic_after": 3, "terminal": true}),
            h.runtime().outputs_tx,
            cell_dir.clone(),
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            0,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned(Path::new("/a"), spawned).await;
    // Phase-13-K-2: stateful factories now boot Dormant — spawn_count stays 0
    // until the first Wake-Pre-Send. The first message triggers Wake →
    // build_cell_with_open_db → spawn_count == 1.

    // msg1: counter=1, snap, no panic. Wake fires here.
    h.send(MessageBuilder::new(Path::new("/a")).build()).await;
    wait_for_cell_db_value(&cell_dir, "counter", "1", std::time::Duration::from_secs(5)).await;
    wait_for_spawn_count(&spawn_count, 1, std::time::Duration::from_secs(5)).await;
    // msg2: counter=2, snap, no panic.
    h.send(MessageBuilder::new(Path::new("/a")).build()).await;
    wait_for_cell_db_value(&cell_dir, "counter", "2", std::time::Duration::from_secs(5)).await;
    // msg3: counter=3, snap(3) — Pre-Panic-Counter persistiert. Dann panic (kein Output).
    h.send(MessageBuilder::new(Path::new("/a")).build()).await;
    wait_for_cell_db_value(&cell_dir, "counter", "3", std::time::Duration::from_secs(5)).await;
    // Supervisor restart → spawn_count == 2.
    wait_for_spawn_count(&spawn_count, 2, std::time::Duration::from_secs(5)).await;
    // msg4: post-Restart, Cell sieht overlay=3, counter=4, snap(4), no panic
    // (panic_after=3, counter=4 ≠ 3).
    h.send(MessageBuilder::new(Path::new("/a")).build()).await;
    wait_for_cell_db_value(&cell_dir, "counter", "4", std::time::Duration::from_secs(5)).await;
    h.shutdown().await;
    let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).unwrap();
    let v: String = conn
        .query_row(
            "SELECT value FROM system WHERE slot_path = 'counter'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        v, "4",
        "after panic+restart, cell incremented from persisted snapshot (3) to 4, NOT from bootstrap (0→1)"
    );
}

/// T29: after supervisor restart cell.db must be lock-free and counter persisted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t29_wal_lock_free_after_supervisor_restart() {
    use meclaw_colony::factory::CellFactory;
    use meclaw_core::serde_json::json;
    use meclaw_testing::factories::PersistCellFactory;
    use meclaw_testing::wait::{wait_for_cell_db_value, wait_for_spawn_count};
    use std::sync::Arc;
    use std::sync::atomic::AtomicU32;

    let h = ColonyHandle::new();
    let td = tempfile::TempDir::new().unwrap();
    let cell_dir = td.path().join("a");
    std::fs::create_dir(&cell_dir).unwrap();
    let factory = Arc::new(PersistCellFactory {
        spawn_count: Arc::new(AtomicU32::new(0)),
    });
    let spawn_count = factory.spawn_count.clone();
    let spawned = factory
        .clone()
        .spawn_cell(
            Path::new("/a"),
            json!({"panic_after": 1, "terminal": true}),
            h.runtime().outputs_tx,
            cell_dir.clone(),
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            0,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned(Path::new("/a"), spawned).await;
    // Phase-13-K-2: stateful factories now boot Dormant — spawn_count stays 0
    // until the first Wake-Pre-Send. The first message triggers Wake → +1.
    // msg1: counter=1, snap(1), panic (no output emitted). Wake fires here.
    h.send(MessageBuilder::new(Path::new("/a")).build()).await;
    // cell.db.counter == "1" persisted before panic.
    wait_for_cell_db_value(&cell_dir, "counter", "1", std::time::Duration::from_secs(5)).await;
    // After first Wake + supervisor restart: spawn_count == 2.
    wait_for_spawn_count(&spawn_count, 2, std::time::Duration::from_secs(5)).await;
    // msg2: counter=2 (overlay=1 +1), snap(2), no panic (panic_after=1, counter=2 ≠ 1).
    h.send(MessageBuilder::new(Path::new("/a")).build()).await;
    wait_for_cell_db_value(&cell_dir, "counter", "2", std::time::Duration::from_secs(5)).await;
    h.shutdown().await;
    let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).unwrap();
    let v: String = conn
        .query_row(
            "SELECT value FROM system WHERE slot_path = 'counter'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        v, "2",
        "after panic+restart, counter persisted across panic boundary"
    );
}
