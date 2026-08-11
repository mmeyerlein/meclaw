//! Issue #56 — a broken `store` seed file must fail the CELL, never the process.
//!
//! `load_seed_if_present` used to sit behind an `.expect` inside the store
//! factory's `WakeFn`. That closure is invoked from the colony task (the
//! routing/dispatch path), so the panic did not take down one cell, it took
//! down the whole colony process (the panic-free colony hot path invariant,
//! A1′ class).
//!
//! Two locks:
//! 1. `spawn_cell` rejects a statically broken seed with a named factory error
//!    (`Err`, no panic) — the supervisor/mutation caller handles it like every
//!    other factory error.
//! 2. A seed file broken AFTER a clean spawn (the only path left once validate
//!    and spawn both gate it) must not panic the wake.

use meclaw_colony::{CellFactory, SpawnedCellKind};
use meclaw_core::Path;
use std::sync::Arc;

const VALID_SEED: &str = r#"{"schema":{"id":"int","name":"text"}}
{"id":1,"name":"a"}
"#;

/// Headerless seed — the reproducer from the issue (line 1 removed).
const HEADERLESS_SEED: &str = r#"{"id":1,"name":"a"}
"#;

fn store_params() -> meclaw_core::JsonValue {
    meclaw_core::serde_json::json!({
        "schema": {"items": {"id": "int", "name": "text"}}
    })
}

fn write_seed(cell_dir: &std::path::Path, body: &str) {
    std::fs::create_dir_all(cell_dir.join("seed")).unwrap();
    std::fs::write(cell_dir.join("seed/items.jsonl"), body).unwrap();
}

fn spawn(cell_dir: &std::path::Path) -> Result<SpawnedCellKind, String> {
    let (otx, orx) = tokio::sync::mpsc::channel(8);
    let (itx, irx) = tokio::sync::mpsc::channel(8);
    // Keep the receivers alive for the duration of the call.
    let res = Arc::new(meclaw_cells::store::StoreCellFactory).spawn_cell(
        Path::new("/notes"),
        store_params(),
        otx,
        cell_dir.to_path_buf(),
        meclaw_colony::ContractView::default(),
        itx,
        None,
        0,
        None,
        None,
        1000,
    );
    drop(orx);
    drop(irx);
    res
}

/// Lock 1: the factory refuses to spawn — named error, no panic, caller lives.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn seed_missing_header_fails_cell_not_process() {
    let td = tempfile::TempDir::new().unwrap();
    write_seed(td.path(), HEADERLESS_SEED);
    let err = spawn(td.path()).err().expect(
        "a store cell with a headerless seed file must fail to spawn instead of \
         panicking later at wake",
    );
    let lower = err.to_lowercase();
    assert!(
        lower.contains("seed"),
        "the factory error must name the seed file; got: {err}"
    );
    assert!(
        lower.contains("schema"),
        "the factory error must name the missing schema header; got: {err}"
    );
}

/// A header that does not cover every declared column fails the same way.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn seed_column_mismatch_fails_cell_not_process() {
    let td = tempfile::TempDir::new().unwrap();
    write_seed(
        td.path(),
        r#"{"schema":{"id":"int"}}
{"id":1}
"#,
    );
    let err = spawn(td.path())
        .err()
        .expect("a seed header missing a declared column must fail the spawn");
    assert!(
        err.to_lowercase().contains("seed"),
        "the factory error must name the seed file; got: {err}"
    );
}

/// Control: a well-formed seed still spawns, and the wake still seeds the DB.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn valid_seed_still_spawns_and_seeds() {
    let td = tempfile::TempDir::new().unwrap();
    write_seed(td.path(), VALID_SEED);
    let spawned = spawn(td.path()).expect("a well-formed seed file must still spawn");
    let SpawnedCellKind::Dormant {
        sender,
        receiver,
        wake,
        ..
    } = spawned
    else {
        unreachable!("the stateful store factory returns Dormant");
    };
    wake(receiver);
    drop(sender);
    for _ in 0..100 {
        if let Ok(conn) = rusqlite::Connection::open(td.path().join("cell.db"))
            && let Ok(n) =
                conn.query_row::<i64, _, _>("SELECT COUNT(*) FROM items", [], |r| r.get(0))
            && n == 1
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("seed row not visible within 1s");
}

/// Lock 2 (regression lock): the seed file is broken AFTER a clean spawn, so the
/// static gate cannot see it. The wake must NOT panic — a panic here runs inside
/// the colony task and kills every cell in the colony, not just this one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn seed_broken_after_spawn_does_not_panic_the_wake() {
    let td = tempfile::TempDir::new().unwrap();
    write_seed(td.path(), VALID_SEED);
    let spawned = spawn(td.path()).expect("clean spawn with a well-formed seed");
    // Now corrupt the seed on disk — exactly what the wake would have panicked on.
    write_seed(td.path(), HEADERLESS_SEED);
    let SpawnedCellKind::Dormant {
        sender,
        receiver,
        wake,
        ..
    } = spawned
    else {
        unreachable!("the stateful store factory returns Dormant");
    };
    // The wake runs synchronously in the caller (the colony task in production).
    // Reaching the next line at all is the assertion.
    let _wiring = wake(receiver);
    drop(sender);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let conn = rusqlite::Connection::open(td.path().join("cell.db"))
        .expect("cell.db must exist — the wake completed instead of aborting the process");
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))
        .expect("the DDL must still have been applied");
    assert_eq!(n, 0, "a rejected seed inserts nothing");
}
