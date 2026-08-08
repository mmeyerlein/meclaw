//! P3 integration proofs for the store query layer.
//!
//! Three things are demonstrated end to end: the bi-temporal as-of query that
//! was impossible before this package, the injection matrix (every place where
//! caller text used to be formatted into SQL), and the write-op breadth that
//! comes with sharing one `where` parser across select/update/delete.

use meclaw_cells::store::StoreParams;
use meclaw_cells::store::ddl::{apply_fts_ddl, apply_schema_ddl};
use meclaw_cells::store::ops::dispatch;
use meclaw_core::serde_json::{Value, json};

// ── 1) as-of query on the facts schema (the P3 done-criterion) ──────────────

/// The memory hive's central question: "what was true at time T?" — expressed
/// entirely in store ops for the first time (`valid_from lte T`,
/// `valid_until or_null gt T`).
#[test]
fn as_of_query_on_facts_schema_returns_only_facts_valid_at_t() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute(
        "CREATE TABLE facts (id TEXT, subject TEXT, claim TEXT, valid_from TEXT, valid_until TEXT)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO facts VALUES \
         ('f1','user:marcus','isst ketogen','2026-01-01T00:00:00Z',NULL),\
         ('f2','user:marcus','isst vegan','2025-01-01T00:00:00Z','2026-01-01T00:00:00Z'),\
         ('f3','user:marcus','zieht um','2026-09-01T00:00:00Z',NULL),\
         ('f4','user:other','irrelevant','2020-01-01T00:00:00Z',NULL)",
        [],
    )
    .unwrap();

    let t = "2026-08-01T00:00:00Z";
    let args = json!({
        "operation":"select","table":"facts","columns":["id","claim"],
        "where":{
            "subject":"user:marcus",
            "valid_from":{"lte": t},
            "valid_until":{"or_null":{"gt": t}}
        },
        "order_by":[{"col":"valid_from","dir":"desc"},{"col":"id","dir":"asc"}],
        "limit":50
    });
    let out = dispatch(&conn, &args).unwrap();
    assert_eq!(out.error_code, None);
    let ids: Vec<&str> = out
        .payload
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        vec!["f1"],
        "f2 expired, f3 not yet valid, f4 other subject"
    );
}

// ── 2) injection matrix ─────────────────────────────────────────────────────

fn fixture() -> rusqlite::Connection {
    let c = rusqlite::Connection::open_in_memory().unwrap();
    c.execute_batch(
        "CREATE TABLE items (id INTEGER, name TEXT);\
         CREATE TABLE keep (id INTEGER);\
         INSERT INTO items VALUES (1,'a'),(2,'b');\
         INSERT INTO keep VALUES (1),(2);",
    )
    .unwrap();
    c
}

