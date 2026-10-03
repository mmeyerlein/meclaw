//! GH #822 — an older writer schema is resolved by declaration, on the two
//! readers the substrate owns: `import` into a running cell
//! (`db_transfer::import_document`) and the mutation staging seeder
//! (`mutation::stage::seed_cell_db_if_present`).
//!
//! The resolver itself is table-tested in `schema_evolution`; what is pinned
//! here is that both readers route through it, at the place a caller sees:
//! the row in the `cell.db`, or the refusal with the column named.
//!
//! Four sentences per reader (plan E § 3): a missing column WITH a default is
//! filled, a missing column WITHOUT one is refused naming it, a changed type is
//! refused, an unknown column is refused. Plus: an export writes its version.
//!
//! And one exception on the import side only (orchestrator ruling B, review
//! N-B E #2): a provenance column -- `audience`, `audience_set`, `channel`,
//! `speaker`, `speaker_ref`, `origin_round` -- is NEVER filled from its default on import. A
//! part that lacks one is refused naming it, as before #822: the audience gate
//! of a running cell does not bend to a template's default. Birth and boot keep
//! the default (the template author decided it for their own seed).

use meclaw_colony::db_transfer::{TransferOutcome, dispatch};
use meclaw_core::serde_json::{Map, Value, json};

fn args(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

fn outcome(conn: &rusqlite::Connection, v: Value) -> TransferOutcome {
    dispatch(conn, &args(v)).unwrap()
}

/// A `cell.db` as the substrate makes one, with a `proposals` table the way a
/// `store` declares it since GH #822: `audience` and `origin_round` carry their
/// `DEFAULT ''`, and so does the plain content column `note`.
fn proposals_db(td: &tempfile::TempDir) -> rusqlite::Connection {
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE proposals (id TEXT PRIMARY KEY, status TEXT, n INTEGER, \
         audience TEXT DEFAULT '', origin_round TEXT DEFAULT '', note TEXT DEFAULT '');",
    )
    .unwrap();
    conn
}

fn audience_of(conn: &rusqlite::Connection, id: &str) -> Option<String> {
    conn.query_row("SELECT audience FROM proposals WHERE id = ?1", [id], |r| {
        r.get(0)
    })
    .unwrap()
}

fn count(conn: &rusqlite::Connection) -> i64 {
    conn.query_row("SELECT count(*) FROM proposals", [], |r| r.get(0))
        .unwrap()
}

