//! Synthesizes `CREATE TABLE IF NOT EXISTS` statements from the
//! validated 2-stage schema map. Sync (rusqlite); called by the
//! StoreCellFactory before `tokio::spawn(cell_task_stateful)`
//! (Phase-7.5 Ad-hoc-DDL-Pattern).

use std::collections::BTreeMap;

/// Validate an identifier that is about to be **created**.
///
/// The catalog cannot vet a name that does not exist yet, so `create_table` and
/// `params.schema` — the only two paths that format a fresh identifier into DDL —
/// gate it by syntax instead: ASCII word characters, not starting with a digit,
/// at most 63 characters, no `sqlite_` prefix (SQLite's own namespace) and no
/// `_fts` suffix (reserved for the FTS index tables of P3).
pub fn check_new_identifier(kind: &str, name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 63
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !ok {
        return Err(format!(
            "{kind} {name:?}: only [A-Za-z_][A-Za-z0-9_]{{0,62}} allowed"
        ));
    }
    let lowered = name.to_ascii_lowercase();
    if lowered.starts_with("sqlite_") {
        return Err(format!("{kind} {name:?}: sqlite_ prefix is reserved"));
    }
    if lowered.ends_with("_fts") {
        return Err(format!(
            "{kind} {name:?}: _fts suffix is reserved for FTS indexes"
        ));
    }
    Ok(())
}

/// Map declared schema-type strings to SQLite column types.
fn sqlite_type(t: &str) -> &'static str {
    match t {
        "int" => "INTEGER",
        "text" => "TEXT",
        "json" => "TEXT", // SQLite JSON1 lives in TEXT columns by convention.
        _ => unreachable!("StoreParams::parse validates allowed types"),
    }
}

/// Apply schema DDL. Idempotent (`CREATE TABLE IF NOT EXISTS`). Order
/// is deterministic because the schema is a BTreeMap.
pub fn apply_schema_ddl(
    conn: &rusqlite::Connection,
    schema: &BTreeMap<String, BTreeMap<String, String>>,
) -> rusqlite::Result<()> {
    for (table, cols) in schema {
        let col_clause = cols
            .iter()
            .map(|(c, t)| format!("\"{c}\" {}", sqlite_type(t)))
            .collect::<Vec<_>>()
            .join(", ");
        let stmt = format!("CREATE TABLE IF NOT EXISTS \"{table}\" ({col_clause})");
        conn.execute(&stmt, [])?;
    }
    Ok(())
}

