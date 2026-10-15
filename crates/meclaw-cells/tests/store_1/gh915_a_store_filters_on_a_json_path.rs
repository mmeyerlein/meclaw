//! GH #915 lock: a `store` filters and sorts on a JSON path inside a column,
//! and a declared index — on a path too, and `unique` — is created, used by the
//! query planner and enforced with a named `unique_violation`.
//!
//! Every read goes through the store dispatcher; the expected row sets are
//! computed in Rust over the raw documents, never by asking SQLite a second time.
//! The query plan is read with `EXPLAIN QUERY PLAN` directly on the `cell.db`,
//! over the exact statement text the store's own renderer produces.

use meclaw_cells::store::StoreParams;
use meclaw_cells::store::ddl::{apply_fts_ddl, apply_index_ddl, apply_schema_ddl};
use meclaw_cells::store::ops::dispatch;
use meclaw_cells::store::query::catalog::Catalog;
use meclaw_cells::store::query::parse::parse_filters;
use meclaw_cells::store::query::sql::render_where;
use meclaw_core::serde_json::{Value, json};
use std::collections::BTreeSet;

const ROWS: usize = 50;

fn params() -> StoreParams {
    StoreParams::parse(&json!({
        "schema": { "records": { "id": "text", "doc": "json", "label": "text" } },
        "fts": { "records": ["label"] },
        "indexes": {
            "records_scope": { "table": "records", "on": ["doc$.scope"] },
            "records_pair": { "table": "records", "on": ["doc$.type", "doc$.state"] },
            "records_id": { "table": "records", "on": ["id"], "unique": true },
            "records_key": { "table": "records", "on": ["doc$.key"], "unique": true }
        }
    }))
    .expect("the declaration parses")
}

/// Document `i`: nested, with gaps — `n` is missing on every seventh row, so
/// `json_extract` yields NULL there and the null operators have something to find.
fn doc(i: usize) -> Value {
    let mut d = json!({
        "scope": (["a", "b", "c"][i % 3]),
        "type": (["task", "note"][i % 2]),
        "state": (["open", "done", "held"][i % 3]),
        "key": format!("k{i}"),
        "due": { "start": (i * 7) % 23 },
        "tags": [format!("t{}", i % 4), "probe"]
    });
    if !i.is_multiple_of(7) {
        d["n"] = json!(i as i64);
    }
    d
}

struct Fixture {
    _dir: tempfile::TempDir,
    conn: rusqlite::Connection,
    docs: Vec<(String, Value)>,
}

/// A `cell.db` equipped the way the store factory equips one: connection
/// extensions, then schema → indexes → fts, then 50 rows written through the
/// dispatcher.
fn fixture() -> Fixture {
    let dir = tempfile::TempDir::new().unwrap();
    let conn = meclaw_colony::persist::open_or_create_cell_db(&dir.path().join("cell.db")).unwrap();
    meclaw_cells::store::query::install_connection_extensions(&conn).unwrap();
    let p = params();
    apply_schema_ddl(&conn, &p.schema).unwrap();
    apply_index_ddl(&conn, &p.indexes).unwrap();
    apply_fts_ddl(&conn, &p.fts, &p.canonical).unwrap();
    let mut docs = Vec::with_capacity(ROWS);
    for i in 0..ROWS {
        let id = format!("r{i:02}");
        let d = doc(i);
        let out = dispatch(
            &conn,
            &json!({"operation": "insert", "table": "records",
                    "row": {"id": id, "doc": d, "label": format!("probe row {i}")}}),
        )
        .unwrap();
        assert_eq!(out.error_code, None, "{:?}", out.error_text);
        docs.push((id, d));
    }
    Fixture {
        _dir: dir,
        conn,
        docs,
    }
}

fn select_ids(conn: &rusqlite::Connection, where_: Value) -> BTreeSet<String> {
    let out = dispatch(
        conn,
        &json!({"operation": "select", "table": "records", "columns": ["id"], "where": where_}),
    )
    .unwrap();
    assert_eq!(out.error_code, None, "{:?}", out.error_text);
    out.payload
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect()
}