fn refusal(o: TransferOutcome) -> (&'static str, String) {
    match o {
        TransferOutcome::Refused { code, detail } => (code, detail),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

// ─────────────────────────────────────────────────────────────── import

#[test]
fn import_a_missing_column_with_a_default_is_filled() {
    let td = tempfile::TempDir::new().unwrap();
    let conn = proposals_db(&td);
    match outcome(
        &conn,
        json!({"operation": "import", "table": "proposals",
               "schema": {"id": "text", "status": "text", "n": "int",
                          "audience": "text", "origin_round": "text"},
               "rows": [{"id": "p1", "status": "open", "n": 1,
                         "audience": "", "origin_round": ""}]}),
    ) {
        TransferOutcome::Done { rows_affected, .. } => assert_eq!(rows_affected, 1),
        other => panic!("a part from an older writer must land: {other:?}"),
    }
    let note: Option<String> = conn
        .query_row("SELECT note FROM proposals WHERE id = 'p1'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        note.as_deref(),
        Some(""),
        "the declared default, and not NULL"
    );
}

/// Ruling B: a provenance column is never filled from a default on import --
/// with or without a fail-closed `''`. A part without it is refused naming it,
/// exactly as before #822, and writes nothing.
#[test]
fn import_never_fills_a_provenance_column_from_its_default() {
    for missing in ["audience", "origin_round"] {
        let td = tempfile::TempDir::new().unwrap();
        let conn = proposals_db(&td);
        let mut schema = json!({"id": "text", "status": "text", "n": "int",
                                "audience": "text", "origin_round": "text"});
        let mut row = json!({"id": "p1", "status": "open", "n": 1,
                             "audience": "", "origin_round": ""});
        schema.as_object_mut().unwrap().remove(missing);
        row.as_object_mut().unwrap().remove(missing);
        let (code, detail) = refusal(outcome(
            &conn,
            json!({"operation": "import", "table": "proposals",
                   "schema": schema, "rows": [row]}),
        ));
        assert_eq!(code, "import_schema_drift", "{missing}");
        assert!(
            detail.contains(&format!("column {missing}")),
            "{missing}: {detail}"
        );
        assert_eq!(count(&conn), 0, "{missing}: a refused part writes nothing");
    }
}

/// Review N-B E (re-review minor, welle-nachlese Z): the collector's `turns`
/// carries `speaker_ref` beside `speaker` -- the legend key of a peer row,
/// provenance just as much. With a declared `DEFAULT ''` (none today, so no
/// leak yet) an import must not fill it either.
#[test]
fn import_never_fills_speaker_ref_from_its_default() {
    let td = tempfile::TempDir::new().unwrap();
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE turns (id TEXT PRIMARY KEY, text TEXT, \
         speaker TEXT DEFAULT '', speaker_ref TEXT DEFAULT '');",
    )
    .unwrap();
    let (code, detail) = refusal(outcome(
        &conn,
        json!({"operation": "import", "table": "turns",
               "schema": {"id": "text", "text": "text", "speaker": "text"},
               "rows": [{"id": "t1", "text": "hi", "speaker": ""}]}),
    ));
    assert_eq!(code, "import_schema_drift");
    assert!(detail.contains("column speaker_ref"), "{detail}");
    let n: i64 = conn
        .query_row("SELECT count(*) FROM turns", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0, "a refused part writes nothing");
}

#[test]
fn import_a_missing_column_without_default_is_refused_naming_it() {
    let td = tempfile::TempDir::new().unwrap();
    let conn = proposals_db(&td);
    let (code, detail) = refusal(outcome(
        &conn,
        json!({"operation": "import", "table": "proposals",
               "schema": {"id": "text", "n": "int", "audience": "text"},
               "rows": [{"id": "p1", "n": 1, "audience": ""}]}),
    ));
    assert_eq!(code, "import_schema_drift");
    assert!(
        detail.contains("schema_mismatch") && detail.contains("column status"),
        "{detail}"
    );
    assert_eq!(count(&conn), 0, "a refused part writes nothing");
}

#[test]
fn import_a_changed_type_is_refused() {
    let td = tempfile::TempDir::new().unwrap();
    let conn = proposals_db(&td);
    let (code, detail) = refusal(outcome(
        &conn,
        json!({"operation": "import", "table": "proposals",
               "schema": {"id": "text", "status": "text", "n": "text", "audience": "text"},
               "rows": [{"id": "p1", "status": "open", "n": "1", "audience": ""}]}),
    ));
    assert_eq!(code, "import_schema_drift");
    assert!(detail.contains("column n"), "{detail}");
    assert_eq!(count(&conn), 0);
}

#[test]
fn import_an_unknown_column_is_refused() {
    let td = tempfile::TempDir::new().unwrap();
    let conn = proposals_db(&td);
    let (code, detail) = refusal(outcome(
        &conn,
        json!({"operation": "import", "table": "proposals",
               "schema": {"id": "text", "status": "text", "n": "int", "audience": "text",
                          "origin_round": "text", "rumour": "text"},
               "rows": [{"id": "p1", "status": "open", "n": 1, "audience": "",
                         "origin_round": "", "rumour": "[]"}]}),
    ));
    assert_eq!(code, "import_schema_drift");
    assert!(detail.contains("column rumour"), "{detail}");
    assert_eq!(count(&conn), 0);
}

#[test]
fn import_a_newer_writer_version_is_refused() {
    let td = tempfile::TempDir::new().unwrap();
    let conn = proposals_db(&td);
    let (code, detail) = refusal(outcome(
        &conn,
        json!({"operation": "import", "table": "proposals", "version": 2,
               "schema": {"id": "text", "status": "text", "n": "int", "audience": "text"},
               "rows": []}),
    ));
    assert_eq!(code, "import_schema_drift");
    assert!(detail.contains("version 2"), "{detail}");
}

// ─────────────────────────────────────────────────────────────── export

#[test]
fn an_export_writes_the_version_of_its_table() {
    let td = tempfile::TempDir::new().unwrap();
    let conn = proposals_db(&td);
    let doc = |conn: &rusqlite::Connection| match outcome(
        conn,
        json!({"operation": "export", "table": "proposals"}),
    ) {
        TransferOutcome::Done { payload, .. } => payload,
        other => panic!("{other:?}"),
    };
    assert_eq!(doc(&conn)["version"], 1, "nothing recorded is version 1");
    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, '3')",
        [meclaw_colony::schema_evolution::version_meta_key(
            "proposals",
        )],
    )
    .unwrap();
    assert_eq!(doc(&conn)["version"], 3);
}