/// Apply the FTS5 index DDL for the declared tables (P3).
///
/// Strategy: **external content** (`content='<table>'`) plus insert/update/delete
/// triggers. Contentless FTS5 cannot be updated or rebuilt, and the store very
/// much has an `update` op (the memory lanes supersede facts and update
/// beliefs), so external content is the only shape that stays correct — and it
/// stores an index, not a copy of the text.
///
/// Idempotent by design, which is also how an existing `cell.db` catches up:
/// if the index table is missing it is created, the triggers are (re-)declared
/// and the index is rebuilt **once** from the base table, so rows written before
/// the declaration become searchable. If it is already there, nothing happens —
/// no rebuild on every boot.
///
/// A pre-existing index whose column list differs from the declaration is a loud
/// error: silently serving a stale index shape would be worse, and dropping it
/// is not this function's call.
///
/// Identifiers come from `params.schema`, which is syntax-gated by
/// [`check_new_identifier`] at parse time.
pub fn apply_fts_ddl(
    conn: &rusqlite::Connection,
    fts: &BTreeMap<String, Vec<String>>,
) -> Result<(), String> {
    for (table, cols) in fts {
        let index = format!("{table}_fts");
        let existing = existing_fts_columns(conn, &index).map_err(|e| e.to_string())?;
        if let Some(found) = existing {
            if found != *cols {
                return Err(format!(
                    "fts column drift on {index}: declared {cols:?}, existing {found:?} — \
                     resolve manually (P3 does not drop or migrate index tables)"
                ));
            }
            continue;
        }
        let col_list = cols
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let new_vals = cols
            .iter()
            .map(|c| format!("new.\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let old_vals = cols
            .iter()
            .map(|c| format!("old.\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let ddl = format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS \"{index}\" USING fts5({col_list}, \
               content='{table}', content_rowid='rowid');
             CREATE TRIGGER IF NOT EXISTS \"{index}_ai\" AFTER INSERT ON \"{table}\" BEGIN
               INSERT INTO \"{index}\"(rowid, {col_list}) VALUES (new.rowid, {new_vals});
             END;
             CREATE TRIGGER IF NOT EXISTS \"{index}_ad\" AFTER DELETE ON \"{table}\" BEGIN
               INSERT INTO \"{index}\"(\"{index}\", rowid, {col_list})
                 VALUES ('delete', old.rowid, {old_vals});
             END;
             CREATE TRIGGER IF NOT EXISTS \"{index}_au\" AFTER UPDATE ON \"{table}\" BEGIN
               INSERT INTO \"{index}\"(\"{index}\", rowid, {col_list})
                 VALUES ('delete', old.rowid, {old_vals});
               INSERT INTO \"{index}\"(rowid, {col_list}) VALUES (new.rowid, {new_vals});
             END;
             INSERT INTO \"{index}\"(\"{index}\") VALUES ('rebuild');"
        );
        conn.execute_batch(&ddl).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The column list of an existing FTS index table, or `None` if it does not exist.
fn existing_fts_columns(
    conn: &rusqlite::Connection,
    index: &str,
) -> rusqlite::Result<Option<Vec<String>>> {
    use rusqlite::OptionalExtension;
    let present: Option<String> = conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type='table' AND name = ?1",
            [index],
            |r| r.get(0),
        )
        .optional()?;
    if present.is_none() {
        return Ok(None);
    }
    let mut st = conn.prepare("SELECT name FROM pragma_table_info(?1)")?;
    let cols = st
        .query_map([index], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(Some(cols))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Tripwire: FTS5 must be compiled into the bundled SQLite. `libsqlite3-sys`
    /// passes `-DSQLITE_ENABLE_FTS5` unconditionally in the bundled build, so this
    /// needs no `Cargo.toml` feature — but a dependency bump could silently drop
    /// it, and the whole `search` op rests on it. Empirical, not inferred.
    #[test]
    fn fts5_is_compiled_in() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE VIRTUAL TABLE probe USING fts5(x);\
             INSERT INTO probe(x) VALUES('hello world');",
        )
        .expect("bundled SQLite must ship FTS5");
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM probe WHERE probe MATCH 'world'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "FTS5 MATCH must return the indexed row");
    }

    fn fts_decl(table: &str, cols: &[&str]) -> BTreeMap<String, Vec<String>> {
        BTreeMap::from([(
            table.to_string(),
            cols.iter().map(|c| c.to_string()).collect(),
        )])
    }

    #[test]
    fn fts_ddl_creates_index_triggers_and_backfills_existing_rows() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        // an "old" cell.db: base table with data, no FTS anywhere
        conn.execute("CREATE TABLE facts (id TEXT, claim TEXT)", [])
            .unwrap();
        conn.execute("INSERT INTO facts VALUES ('f1','acme ships v1')", [])
            .unwrap();

        apply_fts_ddl(&conn, &fts_decl("facts", &["claim"])).unwrap();

        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts_fts WHERE facts_fts MATCH 'ships'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            n, 1,
            "existing rows must be indexed by the one-time rebuild"
        );

        // triggers keep the index in sync with insert / update / delete
        conn.execute("INSERT INTO facts VALUES ('f2','acme builds alpha')", [])
            .unwrap();
        conn.execute(
            "UPDATE facts SET claim='acme builds beta' WHERE id='f2'",
            [],
        )
        .unwrap();
        let hit: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts_fts WHERE facts_fts MATCH 'beta'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hit, 1);
        let stale: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts_fts WHERE facts_fts MATCH 'alpha'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stale, 0, "update must not leave a stale index row");
        conn.execute("DELETE FROM facts WHERE id='f2'", []).unwrap();
        let gone: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts_fts WHERE facts_fts MATCH 'beta'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(gone, 0, "delete must remove the index row");

        // second call is a no-op: no error, and no second rebuild
        apply_fts_ddl(&conn, &fts_decl("facts", &["claim"])).unwrap();
        let again: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts_fts WHERE facts_fts MATCH 'ships'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(again, 1, "rebuild must not run twice");
    }

    #[test]
    fn fts_ddl_fails_loudly_on_column_drift() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE facts (id TEXT, claim TEXT, note TEXT)", [])
            .unwrap();
        apply_fts_ddl(&conn, &fts_decl("facts", &["claim"])).unwrap();
        let e = apply_fts_ddl(&conn, &fts_decl("facts", &["claim", "note"])).unwrap_err();
        assert!(e.contains("fts column drift"), "got {e}");
    }

    #[test]
    fn fts_ddl_without_declaration_is_a_noop() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE facts (id TEXT)", []).unwrap();
        apply_fts_ddl(&conn, &BTreeMap::new()).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name LIKE '%_fts%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn applies_two_tables_with_types() {
        let mut schema = BTreeMap::new();
        let mut t1 = BTreeMap::new();
        t1.insert("id".to_string(), "int".to_string());
        t1.insert("name".to_string(), "text".to_string());
        schema.insert("users".to_string(), t1);
        let mut t2 = BTreeMap::new();
        t2.insert("body".to_string(), "json".to_string());
        schema.insert("events".to_string(), t2);

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        apply_schema_ddl(&conn, &schema).unwrap();

        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('users','events')",
                [], |r| r.get(0),
            ).unwrap();
        assert_eq!(n, 2);
    }
}