fn expect_ids(f: &Fixture, keep: impl Fn(&Value) -> bool) -> BTreeSet<String> {
    f.docs
        .iter()
        .filter(|(_, d)| keep(d))
        .map(|(id, _)| id.clone())
        .collect()
}

/// `n` as SQLite sees it: an integer or NULL (missing key).
fn n(d: &Value) -> Option<i64> {
    d.get("n").and_then(Value::as_i64)
}

type Case = (Value, Box<dyn Fn(&Value) -> bool>);

/// A `where` payload plus the Rust predicate that must select the same rows.
fn case(where_: Value, keep: impl Fn(&Value) -> bool + 'static) -> Case {
    (where_, Box::new(keep))
}

#[test]
fn every_where_operator_on_a_json_path_matches_the_rust_evaluation() {
    let f = fixture();
    // Comparisons with NULL are never true — SQL three-valued logic, which is
    // exactly what `n(d).is_some_and(..)` expresses.
    let cases = vec![
        case(json!({"doc$.n": 10}), |d| n(d) == Some(10)),
        case(json!({"doc$.n": {"eq": 10}}), |d| n(d) == Some(10)),
        case(json!({"doc$.n": {"neq": 10}}), |d| {
            n(d).is_some_and(|v| v != 10)
        }),
        case(json!({"doc$.n": {"lt": 25}}), |d| {
            n(d).is_some_and(|v| v < 25)
        }),
        case(json!({"doc$.n": {"lte": 25}}), |d| {
            n(d).is_some_and(|v| v <= 25)
        }),
        case(json!({"doc$.n": {"gt": 25}}), |d| {
            n(d).is_some_and(|v| v > 25)
        }),
        case(json!({"doc$.n": {"gte": 25}}), |d| {
            n(d).is_some_and(|v| v >= 25)
        }),
        case(json!({"doc$.n": {"in": [1, 2, 3, 7]}}), |d| {
            n(d).is_some_and(|v| [1, 2, 3, 7].contains(&v))
        }),
        case(json!({"doc$.n": {"is_null": true}}), |d| n(d).is_none()),
        case(json!({"doc$.n": {"is_null": false}}), |d| n(d).is_some()),
        case(json!({"doc$.n": {"or_null": {"lt": 10}}}), |d| {
            n(d).is_none_or(|v| v < 10)
        }),
        case(json!({"doc$.scope": "b"}), |d| d["scope"] == "b"),
        case(json!({"doc$.scope": {"in": ["a", "c"]}}), |d| {
            d["scope"] == "a" || d["scope"] == "c"
        }),
        case(json!({"doc$.due.start": {"gte": 11}}), |d| {
            d["due"]["start"].as_i64().unwrap() >= 11
        }),
        case(json!({"doc$.tags[0]": "t1"}), |d| d["tags"][0] == "t1"),
        case(json!({"doc$.type": "task", "doc$.state": "open"}), |d| {
            d["type"] == "task" && d["state"] == "open"
        }),
        case(json!({"doc$.missing.deep": {"is_null": true}}), |_| true),
    ];
    for (where_, keep) in cases {
        let got = select_ids(&f.conn, where_.clone());
        let want = expect_ids(&f, keep);
        assert!(
            !want.is_empty() || where_ == json!({}),
            "{where_}: vacuous case"
        );
        assert_eq!(got, want, "where {where_}");
    }
}

#[test]
fn a_json_value_compares_by_its_json_type_without_casting() {
    let f = fixture();
    dispatch(
        &f.conn,
        &json!({"operation": "insert", "table": "records",
                "row": {"id": "text3", "doc": {"n": "3", "key": "text3"}, "label": "probe"}}),
    )
    .unwrap();
    // A number bind finds the JSON number 3 only; a text bind the JSON string only.
    assert_eq!(
        select_ids(&f.conn, json!({"doc$.n": 3})),
        BTreeSet::from(["r03".to_string()])
    );
    assert_eq!(
        select_ids(&f.conn, json!({"doc$.n": "3"})),
        BTreeSet::from(["text3".to_string()])
    );
}

