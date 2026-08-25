//! W8 Task 4 (GH #380): the `web` cell's own database, and its seed.
//!
//! Four tables — objects, components, pages, assets — created at spawn and
//! seeded **only** when the database was actually created. That last part is
//! the whole point of the test below: the store cell paid for this lesson
//! (`store/seed.rs`, fresh-only per `OpenStatus::Created`), and a display that
//! re-seeded on every wake would silently resurrect objects an operator had
//! deleted, or duplicate every row that carries no primary key.

use meclaw_cells::web::WebCellFactory;
use meclaw_colony::{CellFactory, ContractView, SpawnedCellKind};
use meclaw_core::{CellEmission, Path, serde_json::json};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::sync::mpsc;

/// A port nothing is listening on.
fn free_port() -> u16 {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    let p = l.local_addr().expect("addr").port();
    drop(l);
    p
}

/// Write the three seed files this test uses. Each carries the schema header
/// the convention requires on line 1 — the same shape the store uses, so a
/// person who has seen one seed directory has seen them all.
fn write_seed(cell_dir: &std::path::Path) {
    let seed = cell_dir.join("seed");
    std::fs::create_dir_all(&seed).expect("create seed dir");
    std::fs::write(
        seed.join("components.jsonl"),
        concat!(
            r#"{"schema":{"name":"text","template":"text","prop_schema":"text","editable":"text","layer":"text"}}"#,
            "\n",
            r#"{"name":"text","template":"<p>{{body}}</p>","prop_schema":"{\"body\":\"text\"}","editable":"[]","layer":"content"}"#,
            "\n"
        ),
    )
    .expect("write components seed");
    std::fs::write(
        seed.join("pages.jsonl"),
        concat!(
            r#"{"schema":{"route":"text","root":"text","title":"text"}}"#,
            "\n",
            r#"{"route":"/","root":"root-1","title":"Seeded"}"#,
            "\n"
        ),
    )
    .expect("write pages seed");
    std::fs::write(
        seed.join("objects.jsonl"),
        concat!(
            r#"{"schema":{"id":"text","parent":"text","component":"text","ord":"int","props":"text"}}"#,
            "\n",
            r#"{"id":"root-1","parent":null,"component":"text","ord":0,"props":"{\"body\":\"hello\"}"}"#,
            "\n"
        ),
    )
    .expect("write objects seed");
}

/// Spawn a web cell on `cell_dir`, hold it briefly, then stop it.
///
/// The handles are held for the length of the closure and dropped after: that
/// drop is what closes the mailbox and brings the listener down, so the next
/// spawn in the same test can reuse the port.
async fn spawn_and_settle(cell_dir: &std::path::Path, port: u16) {
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let (inbox_tx, _inbox_rx) = mpsc::channel(8);
    let spawned = Arc::new(WebCellFactory)
        .spawn_cell(
            Path::new("/web"),
            json!({ "port": port }),
            out_tx,
            cell_dir.to_path_buf(),
            ContractView::default(),
            inbox_tx,
            None,
            -1,
            None,
            None,
            64,
        )
        .expect("spawn");
    let SpawnedCellKind::Active { join, sender, .. } = spawned else {
        panic!("web cells spawn Active");
    };
    // Wait for the schema to exist rather than for a fixed duration: the DDL
    // runs synchronously inside the build closure, so it is there as soon as
    // the file is.
    let db = cell_dir.join("cell.db");
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline && !db.exists() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    drop(sender);
    join.abort();
    // Let the abort take effect before the caller opens the file itself.
    tokio::time::sleep(Duration::from_millis(50)).await;
}

