//! GH #253 — the two constraints the transfer core has to survive, proven on
//! the cell type that actually has them: the `store`.
//!
//! The core itself (`meclaw_colony::db_transfer`) is type-agnostic and pinned in
//! `crates/meclaw-colony/tests/gh253_db_transfer.rs`. What can only be shown
//! here is that it survives the two things a real store does that a bare
//! `cell.db` does not:
//!
//! * an FTS5 index built with the `meclaw_stem_v1` tokenizer, whose triggers can
//!   only run on a connection that has the tokenizer registered — which is
//!   exactly why the substrate runs a transfer on the cell's OWN `DbConn`;
//! * the provenance columns of 0.16.0 (`audience_set`, `channel`, `speaker`),
//!   which a transfer must not be able to drop.

use meclaw_cells::store::StoreParams;
use meclaw_cells::store::ddl::{apply_fts_ddl, apply_schema_ddl};
use meclaw_cells::store::ops::dispatch;
use meclaw_colony::db_transfer::{TransferOutcome, dispatch as transfer};
use meclaw_core::serde_json::{Map, Value, json};

fn args(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

fn done(conn: &rusqlite::Connection, v: Value) -> (i64, Value) {
    match transfer(conn, &args(v)).unwrap() {
        TransferOutcome::Done {
            rows_affected,
            payload,
        } => (rows_affected, payload),
        other => panic!("expected Done, got {other:?}"),
    }
}

/// The memory hive's `facts` shape reduced to what a transfer has to carry: an
/// identity, an indexed claim, and the three provenance columns.
fn facts_params() -> StoreParams {
    StoreParams::parse(&json!({
        "schema": {"facts": {
            "id": "text", "claim": "text",
            "audience_set": "json", "channel": "text", "speaker": "text"
        }},
        "fts": {"facts": ["claim"]}
    }))
    .unwrap()
}

/// A store connection exactly as the factory builds one — extensions first, so
/// the FTS index can declare the stemming tokenizer at all.
fn store(params: &StoreParams) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    meclaw_cells::store::query::install_connection_extensions(&conn).unwrap();
    apply_schema_ddl(&conn, &params.schema).unwrap();
    apply_fts_ddl(&conn, &params.fts, &params.canonical).unwrap();
    conn
}

fn insert(conn: &rusqlite::Connection, id: &str, claim: &str, audience: &str) {
    let out = dispatch(
        conn,
        &json!({"operation": "insert", "table": "facts",
                "row": {"id": id, "claim": claim, "audience_set": audience,
                        "channel": "room:one", "speaker": "member:alex"}}),
    )
    .unwrap();
    assert_eq!(out.error_code, None, "{:?}", out.error_text);
}

fn search(conn: &rusqlite::Connection, matcher: &str) -> usize {
    let out = dispatch(
        conn,
        &json!({"operation": "search", "table": "facts",
                "columns": ["id"], "match": matcher}),
    )
    .unwrap();
    assert_eq!(out.error_code, None, "{:?}", out.error_text);
    out.payload.as_array().unwrap().len()
}

#[test]
fn an_imported_row_is_searchable_the_moment_it_lands() {
    let p = facts_params();
    let conn = store(&p);

    let (written, _) = done(
        &conn,
        json!({"operation": "import", "table": "facts", "key": ["id"],
               "schema": {"id": "text", "claim": "text", "audience_set": "json",
                          "channel": "text", "speaker": "text"},
               "rows": [{"id": "f1", "claim": "alex prefers helix editors",
                         "audience_set": "[\"member:alex\"]", "channel": "room:one",
                         "speaker": "member:alex"}]}),
    );
    assert_eq!(written, 1);

    // Plural in the query, singular in the row: only an index folded by
    // `meclaw_stem_v1` answers this, and the trigger that filled it ran on the
    // same connection the import wrote through.
    assert_eq!(
        search(&conn, "\"editoren\"*"),
        1,
        "an imported row must be in the FTS index, folded by the store's own tokenizer"
    );
}

#[test]
fn the_fts_index_is_not_offered_as_content_and_cannot_be_imported_into() {
    let p = facts_params();
    let conn = store(&p);
    let (_, doc) = done(&conn, json!({"operation": "export"}));
    let tables: Vec<&str> = doc["tables"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    // This connection is a bare store schema (no `setup_cell_db`), so `facts`
    // is the whole content list: the FTS index and its four shadow tables are
    // derived data the receiving cell rebuilds from its own rows.
    assert_eq!(tables, vec!["facts"]);

    match transfer(
        &conn,
        &args(json!({"operation": "import", "table": "facts_fts",
                     "schema": {"claim": "text"}, "rows": []})),
    )
    .unwrap()
    {
        TransferOutcome::Refused { code, .. } => assert_eq!(code, "unknown_table"),
        other => panic!("an index must not be an import target: {other:?}"),
    }
}

#[test]
fn a_round_trip_between_two_stores_carries_the_participant_set() {
    let p = facts_params();
    let source = store(&p);
    insert(&source, "f1", "alpha", "[\"member:alex\"]");
    insert(&source, "f2", "beta", "[\"*\"]");

    let (rows, doc) = done(
        &source,
        json!({"operation": "export", "table": "facts", "key": ["id"]}),
    );
    assert_eq!(rows, 2);
    // Every column travels — nothing is projected away, provenance least of all.
    assert_eq!(doc["rows"][0]["audience_set"], "[\"member:alex\"]");
    assert_eq!(doc["rows"][0]["channel"], "room:one");
    assert_eq!(doc["rows"][0]["speaker"], "member:alex");

    let target = store(&p);
    let (written, _) = done(
        &target,
        json!({"operation": "import", "table": "facts", "key": doc["key"],
               "schema": doc["schema"], "rows": doc["rows"]}),
    );
    assert_eq!(written, 2);

    let (_, there) = done(
        &target,
        json!({"operation": "export", "table": "facts", "key": ["id"]}),
    );
    assert_eq!(
        there["rows"], doc["rows"],
        "what left the source is what the target holds, provenance included"
    );
    assert_eq!(
        search(&target, "\"alpha\"*"),
        1,
        "and it is searchable in the target too"
    );
}

#[test]
fn a_part_that_lost_the_audience_column_is_refused_whole() {
    let p = facts_params();
    let conn = store(&p);
    match transfer(
        &conn,
        &args(
            json!({"operation": "import", "table": "facts", "key": ["id"],
                     "schema": {"id": "text", "claim": "text", "channel": "text",
                                "speaker": "text"},
                     "rows": [{"id": "f1", "claim": "alpha", "channel": "room:one",
                               "speaker": "member:alex"}]}),
        ),
    )
    .unwrap()
    {
        TransferOutcome::Refused { code, detail } => {
            assert_eq!(code, "import_schema_drift");
            assert!(
                detail.contains("audience_set"),
                "the refusal names the column that did not survive: {detail}"
            );
        }
        other => panic!("an untagged import must be refused: {other:?}"),
    }
    let n: i64 = conn
        .query_row("SELECT count(*) FROM facts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0, "a refused part writes nothing at all");
}
