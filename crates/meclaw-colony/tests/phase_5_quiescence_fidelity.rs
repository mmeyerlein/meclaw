//! FIX-3 fidelity test: the route_with_log pre-check must log EXACTLY the
//! delivered hops.
//!
//! Verified across three phase-5 topologies:
//! - 1-Hop: external → /a (terminal)
//! - 2-Hop: external → /a → /b (terminal)
//! - 3-Hop: external → /a → /b → /c (terminal)
//!
//! On a red test (COUNT > expected): route_with_log pre-check divergence. REPORT BACK.

use meclaw_core::{MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mocks::EchoMockCell;
use meclaw_testing::wait::wait_for_message_log_count;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fidelity_1_hop_external_to_a_logs_exactly_1_row() {
    let h = ColonyHandle::new();
    h.spawn(Path::new("/a"), || EchoMockCell::new(Path::new("/a")))
        .await;
    let msg = MessageBuilder::new(Path::new("/a")).build();
    let trace_id = msg.trace_id.to_string();
    h.send(msg).await;
    let db_path = h.tempdir_path().join("colony.db");
    wait_for_message_log_count(&db_path, &trace_id, 1, std::time::Duration::from_secs(5)).await;
    h.shutdown().await;
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM message_log WHERE trace_id = ?",
            [&trace_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "1-Hop topology must log exactly 1 row");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fidelity_2_hop_external_to_a_to_b_logs_exactly_2_rows() {
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
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM message_log WHERE trace_id = ?",
            [&trace_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2, "2-Hop topology must log exactly 2 rows");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fidelity_3_hop_external_to_a_to_b_to_c_logs_exactly_3_rows() {
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
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM message_log WHERE trace_id = ?",
            [&trace_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 3, "3-Hop topology must log exactly 3 rows");
}
