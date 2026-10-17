//! GH #1231 (T6-03): the boot reads no cell's `cell.db`.
//!
//! The planner used to open every cell's `cell.db` read-only (`quick_check`,
//! `schema_version`) — a colony-side read of a database that belongs to one
//! cell alone (overview § Database isolation, "a read counts as access"). The planner now
//! leaves the file alone; the cell checks its OWN database when it opens it,
//! and an unusable database makes that cell `failed` instead of the boot.

use meclaw_colony::{CellFactory, CellFactoryRegistry, RegistryOverlay, plan_bootstrap};
use meclaw_testing::factories::EchoCellFactory;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

fn write(dir: &std::path::Path, rel: &str, body: &[u8]) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// The planner does not look into a cell's database: a file that is no
/// SQLite database at all still plans.
#[test]
fn the_planner_does_not_open_a_cells_database() {
    let td = TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        br#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/a/config.json",
        br#"{"cell":{"type":"echo"},"params":{"emitted_target":"/a"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(td.path(), "main/a/cell.db", b"not a sqlite file");

    let mut factories: CellFactoryRegistry = HashMap::new();
    let echo: Arc<dyn CellFactory> = Arc::new(EchoCellFactory);
    factories.insert("echo".to_string(), echo);

    let plan = plan_bootstrap(td.path(), &factories, &RegistryOverlay::new());
    assert!(
        plan.is_ok(),
        "the planner must not judge a cell's own database: {:?}",
        plan.err()
    );
}

/// The cell's own open is where the check lives now: a database of a schema
/// version this build does not know is refused when the cell opens it.
#[test]
fn a_cell_refuses_its_own_database_of_an_unknown_schema_version() {
    let td = TempDir::new().unwrap();
    let db = td.path().join("cell.db");
    {
        let conn = meclaw_colony::persist::open_or_create_cell_db(&db).unwrap();
        conn.execute(
            "UPDATE meta SET value = '2' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
    }
    assert!(
        meclaw_colony::persist::open_or_create_cell_db(&db).is_err(),
        "schema_version 2 must not open as a cell.db of version 1"
    );
}
