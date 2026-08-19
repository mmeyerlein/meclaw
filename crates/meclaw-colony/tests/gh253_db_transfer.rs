//! GH #253 — the content transfer of a cell that has a `cell.db`, pinned at the
//! level it lives on: the substrate, not a cell type.
//!
//! The way **in** was never per-type — `mutation::stage::apply_seed_jsonl` calls
//! itself a *"Generic JSONL-Seed-Loader"* and takes a path. Only the way **out**
//! was missing, and it was missing for all ten types with a `cell.db`. These
//! tests pin the two properties that make the export the loader's inverse rather
//! than a second mechanism: the document's `schema` object IS a seed header, and
//! a document written out as a seed file births a cell with the same rows.
//!
//! The import is the half the loader cannot reach — it runs into a **running**
//! cell — and its three decisions are pinned here too.

use meclaw_colony::db_transfer::{TransferOutcome, dispatch};
use meclaw_core::serde_json::{Map, Value, json};

fn args(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

fn done(conn: &rusqlite::Connection, v: Value) -> (i64, Value) {
    match dispatch(conn, &args(v)).unwrap() {
        TransferOutcome::Done {
            rows_affected,
            payload,
        } => (rows_affected, payload),
        other => panic!("expected Done, got {other:?}"),
    }
}

fn refused(conn: &rusqlite::Connection, v: Value) -> (&'static str, String) {
    match dispatch(conn, &args(v)).unwrap() {
        TransferOutcome::Refused { code, detail } => (code, detail),
        other => panic!("expected Refused, got {other:?}"),
    }
}

/// A `cell.db` as the substrate makes one, plus one content table.
fn cell_db(td: &tempfile::TempDir, ddl: &str) -> rusqlite::Connection {
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    conn.execute_batch(ddl).unwrap();
    conn
}

// ------------------------------------------------------------------ export

#[test]
fn export_lists_content_and_leaves_the_substrates_own_tables_alone() {
    let td = tempfile::TempDir::new().unwrap();
    let conn = cell_db(
        &td,
        "CREATE TABLE notes (id TEXT PRIMARY KEY, body TEXT);
         CREATE TABLE schedules (schedule_id TEXT PRIMARY KEY, cron_expr TEXT);",
    );

    let (_, doc) = done(&conn, json!({"operation": "export"}));
    let tables: Vec<&str> = doc["tables"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    // `system` IS content — `seed/system.jsonl` is a documented seed input
    // (GH #99), so it has to be a documented output. `last_input`, `meta` and
    // `params` are the substrate's own bookkeeping.
    assert_eq!(tables, vec!["notes", "schedules", "system"]);
}

#[test]
fn a_table_the_substrate_owns_is_refused_and_says_which_kind_of_no_it_is() {
    let td = tempfile::TempDir::new().unwrap();
    let conn = cell_db(&td, "CREATE TABLE notes (id TEXT PRIMARY KEY);");

    let (code, detail) = refused(&conn, json!({"operation": "export", "table": "params"}));
    assert_eq!(code, "unknown_table");
    assert!(
        detail.contains("not content"),
        "a substrate table is refused as machinery, not as a typo: {detail}"
    );

    let (code, detail) = refused(&conn, json!({"operation": "export", "table": "notez"}));
    assert_eq!(code, "unknown_table");
    assert!(detail.contains("no such table"), "{detail}");
}

#[test]
fn an_exported_document_is_a_seed_file_and_births_the_same_rows() {
    let source = tempfile::TempDir::new().unwrap();
    let conn = cell_db(
        &source,
        "CREATE TABLE notes (id TEXT PRIMARY KEY, body TEXT, n INTEGER);
         INSERT INTO notes VALUES ('b', 'second', 2), ('a', 'first', 1);",
    );
    let (rows, doc) = done(&conn, json!({"operation": "export", "table": "notes"}));

    assert_eq!(doc["format"], "meclaw-cell-export/1");
    assert_eq!(doc["table"], "notes");
    assert_eq!(doc["key"], json!(["id"]), "the table's own primary key");
    assert_eq!(
        doc["schema"],
        json!({"id": "text", "body": "text", "n": "int"})
    );
    assert_eq!(rows, 2);
    // Ordered by the key, so the same content yields the same document.
    assert_eq!(doc["rows"][0]["id"], "a");
    assert_eq!(doc["rows"][1]["id"], "b");

    // That the document IS a seed file — header on line 1, one row per line —
    // and that the EXISTING loader births a cell from it is pinned in
    // `db_transfer`'s own unit tests, where `seed_cell_db_if_present` is
    // reachable (`exported_document_is_a_seed_file_the_existing_loader_reads`).
}

// ------------------------------------------------------------------ import

fn part(rows: Value) -> Value {
    json!({"operation": "import", "table": "notes",
           "schema": {"id": "text", "body": "text", "n": "int"},
           "rows": rows})
}

fn notes(conn: &rusqlite::Connection) -> Vec<(String, String)> {
    let mut st = conn
        .prepare("SELECT id, COALESCE(body,'') FROM notes ORDER BY id")
        .unwrap();
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[test]
fn import_inserts_what_is_missing_and_the_target_wins_every_collision() {
    let td = tempfile::TempDir::new().unwrap();
    let conn = cell_db(
        &td,
        "CREATE TABLE notes (id TEXT PRIMARY KEY, body TEXT, n INTEGER);
         INSERT INTO notes VALUES ('a', 'the target decided this', 1);",
    );

    let (written, receipt) = done(
        &conn,
        part(json!([
            {"id": "a", "body": "the document disagrees", "n": 9},
            {"id": "b", "body": "new here", "n": 2},
        ])),
    );
    assert_eq!(written, 1);
    assert_eq!(receipt["rows_in_part"], 2);
    assert_eq!(receipt["rows_written"], 1);
    assert_eq!(receipt["rows_skipped"], 1);
    assert_eq!(
        notes(&conn),
        vec![
            ("a".into(), "the target decided this".to_string()),
            ("b".into(), "new here".to_string()),
        ],
        "an import never updates and never overwrites"
    );
}

#[test]
fn the_same_document_applied_twice_leaves_the_same_state() {
    let td = tempfile::TempDir::new().unwrap();
    let conn = cell_db(
        &td,
        "CREATE TABLE notes (id TEXT PRIMARY KEY, body TEXT, n INTEGER);",
    );
    let doc = part(json!([{"id": "a", "body": "x", "n": 1}, {"id": "b", "body": "y", "n": 2}]));

    let (first, _) = done(&conn, doc.clone());
    assert_eq!(first, 2);
    let before = notes(&conn);
    let (second, _) = done(&conn, doc);
    assert_eq!(second, 0, "re-applying is arithmetic");
    assert_eq!(notes(&conn), before);
}

#[test]
fn a_part_that_repeats_a_key_inside_itself_writes_it_once() {
    let td = tempfile::TempDir::new().unwrap();
    let conn = cell_db(
        &td,
        "CREATE TABLE notes (id TEXT PRIMARY KEY, body TEXT, n INTEGER);",
    );
    let (written, _) = done(
        &conn,
        part(json!([{"id": "a", "body": "first", "n": 1},
                    {"id": "a", "body": "again", "n": 2}])),
    );
    assert_eq!(written, 1);
    assert_eq!(notes(&conn), vec![("a".into(), "first".to_string())]);
}

#[test]
fn a_part_that_fails_halfway_writes_nothing() {
    let td = tempfile::TempDir::new().unwrap();
    let conn = cell_db(
        &td,
        "CREATE TABLE notes (id TEXT PRIMARY KEY, body TEXT, n INTEGER CHECK (n < 10));",
    );
    let out = dispatch(
        &conn,
        &args(part(json!([{"id": "a", "body": "fine", "n": 1},
                          {"id": "b", "body": "too big", "n": 99}]))),
    )
    .unwrap();
    assert!(
        matches!(out, TransferOutcome::Sql(_)),
        "expected a SQL failure, got {out:?}"
    );
    assert!(
        notes(&conn).is_empty(),
        "a part applies whole or not at all"
    );
}

#[test]
fn an_import_without_a_key_is_refused_before_it_can_duplicate() {
    let td = tempfile::TempDir::new().unwrap();
    // No PRIMARY KEY — which is every table the seed loader builds, and every
    // table a `store` declares in `params.schema`.
    let conn = cell_db(&td, "CREATE TABLE notes (id TEXT, body TEXT, n INTEGER);");
    let e = dispatch(
        &conn,
        &args(part(json!([{"id": "a", "body": "x", "n": 1}]))),
    )
    .expect_err("no key, no idempotence — say so instead of duplicating");
    assert!(e.contains("key"), "{e}");

    // Named explicitly, it works.
    let (written, _) = done(
        &conn,
        json!({"operation": "import", "table": "notes", "key": ["id"],
               "schema": {"id": "text", "body": "text", "n": "int"},
               "rows": [{"id": "a", "body": "x", "n": 1}]}),
    );
    assert_eq!(written, 1);
}

#[test]
fn import_refuses_a_part_whose_schema_lost_a_column() {
    let td = tempfile::TempDir::new().unwrap();
    let conn = cell_db(
        &td,
        "CREATE TABLE notes (id TEXT PRIMARY KEY, body TEXT, audience_set TEXT);",
    );
    let (code, detail) = refused(
        &conn,
        json!({"operation": "import", "table": "notes",
               "schema": {"id": "text", "body": "text"},
               "rows": [{"id": "a", "body": "x"}]}),
    );
    assert_eq!(code, "import_schema_drift");
    assert!(
        detail.contains("audience_set"),
        "the refusal names the column that did not survive: {detail}"
    );
    assert!(notes_count(&conn) == 0, "a refused part writes nothing");
}

#[test]
fn import_refuses_a_part_that_declares_a_column_this_cell_does_not_have() {
    let td = tempfile::TempDir::new().unwrap();
    let conn = cell_db(&td, "CREATE TABLE notes (id TEXT PRIMARY KEY, body TEXT);");
    let (code, detail) = refused(
        &conn,
        json!({"operation": "import", "table": "notes",
               "schema": {"id": "text", "body": "text", "confidence": "int"},
               "rows": [{"id": "a", "body": "x", "confidence": 3}]}),
    );
    assert_eq!(code, "import_schema_drift");
    assert!(detail.contains("confidence"), "{detail}");
    assert!(notes_count(&conn) == 0);
}

fn notes_count(conn: &rusqlite::Connection) -> i64 {
    conn.query_row("SELECT count(*) FROM notes", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn an_unknown_transfer_operation_is_an_args_level_fault() {
    let td = tempfile::TempDir::new().unwrap();
    let conn = cell_db(&td, "CREATE TABLE notes (id TEXT PRIMARY KEY);");
    assert!(dispatch(&conn, &args(json!({"operation": "truncate"}))).is_err());
    assert!(dispatch(&conn, &args(json!({"table": "notes"}))).is_err());
}