/// Positive receipt: both tables still exist, still hold two rows each, and no
/// value was mutated. Never asserted via "an error appeared" alone.
fn assert_intact(c: &rusqlite::Connection) {
    let tables: i64 = c
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tables, 2, "no table created or dropped");
    for t in ["items", "keep"] {
        let n: i64 = c
            .query_row(&format!("SELECT count(*) FROM \"{t}\""), [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2, "{t} rows untouched");
    }
    let names: String = c
        .query_row(
            "SELECT group_concat(name) FROM (SELECT name FROM items ORDER BY id)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(names, "a,b", "no value mutated");
}

/// Every payload below puts SQL-shaped text exactly where the pre-P3 ops layer
/// formatted caller strings into a statement. Requirement: a clean reject (hard
/// `Err` on the invalid_input path, or an outcome carrying an error code) and an
/// untouched database.
#[test]
fn injection_matrix_rejects_cleanly_and_leaves_the_db_intact() {
    let evil = "\"; DROP TABLE keep; --";
    let cases: Vec<Value> = vec![
        json!({"operation":"select","table":format!("items{evil}"),"columns":["id"]}),
        json!({"operation":"select","table":"items","columns":[format!("id{evil}")]}),
        json!({"operation":"select","table":"items","columns":["id"],
               "where":{format!("id{evil}"): 1}}),
        json!({"operation":"select","table":"items","columns":["id"],
               "where":{"id":{format!("lte{evil}"): 1}}}),
        json!({"operation":"select","table":"items","columns":["id"],
               "order_by":[{"col":format!("id{evil}")}]}),
        json!({"operation":"select","table":"items","columns":["id"],
               "order_by":[{"col":"id","dir":format!("asc{evil}")}]}),
        json!({"operation":"select","table":"items","columns":["id"],
               "limit":"1; DROP TABLE keep"}),
        json!({"operation":"update","table":"items","set":{format!("name{evil}"):"x"},
               "where":{"id":1}}),
        json!({"operation":"update","table":format!("items{evil}"),"set":{"name":"x"}}),
        json!({"operation":"delete","table":"items","where":{format!("id{evil}"):1}}),
        json!({"operation":"delete","table":format!("items{evil}")}),
        json!({"operation":"insert","table":"items","row":{format!("name{evil}"):"x"}}),
        json!({"operation":"insert","table":format!("items{evil}"),"row":{"name":"x"}}),
        json!({"operation":"create_table","table":format!("t{evil}"),"columns":{"a":"int"}}),
        json!({"operation":"create_table","table":"t","columns":{format!("a{evil}"):"int"}}),
        json!({"operation":"search","table":"items","columns":["id"],"match":"x\" OR 1=1 --"}),
        json!({"operation":"search","table":format!("items{evil}"),"columns":["id"],"match":"x"}),
    ];
    for c in cases {
        let conn = fixture();
        match dispatch(&conn, &c) {
            Err(_) => {}
            Ok(o) => assert!(o.error_code.is_some(), "must not succeed: {c}"),
        }
        assert_intact(&conn);
    }
}

/// Sharper than the matrix above: the identifier cases must be stopped by the
/// **catalog**, i.e. answer `unknown_table`/`unknown_column` — not by whatever
/// SQLite happens to make of a mangled statement.
///
/// Why this test exists: a mutation probe (catalog passing caller text through
/// unchecked) left the matrix above fully green, because rusqlite rejects the
/// resulting multi-statement string on its own. "Nothing got through" is the
/// security claim; "the catalog stopped it" is the design claim, and only this
/// test pins the second one.
#[test]
fn identifier_injections_are_stopped_by_the_catalog_not_by_sqlite() {
    let evil = "\"; DROP TABLE keep; --";
    let cases: Vec<(Value, &str)> = vec![
        (
            json!({"operation":"select","table":format!("items{evil}"),"columns":["id"]}),
            "unknown_table",
        ),
        (
            json!({"operation":"select","table":"items","columns":[format!("id{evil}")]}),
            "unknown_column",
        ),
        (
            json!({"operation":"select","table":"items","columns":["id"],
                   "where":{format!("id{evil}"): 1}}),
            "unknown_column",
        ),
        (
            json!({"operation":"select","table":"items","columns":["id"],
                   "order_by":[{"col":format!("id{evil}")}]}),
            "unknown_column",
        ),
        (
            json!({"operation":"update","table":"items","set":{format!("name{evil}"):"x"}}),
            "unknown_column",
        ),
        (
            json!({"operation":"delete","table":"items","where":{format!("id{evil}"):1}}),
            "unknown_column",
        ),
        (
            json!({"operation":"insert","table":"items","row":{format!("name{evil}"):"x"}}),
            "unknown_column",
        ),
        (
            json!({"operation":"search","table":format!("items{evil}"),"columns":["id"],
                   "match":"x"}),
            "unknown_table",
        ),
    ];
    for (payload, want) in cases {
        let conn = fixture();
        let out = dispatch(&conn, &payload).expect("catalog rejects are outcomes, not hard errors");
        assert_eq!(
            out.error_code,
            Some(want),
            "wrong rejecting layer for {payload}"
        );
        assert_intact(&conn);
    }
}

/// The values inside an `in` list are bound, so SQL-shaped *values* are inert —
/// they simply do not match anything.
#[test]
fn sql_shaped_values_are_bound_not_interpreted() {
    let conn = fixture();
    let out = dispatch(
        &conn,
        &json!({"operation":"select","table":"items","columns":["id"],
                "where":{"name":{"in":["a'); DROP TABLE keep; --","b"]}}}),
    )
    .unwrap();
    assert_eq!(out.error_code, None);
    assert_eq!(out.rows_affected, 1, "only the literal 'b' matches");
    assert_intact(&conn);
}

// ── 3) write ops in full breadth (plan ruling R5, review obligation) ────────

fn write_fixture() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute("CREATE TABLE t (id INTEGER, grp TEXT, ts TEXT)", [])
        .unwrap();
    conn.execute(
        "INSERT INTO t VALUES (1,'a','2026-09-01'),(2,'a',NULL),(3,'a','2026-01-01'),\
                              (4,'b','2026-09-01'),(5,'b',NULL)",
        [],
    )
    .unwrap();
    conn
}

