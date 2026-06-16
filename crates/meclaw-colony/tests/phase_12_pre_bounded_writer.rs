//! Phase 12-Pre TDD-Anker: bounded colony.db-Writer-Channel.
//!
//! Inkarnation 2 (async, post-Migration): nutzt `send_op(...).await`. Beweist
//! die Cap-Invariante (queue_depth bleibt nahe Cap unter kooperativem Block)
//! UND den Positiv-Beweis (alle N Ops nach Drain in der DB).
//!
//! Sampling-Strategie: queue_depth() inline nach jedem .await (kein Observer,
//! kein Arc<ColonyDb> — ColonyDb ist !Sync via rusqlite). multi_thread für
//! CLAUDE.md-Topologie-Test-Konvention (worker_threads = 4).

use meclaw_colony::persist::colony_db::ColonyDb;
use meclaw_colony::persist::writer::ColonyWriteOp;
use meclaw_core::Path;

const BURST_N: usize = 5000;
const CAP_PLUS_BATCH_PLUS_EPS: i64 = 1100; // 1000 cap + 64 BATCH_MAX + ε

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bounded_writer_caps_queue_depth_at_capacity_under_burst() {
    let td = tempfile::TempDir::new().unwrap();
    let db_path = td.path().join("c.db");
    let db = ColonyDb::open(&db_path).unwrap();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let mut max_observed: i64 = 0;
    for i in 0..BURST_N {
        db.send_op(ColonyWriteOp::UpsertRegistry {
            path: Path::new(&format!("/cap-test/{i}")),
            cell_id: format!("{i:032x}"),
            cell_type: "test".to_string(),
            created_at: now,
            updated_at: now,
        })
        .await;
        let d = db.queue_depth();
        if d > max_observed {
            max_observed = d;
        }
    }

    db.shutdown_async().await;

    assert!(
        max_observed <= CAP_PLUS_BATCH_PLUS_EPS,
        "queue_depth max was {max_observed} (expected ≤ {CAP_PLUS_BATCH_PLUS_EPS} \
         = cap 1000 + BATCH_MAX 64 + ε). Bounded Backpressure greift nicht."
    );

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let cnt: i64 = conn
        .query_row("SELECT COUNT(*) FROM registry", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        cnt, BURST_N as i64,
        "expected {BURST_N} rows in registry after drain, got {cnt}"
    );
}
