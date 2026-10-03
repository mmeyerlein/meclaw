//! Seed-Loader for Phase-9 store. Reads `<cell_dir>/seed/<table>.jsonl`
//! (one file per table) and inserts rows. Called by the factory after
//! `apply_schema_ddl` ONLY when the cell.db was freshly created
//! (`OpenStatus::Created` — see brainstorm E4).
//!
//! Format (per overview Z.1193–1213):
//!   Line 1: `{"schema": {"<col>": "<type>", ...}, "version"?: n}` — the
//!           schema the rows were WRITTEN with (GH #822; no version = 1)
//!   Lines 2+: data rows as JSON-objects keyed by column name.
//!
//! GH #822: line 1 is resolved against the store's declaration by
//! `meclaw_colony::schema_evolution::resolve` — the one resolver the `llm` and
//! `web` loaders, the staging seeder and `import` share. A declared column the
//! header lacks takes its declared default; one without a default, a changed
//! storage class, an undeclared header column and a newer header version are
//! refused as `schema_mismatch` naming the column.

use crate::store::ops::json_to_sql_value;
use meclaw_colony::schema_evolution::{self, ResolvePlan, TableDecl};
use meclaw_core::serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Filesystem location of the seed file of one declared table.
fn seed_path(cell_dir: &std::path::Path, table: &str) -> std::path::PathBuf {
    cell_dir.join("seed").join(format!("{table}.jsonl"))
}

/// Parse ONE seed file into its data rows plus the resolution of its header
/// against the declaration (GH #822). Every further non-empty line must be a
/// JSON object.
///
/// Pure parse — no database, no side effects. Issue #56: this is the single
/// parse path shared by the static check ([`check_declared_seed_files`], run in
/// the `--validate` / bootstrap-plan phase and at factory spawn) and the loader
/// ([`load_declared_seed_if_present`], run at the first wake). Keeping them on
/// one path is what makes the validate-equals-spawn invariant hold: a seed file
/// that survives validation always parses at wake.
fn parse_seed_file(
    path: &std::path::Path,
    table: &str,
    decl: &TableDecl,
) -> Result<(Vec<Map<String, Value>>, ResolvePlan), String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("seed read {}: {e}", path.display()))?;
    let mut lines = text.lines();
    let header_line = lines
        .next()
        .ok_or_else(|| format!("seed {}: empty file", path.display()))?;
    let header: Value = meclaw_core::serde_json::from_str(header_line).map_err(|e| {
        format!(
            "seed {} line 1: expected the schema header {{\"schema\":{{…}}}}, got invalid JSON: {e}",
            path.display()
        )
    })?;
    let header_obj = header
        .as_object()
        .filter(|h| h.get("schema").is_some_and(Value::is_object))
        .ok_or_else(|| {
            format!(
                "seed {} line 1: missing schema object — line 1 must be the header \
                 {{\"schema\":{{…}}}}, not a data row",
                path.display()
            )
        })?;
    let writer = schema_evolution::parse_writer_header(table, header_obj)
        .map_err(|e| format!("seed {}: {e}", path.display()))?;
    let plan = schema_evolution::resolve(table, &writer, decl)
        .map_err(|e| format!("seed {}: {e}", path.display()))?;
    let mut rows = Vec::new();
    for (idx, line) in lines.enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let row: Value = meclaw_core::serde_json::from_str(line)
            .map_err(|e| format!("seed {} line {}: {e}", path.display(), idx + 2))?;
        let row_obj = row.as_object().ok_or_else(|| {
            format!(
                "seed {} line {}: row must be JSON object",
                path.display(),
                idx + 2
            )
        })?;
        rows.push(row_obj.clone());
    }
    Ok((rows, plan))
}