/// Read one scalar out of the cell's database.
fn query_one<T: rusqlite::types::FromSql>(cell_dir: &std::path::Path, sql: &str) -> T {
    let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).expect("open cell.db");
    conn.query_row(sql, [], |r| r.get(0)).expect("query")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_four_tables_exist_after_a_spawn() {
    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("create cell dir");

    spawn_and_settle(&cell_dir, free_port()).await;

    for table in ["objects", "components", "pages", "assets"] {
        let n: i64 = query_one(
            &cell_dir,
            &format!("SELECT count(*) FROM sqlite_master WHERE type='table' AND name='{table}'"),
        );
        assert_eq!(n, 1, "table {table} must exist after spawn");
    }
    // The index the object tree is read through — children of a parent, in
    // order. Without it every render walks the table.
    let idx: i64 = query_one(
        &cell_dir,
        "SELECT count(*) FROM sqlite_master WHERE type='index' AND name='idx_objects_parent'",
    );
    assert_eq!(idx, 1, "the parent/ord index must exist");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_seeded_cell_carries_its_rows() {
    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("create cell dir");
    write_seed(&cell_dir);

    spawn_and_settle(&cell_dir, free_port()).await;

    let components: i64 = query_one(&cell_dir, "SELECT count(*) FROM components");
    let pages: i64 = query_one(&cell_dir, "SELECT count(*) FROM pages");
    let objects: i64 = query_one(&cell_dir, "SELECT count(*) FROM objects");
    assert_eq!(
        (components, pages, objects),
        (1, 1, 1),
        "every seed file loaded"
    );

    let template: String = query_one(
        &cell_dir,
        "SELECT template FROM components WHERE name='text'",
    );
    assert_eq!(template, "<p>{{body}}</p>");
    let route: String = query_one(&cell_dir, "SELECT route FROM pages WHERE root='root-1'");
    assert_eq!(route, "/");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_spawn_on_the_same_directory_does_not_seed_again() {
    // The store's lesson, inherited deliberately: the seed loads on
    // `OpenStatus::Created` and never again. A display that re-seeded on every
    // wake would resurrect objects an operator deleted.
    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("create cell dir");
    write_seed(&cell_dir);

    spawn_and_settle(&cell_dir, free_port()).await;
    let after_first: i64 = query_one(&cell_dir, "SELECT count(*) FROM objects");
    assert_eq!(after_first, 1);

    // Prove the second spawn *saw* a live database rather than a fresh one, by
    // leaving a mark the seed cannot have written.
    {
        let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).expect("open");
        conn.execute(
            "INSERT INTO objects (id, parent, component, ord, props) VALUES ('mark', NULL, 'text', 1, '{}')",
            [],
        )
        .expect("insert mark");
    }

    spawn_and_settle(&cell_dir, free_port()).await;

    let after_second: i64 = query_one(&cell_dir, "SELECT count(*) FROM objects");
    assert_eq!(
        after_second, 2,
        "the seed must not run twice — expected the seeded row plus the mark, \
         got {after_second} rows"
    );
    let mark: i64 = query_one(&cell_dir, "SELECT count(*) FROM objects WHERE id='mark'");
    assert_eq!(
        mark, 1,
        "the mark survived, so this really was the same database"
    );
}

#[test]
fn a_broken_seed_file_is_refused_before_the_cell_spawns() {
    // validate-equals-spawn: a syntactic mistake in a seed file must surface in
    // the plan phase, not as a surprise on the first boot.
    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(cell_dir.join("seed")).expect("create seed dir");

    // Line 1 is a data row, not the schema header.
    std::fs::write(
        cell_dir.join("seed").join("pages.jsonl"),
        "{\"route\":\"/\",\"root\":\"root-1\"}\n",
    )
    .expect("write");

    let err = WebCellFactory
        .validate_cell_dir(&json!({ "port": 7800 }), &cell_dir)
        .expect_err("a header-less seed file must be refused");
    assert!(
        err.contains("schema"),
        "the refusal must name what is missing, got: {err}"
    );
}

#[test]
fn a_seed_file_for_a_table_the_cell_does_not_have_is_refused() {
    // The schema is fixed, so a seed file named after something else is a typo
    // that would otherwise be silently ignored forever.
    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(cell_dir.join("seed")).expect("create seed dir");
    std::fs::write(
        cell_dir.join("seed").join("widgets.jsonl"),
        "{\"schema\":{\"a\":\"text\"}}\n",
    )
    .expect("write");

    let err = WebCellFactory
        .validate_cell_dir(&json!({ "port": 7800 }), &cell_dir)
        .expect_err("an unknown seed table must be refused");
    assert!(
        err.contains("widgets"),
        "the refusal must name the offending file, got: {err}"
    );
}