#[test]
fn order_by_on_a_json_path_sorts_both_ways() {
    let f = fixture();
    for dir in ["asc", "desc"] {
        let out = dispatch(
            &f.conn,
            &json!({"operation": "select", "table": "records", "columns": ["id"],
                    "order_by": [{"col": "doc$.due.start", "dir": dir}, {"col": "id"}]}),
        )
        .unwrap();
        let got: Vec<String> = out
            .payload
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap().to_string())
            .collect();
        let mut want: Vec<(i64, String)> = f
            .docs
            .iter()
            .map(|(id, d)| (d["due"]["start"].as_i64().unwrap(), id.clone()))
            .collect();
        want.sort_by(|a, b| {
            let primary = if dir == "asc" {
                a.0.cmp(&b.0)
            } else {
                b.0.cmp(&a.0)
            };
            primary.then_with(|| a.1.cmp(&b.1))
        });
        let want: Vec<String> = want.into_iter().map(|(_, id)| id).collect();
        assert_eq!(got, want, "order {dir}");
    }
}

#[test]
fn search_filters_on_a_json_path_next_to_the_full_text_match() {
    let f = fixture();
    let out = dispatch(
        &f.conn,
        &json!({"operation": "search", "table": "records", "match": "probe",
                "columns": ["id"], "where": {"doc$.scope": "a"},
                "order_by": [{"col": "doc$.n", "dir": "desc"}]}),
    )
    .unwrap();
    assert_eq!(out.error_code, None, "{:?}", out.error_text);
    let got: BTreeSet<String> = out
        .payload
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(got, expect_ids(&f, |d| d["scope"] == "a"));
}