/// Static, database-free parse check of every seed file below `cell_dir`.
///
/// Issue #56: a `seed/<table>.jsonl` is a statically parseable part of a cell's
/// configuration, so a syntactic mistake in it must surface where every other
/// static mistake surfaces — during `--validate` and at factory spawn — not as
/// a panic on the store cell's first wake.
///
/// Checked per declared table (a missing seed file stays legal): the header
/// line is present and is a `{"schema": {…}}` object, it resolves against the
/// declaration (GH #822), and every data line is a JSON object. Data VALUES are
/// deliberately not type-checked — "statically parseable" is the bar here.
pub fn check_declared_seed_files(
    cell_dir: &std::path::Path,
    params: &crate::store::StoreParams,
) -> Result<(), String> {
    for table in params.schema.keys() {
        let path = seed_path(cell_dir, table);
        if !path.exists() {
            continue;
        }
        if let Some(decl) = params.table_decl(table) {
            parse_seed_file(&path, table, &decl)?;
        }
    }
    Ok(())
}

/// [`check_declared_seed_files`] for a declaration in the short form only (no
/// defaults, every table version 1).
pub fn check_seed_files(
    cell_dir: &std::path::Path,
    schema: &BTreeMap<String, BTreeMap<String, String>>,
) -> Result<(), String> {
    for (table, cols) in schema {
        let path = seed_path(cell_dir, table);
        if !path.exists() {
            continue;
        }
        parse_seed_file(&path, table, &schema_evolution::table_decl_of_types(cols))?;
    }
    Ok(())
}

/// Load seed JSONL for every table declared in `params.schema`. Missing seed
/// file is OK (silent skip). Per-file: full parse first (header resolution +
/// data rows, see [`parse_seed_file`]), then the inserts using rusqlite
/// parameter binding — so a parse error never leaves a half-seeded table
/// behind. Every declared column is written: from the row when the writer had
/// the column, from its declared default when it did not (GH #822).
///
/// Called by `StoreCellFactory`'s `WakeFn` ONLY when `OpenStatus::Created`
/// (brainstorm E4 — fresh-only, never on resume).
pub fn load_declared_seed_if_present(
    conn: &rusqlite::Connection,
    cell_dir: &std::path::Path,
    params: &crate::store::StoreParams,
) -> Result<(), String> {
    for table in params.schema.keys() {
        if let Some(decl) = params.table_decl(table) {
            load_one(conn, cell_dir, table, &decl)?;
        }
    }
    Ok(())
}

/// [`load_declared_seed_if_present`] for a declaration in the short form only.
pub fn load_seed_if_present(
    conn: &rusqlite::Connection,
    cell_dir: &std::path::Path,
    schema: &BTreeMap<String, BTreeMap<String, String>>,
) -> Result<(), String> {
    for (table, cols) in schema {
        load_one(
            conn,
            cell_dir,
            table,
            &schema_evolution::table_decl_of_types(cols),
        )?;
    }
    Ok(())
}

