//! Synthesizes `CREATE TABLE IF NOT EXISTS` statements from the
//! validated 2-stage schema map. Sync (rusqlite); called by the
//! StoreCellFactory before `tokio::spawn(cell_task_stateful)`
//! (Phase-7.5 Ad-hoc-DDL-Pattern).

use std::collections::BTreeMap;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

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