// ─────────────────────────────────────────────────────────────── staging

/// A staged `store` directory: the substituted `config.json` declares
/// `proposals` with a defaulted `audience`, and the seed is `body`.
fn staged(body: &str) -> tempfile::TempDir {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(td.path().join("seed")).unwrap();
    std::fs::write(
        td.path().join("config.json"),
        json!({"cell": {"type": "store"},
        "params": {"schema": {"proposals": {
            "id": "text", "status": "text", "n": "int",
            "audience": {"type": "text", "default": ""}
        }}}})
        .to_string(),
    )
    .unwrap();
    std::fs::write(td.path().join("seed/proposals.jsonl"), body).unwrap();
    td
}

fn stage(td: &tempfile::TempDir) -> Result<(), String> {
    meclaw_colony::mutation::stage::seed_cell_db_if_present(td.path(), "store", &Default::default())
        .map_err(|e| format!("{e:?}"))
}

#[test]
fn staging_a_missing_column_with_a_default_is_filled_at_birth() {
    let td = staged(concat!(
        r#"{"schema":{"id":"text","status":"text","n":"int"}}"#,
        "\n",
        r#"{"id":"p1","status":"accepted","n":1}"#,
        "\n"
    ));
    stage(&td).unwrap();
    let conn = rusqlite::Connection::open(td.path().join("cell.db")).unwrap();
    assert_eq!(audience_of(&conn, "p1").as_deref(), Some(""));
}

#[test]
fn staging_a_missing_column_without_default_is_refused_at_birth_naming_it() {
    let td = staged(concat!(
        r#"{"schema":{"id":"text","n":"int","audience":"text"}}"#,
        "\n"
    ));
    let err = stage(&td).unwrap_err();
    assert!(
        err.contains("schema_mismatch") && err.contains("column status"),
        "{err}"
    );
}

#[test]
fn staging_a_changed_type_is_refused() {
    let td = staged(concat!(
        r#"{"schema":{"id":"text","status":"text","n":"text","audience":"text"}}"#,
        "\n"
    ));
    let err = stage(&td).unwrap_err();
    assert!(err.contains("column n"), "{err}");
}

#[test]
fn staging_an_unknown_column_is_refused() {
    let td = staged(concat!(
        r#"{"schema":{"id":"text","status":"text","n":"int","audience":"text","x":"text"}}"#,
        "\n"
    ));
    let err = stage(&td).unwrap_err();
    assert!(err.contains("column x"), "{err}");
}

/// Not a `store`: no declaration to resolve against, and the seeder behaves
/// exactly as before GH #822 — the header alone builds the table.
#[test]
fn staging_another_cell_type_keeps_the_header_alone() {
    let td = staged(concat!(r#"{"schema":{"id":"text","x":"text"}}"#, "\n"));
    meclaw_colony::mutation::stage::seed_cell_db_if_present(
        td.path(),
        "timer",
        &Default::default(),
    )
    .unwrap();
}