/// `or_null` must bind tighter than the surrounding `AND` — proven by row
/// counts, not by string shape. A missing pair of parentheses turns this delete
/// into a mass delete, which is exactly what this test exists to catch.
#[test]
fn or_null_stays_parenthesised_under_conjunction_on_delete_and_update() {
    let conn = write_fixture();

    // grp='a' AND (ts > '2026-06-01' OR ts IS NULL) -> rows 1 and 2 only
    let out = dispatch(
        &conn,
        &json!({"operation":"delete","table":"t",
                "where":{"grp":"a","ts":{"or_null":{"gt":"2026-06-01"}}}}),
    )
    .unwrap();
    assert_eq!(
        out.rows_affected, 2,
        "must not touch grp='b' or the old row"
    );
    let left: Vec<i64> = conn
        .prepare("SELECT id FROM t ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(left, vec![3, 4, 5]);

    // same shape on update: only the grp='b' rows 4 and 5
    let out = dispatch(
        &conn,
        &json!({"operation":"update","table":"t","set":{"grp":"z"},
                "where":{"grp":"b","ts":{"or_null":{"gt":"2026-06-01"}}}}),
    )
    .unwrap();
    assert_eq!(out.rows_affected, 2);
    let untouched: String = conn
        .query_row("SELECT grp FROM t WHERE id=3", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        untouched, "a",
        "row outside the predicate must be untouched"
    );
}

/// The full operator set must select and delete the same row set — one parser,
/// one semantics, whether the op reads or writes.
#[test]
fn every_operator_selects_and_deletes_the_same_row_set() {
    for where_json in [
        json!({"ts":{"is_null":true}}),
        json!({"ts":{"is_null":false}}),
        json!({"id":{"in":[1,3]}}),
        json!({"id":{"neq":1}}),
        json!({"ts":{"lte":"2026-06-01"}}),
        json!({"grp":"a","ts":{"or_null":{"gte":"2026-09-01"}}}),
        json!({"grp":{"eq":"b"}}),
        json!({"id":{"gt":3}}),
        json!({"id":{"lt":3}}),
    ] {
        let conn = write_fixture();
        let sel = dispatch(
            &conn,
            &json!({"operation":"select","table":"t","columns":["id"],"where":where_json}),
        )
        .unwrap();
        let expected = sel.rows_affected;
        assert!(expected > 0, "case must select something: {where_json}");
        let del = dispatch(
            &conn,
            &json!({"operation":"delete","table":"t","where":where_json}),
        )
        .unwrap();
        assert_eq!(
            del.rows_affected, expected,
            "select/delete disagree on {where_json}"
        );
    }
}

// ── 4) FTS catches up on an existing cell.db ────────────────────────────────

/// The realistic upgrade path: a store that has been running without any FTS
/// declaration gets one, and the rows written *before* that declaration become
/// searchable on the next spawn — one rebuild, not one per boot.
#[test]
fn fts_is_backfilled_into_an_existing_cell_db_on_next_open() {
    let td = tempfile::TempDir::new().unwrap();
    let path = td.path().join("cell.db");

    {
        let conn = meclaw_colony::persist::open_or_create_cell_db(&path).unwrap();
        let p =
            StoreParams::parse(&json!({"schema":{"facts":{"id":"text","claim":"text"}}})).unwrap();
        apply_schema_ddl(&conn, &p.schema).unwrap();
        apply_fts_ddl(&conn, &p.fts).unwrap();
        let args = json!({"operation":"insert","table":"facts",
                          "row":{"id":"f1","claim":"marcus isst ketogen"}});
        assert_eq!(dispatch(&conn, &args).unwrap().rows_affected, 1);
        // no index yet
        assert_eq!(
            dispatch(
                &conn,
                &json!({"operation":"search","table":"facts","columns":["id"],"match":"ketogen"})
            )
            .unwrap()
            .error_code,
            Some("unknown_table")
        );
    }

    let conn = meclaw_colony::persist::open_or_create_cell_db(&path).unwrap();
    let p = StoreParams::parse(&json!({
        "schema": {"facts": {"id":"text","claim":"text"}},
        "fts": {"facts": ["claim"]}
    }))
    .unwrap();
    apply_schema_ddl(&conn, &p.schema).unwrap();
    apply_fts_ddl(&conn, &p.fts).unwrap();

    let out = dispatch(
        &conn,
        &json!({"operation":"search","table":"facts","columns":["id","claim"],
                "match":"ketogen"}),
    )
    .unwrap();
    assert_eq!(out.error_code, None);
    let rows = out.payload.as_array().unwrap();
    assert_eq!(
        rows.len(),
        1,
        "one-time rebuild must index pre-existing rows"
    );
    assert_eq!(rows[0]["id"], "f1");
    assert!(rows[0]["rank"].is_number(), "bm25 rank must be reported");
}
