//! Seed-Loader for Phase-9 store. Reads `<cell_dir>/seed/<table>.jsonl`
//! (one file per table) and inserts rows. Called by the factory after
//! `apply_schema_ddl` ONLY when the cell.db was freshly created
//! (`OpenStatus::Created` — see brainstorm E4).
//!
//! Format (per overview Z.1193–1213):
//!   Line 1: `{"schema": {"<col>": "<type>", ...}}`  (Cross-Check)
//!   Lines 2+: data rows as JSON-objects keyed by column name.

use crate::store::ops::json_to_sql_value;
use meclaw_core::serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Filesystem location of the seed file of one declared table.
fn seed_path(cell_dir: &std::path::Path, table: &str) -> std::path::PathBuf {
    cell_dir.join("seed").join(format!("{table}.jsonl"))
}

/// Parse ONE seed file into its data rows. Header line 1 must be
/// `{"schema": {…}}` and must cover every column declared for the table;
/// every further non-empty line must be a JSON object.
///
/// Pure parse — no database, no side effects. Issue #56: this is the single
/// parse path shared by the static check ([`check_seed_files`], run in the
/// `--validate` / bootstrap-plan phase and at factory spawn) and the loader
/// ([`load_seed_if_present`], run at the first wake). Keeping them on one path
/// is what makes the validate-equals-spawn invariant hold: a seed file that
/// survives validation always parses at wake.
fn parse_seed_file(
    path: &std::path::Path,
    cols: &BTreeMap<String, String>,
) -> Result<Vec<Map<String, Value>>, String> {
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
    let header_schema = header
        .get("schema")
        .and_then(|v| v.as_object())
        .ok_or_else(|| {
            format!(
                "seed {} line 1: missing schema object — line 1 must be the header \
                 {{\"schema\":{{…}}}}, not a data row",
                path.display()
            )
        })?;
    for col in cols.keys() {
        if !header_schema.contains_key(col) {
            return Err(format!(
                "seed {}: schema mismatch — column {col} missing in seed header",
                path.display()
            ));
        }
    }
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
    Ok(rows)
}

/// Static, database-free parse check of every seed file below `cell_dir`.
///
/// Issue #56: a `seed/<table>.jsonl` is a statically parseable part of a cell's
/// configuration, so a syntactic mistake in it must surface where every other
/// static mistake surfaces — during `--validate` and at factory spawn — not as
/// a panic on the store cell's first wake.
///
/// Checked per declared table (a missing seed file stays legal): the header
/// line is present and is a `{"schema": {…}}` object, it covers every declared
/// column, and every data line is a JSON object. Data VALUES are deliberately
/// not type-checked — "statically parseable" is the bar here.
pub fn check_seed_files(
    cell_dir: &std::path::Path,
    schema: &BTreeMap<String, BTreeMap<String, String>>,
) -> Result<(), String> {
    for (table, cols) in schema {
        let path = seed_path(cell_dir, table);
        if !path.exists() {
            continue;
        }
        parse_seed_file(&path, cols)?;
    }
    Ok(())
}

/// Load seed JSONL for every table declared in `schema`. Missing seed
/// file is OK (silent skip). Per-file: full parse first (schema-line
/// cross-check + data rows, see [`parse_seed_file`]), then the inserts using
/// rusqlite parameter binding — so a parse error never leaves a half-seeded
/// table behind.
///
/// Called by `StoreCellFactory`'s `WakeFn` ONLY when `OpenStatus::Created`
/// (brainstorm E4 — fresh-only, never on resume).
pub fn load_seed_if_present(
    conn: &rusqlite::Connection,
    cell_dir: &std::path::Path,
    schema: &BTreeMap<String, BTreeMap<String, String>>,
) -> Result<(), String> {
    for (table, cols) in schema {
        let path = seed_path(cell_dir, table);
        if !path.exists() {
            continue;
        }
        let rows = parse_seed_file(&path, cols)?;
        let col_names: Vec<&String> = cols.keys().collect();
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
                .map(|c| json_to_sql_value(row_obj.get(c.as_str())))
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
