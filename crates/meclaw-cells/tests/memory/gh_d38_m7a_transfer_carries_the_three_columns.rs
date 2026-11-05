//! GH #1421 (M7-A) -- `stance`, `via` and `answers` travel, and a memory from
//! before them still arrives.
//!
//! On the shipped memory-hive store schema: a transfer round trip (`export`
//! from one store, `import` into another) carries the three columns as they
//! were written; a part written by a store from before the columns imports,
//! and its rows read empty -- `asserted`, no source, no reply; and a
//! `cell.db` of such a store grows the columns at boot without losing a row.
//! The porter walks `facts`, so this is the whole of what its transfer needs.

// harness_fold: #[path = "support/memory_hive_subject.rs"]
#[allow(unused_imports)]
use crate::support; // harness_fold: mod support;

use meclaw_cells::store::StoreParams;
use meclaw_cells::store::ddl::{
    apply_canonical_ddl, apply_declared_schema_ddl, apply_fts_ddl, apply_index_ddl,
};
use meclaw_colony::db_transfer::{TransferOutcome, dispatch as transfer};
use meclaw_core::serde_json::{Map, Value, json};
use support::*;

const NEW: [&str; 3] = ["stance", "via", "answers"];

fn shipped_params() -> Value {
    read_json(std::path::Path::new(STORE))["params"].clone()
}

/// The shipped params with the three columns taken out: the store as it was.
fn old_params() -> Value {
    let mut p = shipped_params();
    let schema = p["schema"]["facts"].as_object_mut().expect("facts schema");
    for c in NEW {
        schema.remove(c);
    }
    p
}

fn open(conn: &rusqlite::Connection, params: &Value) {
    let p = StoreParams::parse(params).expect("the store params parse");
    apply_declared_schema_ddl(conn, &p).expect("schema");
    apply_index_ddl(conn, &p.indexes).expect("indexes");
    apply_canonical_ddl(conn, &p.canonical).expect("canonical");
    apply_fts_ddl(conn, &p.fts, &p.canonical).expect("fts");
}

fn store(params: &Value) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().expect("open");
    meclaw_cells::store::query::install_connection_extensions(&conn).expect("extensions");
    open(&conn, params);
    conn
}

fn args(v: Value) -> Map<String, Value> {
    v.as_object().expect("an object").clone()
}

fn done(conn: &rusqlite::Connection, v: Value) -> (i64, Value) {
    match transfer(conn, &args(v)).expect("well formed") {
        TransferOutcome::Done {
            rows_affected,
            payload,
        } => (rows_affected, payload),
        other => panic!("expected Done, got {other:?}"),
    }
}

fn insert(conn: &rusqlite::Connection, params: &Value, row: Value) {
    let p = StoreParams::parse(params).expect("params");
    let out = meclaw_cells::store::ops::dispatch_with(
        conn,
        &json!({"operation": "insert", "table": "facts", "row": row}),
        &p.canonical,
    )
    .expect("the op is well formed");
    assert_eq!(out.error_code, None, "{:?}", out.error_text);
}

fn base(id: &str, claim: &str) -> Value {
    json!({"id": id, "episode_id": "ep-1", "session_id": "s-1", "channel": "direct:e",
           "audience_set": M7A_AUDIENCE, "subject": "member", "predicate": "has_mole",
           "claim": claim, "claim_hash": id, "fact_kind": "world",
           "valid_from": "2026-11-05T10:00:00Z", "recorded_at": "2026-11-05T10:00:00Z",
           "confidence": 70, "source": ""})
}

fn column(conn: &rusqlite::Connection, id: &str, col: &str) -> Option<String> {
    conn.query_row(
        &format!("SELECT {col} FROM facts WHERE id = ?1"),
        [id],
        |r| r.get::<_, Option<String>>(0),
    )
    .expect("the row")
}

#[test]
fn a_round_trip_carries_the_three_columns() {
    let params = shipped_params();
    let source = store(&params);
    let mut said = base("f-said", "harmless");
    said["stance"] = json!("reported");
    said["via"] = json!("the doctor");
    insert(&source, &params, said);
    let mut reply = base("f-reply", "worth showing to a doctor");
    reply["source"] = json!("agent:self");
    reply["stance"] = json!("asserted");
    reply["answers"] = json!("f-said");
    insert(&source, &params, reply);

    let (n, doc) = done(
        &source,
        json!({"operation": "export", "table": "facts", "key": ["id"]}),
    );
    assert_eq!(n, 2);
    for c in NEW {
        assert!(
            doc["schema"].get(c).is_some(),
            "the part declares {c}: {doc}"
        );
    }
    let target = store(&params);
    let (written, _) = done(
        &target,
        json!({"operation": "import", "table": "facts", "key": doc["key"],
               "schema": doc["schema"], "rows": doc["rows"], "version": doc["version"]}),
    );
    assert_eq!(written, 2);
    assert_eq!(
        column(&target, "f-said", "stance").as_deref(),
        Some("reported")
    );
    assert_eq!(
        column(&target, "f-said", "via").as_deref(),
        Some("the doctor")
    );
    assert_eq!(
        column(&target, "f-reply", "answers").as_deref(),
        Some("f-said")
    );
}

#[test]
fn a_part_from_before_the_columns_imports_and_reads_empty() {
    let old = old_params();
    let source = store(&old);
    insert(&source, &old, base("f-old", "small"));
    let (_, doc) = done(
        &source,
        json!({"operation": "export", "table": "facts", "key": ["id"]}),
    );
    for c in NEW {
        assert!(doc["schema"].get(c).is_none(), "an old part: {doc}");
    }
    let target = store(&shipped_params());
    let (written, _) = done(
        &target,
        json!({"operation": "import", "table": "facts", "key": doc["key"],
               "schema": doc["schema"], "rows": doc["rows"], "version": doc["version"]}),
    );
    assert_eq!(
        written, 1,
        "an additive column never refuses an older memory"
    );
    for c in NEW {
        assert_eq!(column(&target, "f-old", c).as_deref(), Some(""), "{c}");
    }
}

#[test]
fn an_old_store_grows_the_columns_at_boot_and_keeps_its_rows() {
    let old = old_params();
    let conn = store(&old);
    insert(&conn, &old, base("f-old", "small"));
    open(&conn, &shipped_params());
    for c in NEW {
        assert_eq!(column(&conn, "f-old", c).as_deref(), Some(""), "{c}");
    }
    assert_eq!(column(&conn, "f-old", "claim").as_deref(), Some("small"));
}
