//! Phase-12-A TDD-Anker: apply_scan_result und boot_load_or_scan sind async.
//! Beweist, dass die δ-Bridge (send_op_try) wegfällt und Bootstrap denselben
//! send_op(.await)-Pfad nutzt wie alle anderen Producer.

use meclaw_colony::ColonyDb;
use meclaw_colony::templates::boot_load_or_scan;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn boot_load_or_scan_is_async_and_uses_send_op_await() {
    let td = tempfile::TempDir::new().unwrap();
    let templates_root = td.path().join("templates");
    std::fs::create_dir_all(&templates_root).unwrap();
    let db = ColonyDb::open(&td.path().join("c.db")).unwrap();
    boot_load_or_scan(&templates_root, &db, false, 1234)
        .await
        .unwrap();
}