fn load_one(
    conn: &rusqlite::Connection,
    cell_dir: &std::path::Path,
    table: &str,
    decl: &TableDecl,
) -> Result<(), String> {
    let path = seed_path(cell_dir, table);
    if !path.exists() {
        return Ok(());
    }
    let (rows, plan) = parse_seed_file(&path, table, decl)?;
    let col_names: Vec<&String> = decl.columns.keys().collect();
    let placeholders = col_names.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let col_list = col_names
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(",");
    let stmt = format!("INSERT INTO \"{table}\" ({col_list}) VALUES ({placeholders})");
    for (idx, row_obj) in rows.iter().enumerate() {
        let params: Vec<rusqlite::types::Value> = col_names
            .iter()
            .map(|c| match plan.fill.get(c.as_str()) {
                Some(default) => json_to_sql_value(Some(default)),
                None => json_to_sql_value(row_obj.get(c.as_str())),
            })
            .collect();
        let bind: Vec<&dyn rusqlite::ToSql> =
            params.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
        conn.execute(&stmt, bind.as_slice()).map_err(|e| {
            format!(
                "seed {} row {}: insert failed: {e}",
                path.display(),
                idx + 1
            )
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn loads_two_rows_into_existing_table() {
        let td = tempfile::TempDir::new().unwrap();
        let seed_dir = td.path().join("seed");
        std::fs::create_dir_all(&seed_dir).unwrap();
        std::fs::write(
            seed_dir.join("items.jsonl"),
            r#"{"schema":{"id":"int","name":"text"}}
{"id":1,"name":"alice"}
{"id":2,"name":"bob"}
"#,
        )
        .unwrap();

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE items (id INTEGER, name TEXT)", [])
            .unwrap();

        let mut schema = BTreeMap::new();
        let mut cols = BTreeMap::new();
        cols.insert("id".to_string(), "int".to_string());
        cols.insert("name".to_string(), "text".to_string());
        schema.insert("items".to_string(), cols);

        load_seed_if_present(&conn, td.path(), &schema).unwrap();

        let cnt: i64 = conn
            .query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cnt, 2);
    }

    #[test]
    fn missing_seed_file_is_ok() {
        let td = tempfile::TempDir::new().unwrap();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE items (id INTEGER)", []).unwrap();
        let mut schema = BTreeMap::new();
        let mut cols = BTreeMap::new();
        cols.insert("id".to_string(), "int".to_string());
        schema.insert("items".to_string(), cols);
        load_seed_if_present(&conn, td.path(), &schema).unwrap();
    }

    #[test]
    fn rejects_schema_mismatch() {
        let td = tempfile::TempDir::new().unwrap();
        let seed_dir = td.path().join("seed");
        std::fs::create_dir_all(&seed_dir).unwrap();
        std::fs::write(
            seed_dir.join("items.jsonl"),
            r#"{"schema":{"other":"text"}}
"#,
        )
        .unwrap();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE items (id INTEGER)", []).unwrap();
        let mut schema = BTreeMap::new();
        let mut cols = BTreeMap::new();
        cols.insert("id".to_string(), "int".to_string());
        schema.insert("items".to_string(), cols);
        let r = load_seed_if_present(&conn, td.path(), &schema);
        assert!(r.is_err());
    }

    // ---- Issue #56: the static (database-free) seed check. ----

    /// `items(id, name)` — the schema the checks below run against.
    fn items_schema() -> BTreeMap<String, BTreeMap<String, String>> {
        let mut schema = BTreeMap::new();
        let mut cols = BTreeMap::new();
        cols.insert("id".to_string(), "int".to_string());
        cols.insert("name".to_string(), "text".to_string());
        schema.insert("items".to_string(), cols);
        schema
    }

    fn td_with_seed(body: &str) -> tempfile::TempDir {
        let td = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(td.path().join("seed")).unwrap();
        std::fs::write(td.path().join("seed/items.jsonl"), body).unwrap();
        td
    }

    #[test]
    fn check_seed_files_accepts_well_formed_file() {
        let td = td_with_seed(
            r#"{"schema":{"id":"int","name":"text"}}
{"id":1,"name":"a"}
{"id":2,"name":"b"}
"#,
        );
        check_seed_files(td.path(), &items_schema()).unwrap();
    }

    #[test]
    fn check_seed_files_accepts_absent_file() {
        let td = tempfile::TempDir::new().unwrap();
        check_seed_files(td.path(), &items_schema()).unwrap();
    }

    /// The issue's reproducer: line 1 of the seed file removed.
    #[test]
    fn check_seed_files_rejects_missing_header() {
        let td = td_with_seed(
            r#"{"id":1,"name":"a"}
{"id":2,"name":"b"}
"#,
        );
        let err = check_seed_files(td.path(), &items_schema())
            .expect_err("a data row on line 1 is not a schema header");
        assert!(err.contains("schema"), "error must name the header: {err}");
    }

    #[test]
    fn check_seed_files_rejects_column_mismatch() {
        let td = td_with_seed(
            r#"{"schema":{"id":"int"}}
{"id":1}
"#,
        );
        let err = check_seed_files(td.path(), &items_schema())
            .expect_err("header must cover every declared column");
        assert!(err.contains("name"), "error must name the column: {err}");
    }

    #[test]
    fn check_seed_files_rejects_unparseable_data_row() {
        let td = td_with_seed(
            r#"{"schema":{"id":"int","name":"text"}}
{"id":1,"name":
"#,
        );
        assert!(check_seed_files(td.path(), &items_schema()).is_err());
    }

    #[test]
    fn check_seed_files_rejects_non_object_data_row() {
        let td = td_with_seed(
            r#"{"schema":{"id":"int","name":"text"}}
[1,"a"]
"#,
        );
        assert!(check_seed_files(td.path(), &items_schema()).is_err());
    }

    #[test]
    fn check_seed_files_rejects_empty_file() {
        let td = td_with_seed("");
        assert!(check_seed_files(td.path(), &items_schema()).is_err());
    }

    // ---- GH #822: the header is resolved against the declaration. ----

    fn declared() -> crate::store::StoreParams {
        crate::store::StoreParams::parse(&meclaw_core::serde_json::json!({"schema": {"items": {
            "id": "int",
            "name": "text",
            "audience": {"type": "text", "default": ""}
        }}}))
        .unwrap()
    }

    #[test]
    fn a_missing_column_with_a_default_is_filled_at_birth() {
        let td = td_with_seed(
            r#"{"schema":{"id":"int","name":"text"}}
{"id":1,"name":"a"}
"#,
        );
        let p = declared();
        check_declared_seed_files(td.path(), &p).unwrap();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::ddl::apply_declared_schema_ddl(&conn, &p).unwrap();
        load_declared_seed_if_present(&conn, td.path(), &p).unwrap();
        let aud: Option<String> = conn
            .query_row("SELECT audience FROM items WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(aud.as_deref(), Some(""), "the declared default, not NULL");
    }

    #[test]
    fn a_missing_column_without_default_is_refused_at_birth_naming_it() {
        let td = td_with_seed(
            r#"{"schema":{"id":"int","audience":"text"}}
{"id":1,"audience":""}
"#,
        );
        let err = check_declared_seed_files(td.path(), &declared()).unwrap_err();
        assert!(
            err.contains("schema_mismatch") && err.contains("column name"),
            "{err}"
        );
    }

    #[test]
    fn a_changed_type_is_refused() {
        let td = td_with_seed(
            r#"{"schema":{"id":"text","name":"text","audience":"text"}}
"#,
        );
        let err = check_declared_seed_files(td.path(), &declared()).unwrap_err();
        assert!(
            err.contains("schema_mismatch") && err.contains("column id"),
            "{err}"
        );
    }

    #[test]
    fn an_unknown_column_is_refused() {
        let td = td_with_seed(
            r#"{"schema":{"id":"int","name":"text","audience":"text","extra":"text"}}
"#,
        );
        let err = check_declared_seed_files(td.path(), &declared()).unwrap_err();
        assert!(
            err.contains("schema_mismatch") && err.contains("column extra"),
            "{err}"
        );
    }

    #[test]
    fn a_newer_header_version_is_refused_and_an_absent_one_is_version_one() {
        let newer = td_with_seed(
            r#"{"schema":{"id":"int","name":"text","audience":"text"},"version":2}
"#,
        );
        let err = check_declared_seed_files(newer.path(), &declared()).unwrap_err();
        assert!(
            err.contains("schema_mismatch") && err.contains("version 2"),
            "{err}"
        );
        let same = td_with_seed(
            r#"{"schema":{"id":"int","name":"text","audience":"text"},"version":1}
"#,
        );
        check_declared_seed_files(same.path(), &declared()).unwrap();
    }

    /// Data VALUES are not type-checked — "statically parseable" is the bar.
    #[test]
    fn check_seed_files_ignores_value_types() {
        let td = td_with_seed(
            r#"{"schema":{"id":"int","name":"text"}}
{"id":"not-an-int","name":42}
"#,
        );
        check_seed_files(td.path(), &items_schema()).unwrap();
    }
}
