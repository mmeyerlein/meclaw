//! Seed-Loader for Phase-9 store. Reads `<cell_dir>/seed/<table>.jsonl`
//! (one file per table) and inserts rows. Called by the factory after
//! `apply_schema_ddl` ONLY when the cell.db was freshly created
//! (`OpenStatus::Created` — see brainstorm E4).
//!
//! Format (per overview Z.1193–1213):
//!   Line 1: `{"schema": {"<col>": "<type>", ...}}`  (Cross-Check)
//!   Lines 2+: data rows as JSON-objects keyed by column name.

use crate::store::ops::json_to_sql_value;
use meclaw_core::serde_json::Value;
use std::collections::BTreeMap;

/// Load seed JSONL for every table declared in `schema`. Missing seed
/// file is OK (silent skip). Per-line: schema-line cross-check, then
/// data-row inserts using rusqlite parameter binding.
///
/// Called by `StoreCellFactory::spawn_cell` ONLY when
/// `OpenStatus::Created` (brainstorm E4 — fresh-only, never on resume).
pub fn load_seed_if_present(
    conn: &rusqlite::Connection,
    cell_dir: &std::path::Path,
    schema: &BTreeMap<String, BTreeMap<String, String>>,
) -> Result<(), String> {
    for (table, cols) in schema {
        let path = cell_dir.join("seed").join(format!("{table}.jsonl"));
        if !path.exists() {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("seed read {}: {e}", path.display()))?;
        let mut lines = text.lines();
        let header_line = lines
            .next()
            .ok_or_else(|| format!("seed {}: empty file", path.display()))?;
        let header: Value = meclaw_core::serde_json::from_str(header_line)
            .map_err(|e| format!("seed {} line 1: {e}", path.display()))?;
        let header_schema = header
            .get("schema")
            .and_then(|v| v.as_object())
            .ok_or_else(|| format!("seed {} line 1: missing schema object", path.display()))?;
        for col in cols.keys() {
            if !header_schema.contains_key(col) {
                return Err(format!(
                    "seed {}: schema mismatch — column {col} missing in seed header",
                    path.display()
                ));
            }
        }
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
            let col_names: Vec<&String> = cols.keys().collect();
            let placeholders = col_names.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let col_list = col_names
                .iter()
                .map(|c| format!("\"{c}\""))
                .collect::<Vec<_>>()
                .join(",");
            let stmt = format!("INSERT INTO \"{table}\" ({col_list}) VALUES ({placeholders})");
            let params: Vec<rusqlite::types::Value> = col_names
                .iter()
                .map(|c| json_to_sql_value(row_obj.get(c.as_str())))
                .collect();
            let bind: Vec<&dyn rusqlite::ToSql> =
                params.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
            conn.execute(&stmt, bind.as_slice()).map_err(|e| {
                format!(
                    "seed {} line {}: insert failed: {e}",
                    path.display(),
                    idx + 2
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
}
