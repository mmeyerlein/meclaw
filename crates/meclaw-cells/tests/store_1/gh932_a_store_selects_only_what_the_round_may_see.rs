//! GH #932 lock: the store evaluates the audience set rule where rows are
//! selected — the `where` operator `covers`.
//!
//! `{"<col>": {"covers": <round>}}` is true exactly when the row value is a
//! JSON array that contains `"*"` or contains every element of the round.
//! NULL, a non-array, an empty array and an array without matching strings are
//! false, and a broken row value is never an error. The operator works in
//! `select`, `update`, `delete` and `search`, on a column and on a JSON path,
//! so a reader filtering by round cannot write through a gap.
//!
//! Every read goes through the store dispatcher; the expected row sets are
//! computed in Rust over the raw values, never by asking SQLite a second time.

use meclaw_cells::store::StoreParams;
use meclaw_cells::store::ddl::{apply_fts_ddl, apply_index_ddl, apply_schema_ddl};
use meclaw_cells::store::ops::dispatch;
use meclaw_core::serde_json::{Value, json};
use std::collections::BTreeSet;

/// Raw `aud` cell values (`None` = SQL NULL). The first three are the rows the
/// round `["e","a"]` may see; the tail pins the edge cases: order does not
/// matter, `*` counts only as a whole element, non-strings never match.
const AUD: &[Option<&str>] = &[
    Some(r#"["*"]"#),
    Some(r#"["e","a"]"#),
    Some(r#"["e","a","b"]"#),
    Some(r#"["e","b"]"#),
    Some("[]"),
    None,
    Some(r#""x""#),
    Some(r#"{"e":1}"#),
    Some("x"),
    Some(r#"["a","e"]"#),
    Some(r#"["e*","a*"]"#),
    Some(r#"["*e","a"]"#),
    Some("[1,2]"),
    Some(r#"[["e","a"]]"#),
    Some(r#"["e",1,"a"]"#),
];

fn params() -> StoreParams {
    StoreParams::parse(&json!({
        "schema": { "t": { "id": "text", "aud": "text", "doc": "json", "label": "text" } },
        "fts": { "t": ["label"] }
    }))
    .expect("the declaration parses")
}

struct Fixture {
    _dir: tempfile::TempDir,
    conn: rusqlite::Connection,
}

fn id(i: usize) -> String {
    format!("r{i:02}")
}

/// A `cell.db` equipped the way the store factory equips one, every row
/// written through the dispatcher. `doc.aud` carries the same value as `aud`
/// as real JSON where it parses; a raw non-JSON value and NULL become a JSON
/// string and a missing key, and one extra row hides an array inside a JSON
/// STRING — a path value must be an array, not text that looks like one.
fn fixture() -> Fixture {
    let dir = tempfile::TempDir::new().unwrap();
    let conn = meclaw_colony::persist::open_or_create_cell_db(&dir.path().join("cell.db")).unwrap();
    meclaw_cells::store::query::install_connection_extensions(&conn).unwrap();
    let p = params();
    apply_schema_ddl(&conn, &p.schema).unwrap();
    apply_index_ddl(&conn, &p.indexes).unwrap();
    apply_fts_ddl(&conn, &p.fts, &p.canonical).unwrap();
    for (i, aud) in AUD.iter().enumerate() {
        let doc = match aud {
            None => json!({}),
            Some(s) => match meclaw_core::serde_json::from_str::<Value>(s) {
                Ok(v) => json!({ "aud": v }),
                Err(_) => json!({ "aud": s }),
            },
        };
        insert(
            &conn,
            &id(i),
            aud.map(|s| json!(s)).unwrap_or(Value::Null),
            doc,
        );
    }
    insert(&conn, "rstr", Value::Null, json!({ "aud": r#"["e","a"]"# }));
    // A `doc` that is not JSON at all: a path read on it must be false, never
    // a `malformed JSON` error that takes the whole select down.
    insert(&conn, "rbad", Value::Null, json!({}));
    conn.execute(
        "UPDATE \"t\" SET \"doc\" = 'not json {' WHERE \"id\" = 'rbad'",
        [],
    )
    .unwrap();
    Fixture { _dir: dir, conn }
}

fn insert(conn: &rusqlite::Connection, id: &str, aud: Value, doc: Value) {
    let out = dispatch(
        conn,
        &json!({"operation": "insert", "table": "t",
                "row": {"id": id, "aud": aud, "doc": doc, "label": format!("probe {id}")}}),
    )
    .unwrap();
    assert_eq!(out.error_code, None, "{:?}", out.error_text);
}

/// The rule in Rust: the value parses as a JSON array and contains `"*"` or
/// every round element (as a string element).
fn covers(raw: Option<&str>, round: &[&str]) -> bool {
    let Some(Value::Array(items)) = raw.and_then(|s| meclaw_core::serde_json::from_str(s).ok())
    else {
        return false;
    };
    let has = |w: &str| items.iter().any(|v| v.as_str() == Some(w));
    has("*") || round.iter().all(|w| has(w))
}

fn expect(round: &[&str]) -> BTreeSet<String> {
    AUD.iter()
        .enumerate()
        .filter(|(_, a)| covers(**a, round))
        .map(|(i, _)| id(i))
        .collect()
}

fn ids_of(out: &meclaw_cells::store::ops::OpOutcome) -> BTreeSet<String> {
    out.payload
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect()
}

fn select_ids(conn: &rusqlite::Connection, where_: Value) -> BTreeSet<String> {
    let out = dispatch(
        conn,
        &json!({"operation": "select", "table": "t", "columns": ["id"], "where": where_}),
    )
    .unwrap();
    assert_eq!(out.error_code, None, "{:?}", out.error_text);
    ids_of(&out)
}

fn set(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|s| s.to_string()).collect()
}

#[test]
fn select_with_covers_returns_exactly_the_rows_the_round_may_see() {
    let f = fixture();
    let got = select_ids(&f.conn, json!({"aud": {"covers": ["e", "a"]}}));
    assert_eq!(got, expect(&["e", "a"]));
    // The contract's literal expectation, independent of the Rust model.
    assert_eq!(got, set(&["r00", "r01", "r02", "r09", "r14"]));

    let got = select_ids(&f.conn, json!({"aud": {"covers": ["e", "a", "b"]}}));
    assert_eq!(got, expect(&["e", "a", "b"]));
    assert_eq!(got, set(&["r00", "r02"]));

    // Duplicates in the round change nothing; a round element `*` is just a
    // string — only the row's `*` is universal.
    assert_eq!(
        select_ids(&f.conn, json!({"aud": {"covers": ["a", "e", "a"]}})),
        expect(&["e", "a"])
    );
    assert_eq!(
        select_ids(&f.conn, json!({"aud": {"covers": ["*"]}})),
        set(&["r00"])
    );
}

#[test]
fn covers_combines_with_other_filters() {
    let f = fixture();
    assert_eq!(
        select_ids(
            &f.conn,
            json!({"aud": {"covers": ["e", "a"]}, "id": {"in": ["r01", "r03", "r09"]}})
        ),
        set(&["r01", "r09"])
    );
}

#[test]
fn covers_on_a_json_path_needs_a_real_array_at_the_path() {
    let f = fixture();
    let got = select_ids(&f.conn, json!({"doc$.aud": {"covers": ["e", "a"]}}));
    // `rstr` stores the array as a JSON string — text that merely looks like an
    // array is not one.
    assert_eq!(got, expect(&["e", "a"]));
    assert!(!got.contains("rstr") && !got.contains("rbad"));
}

#[test]
fn update_and_delete_with_covers_touch_only_the_visible_rows() {
    let f = fixture();
    let out = dispatch(
        &f.conn,
        &json!({"operation": "update", "table": "t", "set": {"label": "seen"},
                "where": {"aud": {"covers": ["e", "b"]}}}),
    )
    .unwrap();
    assert_eq!(out.error_code, None, "{:?}", out.error_text);
    assert_eq!(out.rows_affected as usize, expect(&["e", "b"]).len());
    assert_eq!(
        select_ids(&f.conn, json!({"label": "seen"})),
        expect(&["e", "b"])
    );

    let out = dispatch(
        &f.conn,
        &json!({"operation": "delete", "table": "t",
                "where": {"aud": {"covers": ["e", "a", "b"]}}}),
    )
    .unwrap();
    assert_eq!(out.error_code, None, "{:?}", out.error_text);
    assert_eq!(out.rows_affected, 2);
    let left = select_ids(&f.conn, json!({"id": {"neq": ""}}));
    assert!(!left.contains("r00") && !left.contains("r02"));
    assert_eq!(left.len(), AUD.len() + 2 - 2);
}

#[test]
fn search_with_covers_filters_next_to_the_full_text_match() {
    let f = fixture();
    let out = dispatch(
        &f.conn,
        &json!({"operation": "search", "table": "t", "match": "probe",
                "columns": ["id"], "where": {"aud": {"covers": ["e", "a"]}}}),
    )
    .unwrap();
    assert_eq!(out.error_code, None, "{:?}", out.error_text);
    assert_eq!(ids_of(&out), expect(&["e", "a"]));

    let out = dispatch(
        &f.conn,
        &json!({"operation": "search", "table": "t", "match": "probe",
                "columns": ["id"], "where": {"doc$.aud": {"covers": ["e", "a"]}}}),
    )
    .unwrap();
    assert_eq!(out.error_code, None, "{:?}", out.error_text);
    assert_eq!(ids_of(&out), expect(&["e", "a"]));
}

#[test]
fn a_malformed_round_is_invalid_input_naming_the_key() {
    let f = fixture();
    let long = "y".repeat(201);
    let many: Vec<String> = (0..65).map(|i| format!("w{i}")).collect();
    for round in [
        json!([]),
        json!("e"),
        json!(null),
        json!(["e", 1]),
        json!(["e", ""]),
        json!([long]),
        json!(many),
    ] {
        for op in ["select", "update", "delete", "search"] {
            let where_ = json!({"aud": {"covers": round.clone()}});
            let args = match op {
                "select" => {
                    json!({"operation": op, "table": "t", "columns": ["id"], "where": where_})
                }
                "update" => {
                    json!({"operation": op, "table": "t", "set": {"label": "z"}, "where": where_})
                }
                "delete" => json!({"operation": op, "table": "t", "where": where_}),
                _ => {
                    json!({"operation": op, "table": "t", "columns": ["id"], "match": "probe", "where": where_})
                }
            };
            let err = dispatch(&f.conn, &args).expect_err("a malformed round is invalid_input");
            assert!(err.contains("aud"), "{op} {round}: {err}");
            assert!(err.contains("covers"), "{op} {round}: {err}");
        }
    }
    // Nothing was written by the refused updates and deletes.
    assert_eq!(
        select_ids(&f.conn, json!({"id": {"neq": ""}})).len(),
        AUD.len() + 2
    );
}