/// The statement the store renders for `where` — taken from the store's own
/// renderer, so the plan below is the plan of the real query.
fn plan_of(conn: &rusqlite::Connection, where_: Value) -> String {
    let cat = Catalog::load(conn, "records").unwrap();
    let filters = parse_filters(Some(&where_)).unwrap();
    let (clause, vals) = render_where(&filters, &cat).unwrap();
    let stmt = format!("EXPLAIN QUERY PLAN SELECT \"id\" FROM \"records\"{clause}");
    let mut st = conn.prepare(&stmt).unwrap();
    let bind: Vec<&dyn rusqlite::ToSql> = vals.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
    st.query_map(bind.as_slice(), |r| r.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .join(" | ")
}

#[test]
fn the_query_planner_uses_the_declared_path_indexes() {
    let f = fixture();
    let plan = plan_of(&f.conn, json!({"doc$.scope": "a"}));
    assert!(plan.contains("USING INDEX records_scope"), "{plan}");
    let plan = plan_of(&f.conn, json!({"doc$.type": "task", "doc$.state": "open"}));
    assert!(plan.contains("USING INDEX records_pair"), "{plan}");
    let plan = plan_of(&f.conn, json!({"id": "r01"}));
    assert!(plan.contains("records_id"), "{plan}");
}

#[test]
fn a_write_against_a_unique_index_answers_unique_violation_with_its_name() {
    let f = fixture();
    // insert: plain column index
    let out = dispatch(
        &f.conn,
        &json!({"operation": "insert", "table": "records",
                "row": {"id": "r01", "doc": {"key": "fresh"}}}),
    )
    .unwrap();
    assert_eq!(out.error_code, Some("unique_violation"));
    assert_eq!(out.error_index.as_deref(), Some("records_id"));
    let text: Value =
        meclaw_core::serde_json::from_str(out.error_text.as_deref().unwrap()).unwrap();
    assert_eq!(text["code"], "unique_violation");
    assert_eq!(text["index"], "records_id");

    // insert: path index
    let out = dispatch(
        &f.conn,
        &json!({"operation": "insert", "table": "records",
                "row": {"id": "new", "doc": {"key": "k5"}}}),
    )
    .unwrap();
    assert_eq!(out.error_code, Some("unique_violation"));
    assert_eq!(out.error_index.as_deref(), Some("records_key"));

    // update: path index
    let out = dispatch(
        &f.conn,
        &json!({"operation": "update", "table": "records",
                "set": {"doc": {"key": "k7"}}, "where": {"id": "r08"}}),
    )
    .unwrap();
    assert_eq!(out.error_code, Some("unique_violation"));
    assert_eq!(out.error_index.as_deref(), Some("records_key"));
    assert_eq!(
        select_ids(&f.conn, json!({"doc$.key": "k8"})),
        BTreeSet::from(["r08".to_string()])
    );

    // update: plain column index
    let out = dispatch(
        &f.conn,
        &json!({"operation": "update", "table": "records",
                "set": {"id": "r02"}, "where": {"id": "r03"}}),
    )
    .unwrap();
    assert_eq!(out.error_code, Some("unique_violation"));
    assert_eq!(out.error_index.as_deref(), Some("records_id"));

    // a clean write still succeeds
    let out = dispatch(
        &f.conn,
        &json!({"operation": "insert", "table": "records",
                "row": {"id": "new", "doc": {"key": "fresh"}}}),
    )
    .unwrap();
    assert_eq!(out.error_code, None);
}

#[test]
fn a_constraint_the_store_did_not_declare_stays_a_constraint_violation() {
    let f = fixture();
    f.conn
        .execute_batch(
            "CREATE TABLE other (k TEXT PRIMARY KEY, u TEXT UNIQUE);
             INSERT INTO other VALUES ('a', 'x');",
        )
        .unwrap();
    for row in [json!({"k": "a", "u": "y"}), json!({"k": "b", "u": "x"})] {
        let out = dispatch(
            &f.conn,
            &json!({"operation": "insert", "table": "other", "row": row}),
        )
        .unwrap();
        assert_eq!(out.error_code, Some("constraint_violation"), "{row}");
        assert_eq!(out.error_index, None);
    }
}

#[test]
fn malformed_path_keys_and_unknown_columns_fail_cleanly() {
    let f = fixture();
    for bad in [
        json!({"operation": "select", "table": "records", "columns": ["id"],
               "where": {"doc$.a'": 1}}),
        json!({"operation": "select", "table": "records", "columns": ["id"],
               "where": {"doc$": 1}}),
        json!({"operation": "select", "table": "records", "columns": ["id"],
               "order_by": [{"col": "doc$..a"}]}),
        json!({"operation": "delete", "table": "records", "where": {"doc$[x]": 1}}),
        json!({"operation": "select", "table": "records", "columns": ["id"],
               "distinct": true, "order_by": [{"col": "doc$.n"}]}),
    ] {
        assert!(dispatch(&f.conn, &bad).is_err(), "{bad} is invalid_input");
    }
    let out = dispatch(
        &f.conn,
        &json!({"operation": "select", "table": "records", "columns": ["id"],
                "where": {"nope$.a": 1}}),
    )
    .unwrap();
    assert_eq!(out.error_code, Some("unknown_column"));
    assert_eq!(
        select_ids(&f.conn, json!({})).len(),
        ROWS,
        "nothing was touched"
    );
}

#[test]
fn a_path_over_a_row_that_is_not_json_is_an_error_outcome_not_a_panic() {
    let f = fixture();
    // A table without path indexes: on `records` the index maintenance itself
    // would already refuse to store a non-JSON document.
    f.conn
        .execute_batch(
            "CREATE TABLE loose (id TEXT, doc TEXT);
             INSERT INTO loose VALUES ('fine', '{\"scope\":\"a\"}'), ('broken', 'not json');",
        )
        .unwrap();
    let out = dispatch(
        &f.conn,
        &json!({"operation": "select", "table": "loose", "columns": ["id"],
                "where": {"doc$.scope": "a"}}),
    )
    .unwrap();
    assert_eq!(out.error_code, Some("sql_error"), "{:?}", out.error_text);
}
