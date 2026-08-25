//! W8 (GH #380): seeding a `web` cell from `seed/<table>.jsonl`.
//!
//! Same convention as the store cell, deliberately: one file per table, line 1
//! is the header `{"schema": {…}}` and must cover every column, the remaining
//! lines are data rows, a missing file is a silent skip. Somebody who has read
//! one seed directory in this repo has read them all.
//!
//! Two things differ, and both follow from the schema being fixed
//! ([`crate::web::db`]):
//!
//! 1. The set of legal file names is closed, so a `seed/widgets.jsonl` is a
//!    **typo to report** rather than a table to create. The store cannot make
//!    that check — any name might be a declared table there.
//! 2. The header is checked against the real columns rather than against a
//!    declaration, which means a seed written for an older schema fails loudly
//!    instead of inserting into columns that moved.
//!
//! The load runs **only** on `OpenStatus::Created` (the store's `seed.rs`
//! lesson): a display that re-seeded on every wake would resurrect objects an
//! operator had deleted.

use crate::web::db::{TABLES, columns_of};
use meclaw_core::serde_json::{Map, Value};
use rusqlite::Connection;

/// Filesystem location of one table's seed file.
fn seed_path(cell_dir: &std::path::Path, table: &str) -> std::path::PathBuf {
    cell_dir.join("seed").join(format!("{table}.jsonl"))
}

/// Parse ONE seed file into its data rows.
///
/// Pure parse — no database, no side effects. This is the single parse path
/// shared by the static check ([`check_seed_files`]) and the loader
/// ([`load_seed_if_present`]), which is what makes validate-equals-spawn hold:
/// a seed file that survives validation always parses at spawn.
///
/// It is also where the **material rule** meets seeded components (W8 Task 12,
/// GH #382). A seed row never passes through `component.define`, so a rule
/// enforced only there would be a rule every shipped template — the one place a
/// designed component set actually comes from — walks straight past. The check
/// itself lives once, in [`crate::web::ops::check_glass_layer`]; threading it
/// through this function rather than through the two callers is what keeps
/// validate and spawn from drifting apart.
fn parse_seed_file(
    path: &std::path::Path,
    table: &str,
    cols: &[&str],
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
    for col in cols {
        if !header_schema.contains_key(*col) {
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
        if table == "components" {
            check_component_row(row_obj)
                .map_err(|e| format!("seed {} line {}: {e}", path.display(), idx + 2))?;
        }
        rows.push(row_obj.clone());
    }
    Ok(rows)
}

/// The semantic checks one seeded `components` row has to pass.
///
/// Today that is the material rule and nothing else: the template's own syntax
/// is pinned by the shipped-template suite, and a `layer` value outside the
/// vocabulary is treated here as what it is — not navigation, therefore not
/// allowed to wear glass.
fn check_component_row(row: &Map<String, Value>) -> Result<(), String> {
    let name = row.get("name").and_then(Value::as_str).unwrap_or_default();
    let template = row
        .get("template")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let layer = row
        .get("layer")
        .and_then(Value::as_str)
        .unwrap_or("content");
    crate::web::ops::check_glass_layer(name, template, layer)
}

/// Static, database-free check of every seed file in `cell_dir`.
///
/// Runs in the bootstrap plan phase (`--validate`) and again at spawn, so a
/// syntactic mistake surfaces where every other static mistake surfaces.
///
/// Also reports a seed file named after a table this cell type does not have.
/// The schema is fixed, so such a file could never load, and staying silent
/// about it would mean an operator watching for seeded content that will never
/// appear.
pub fn check_seed_files(cell_dir: &std::path::Path) -> Result<(), String> {
    let dir = cell_dir.join("seed");
    if !dir.exists() {
        return Ok(());
    }

    let entries =
        std::fs::read_dir(&dir).map_err(|e| format!("seed read {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("seed read {}: {e}", dir.display()))?;
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(stem) = name.strip_suffix(".jsonl") else {
            continue;
        };
        if !TABLES.contains(&stem) {
            return Err(format!(
                "seed {}: no table named {stem:?} in a web cell — the schema is fixed, \
                 so this file would never load. Tables: {}",
                entry.path().display(),
                TABLES.join(", ")
            ));
        }
    }

    for table in TABLES {
        let path = seed_path(cell_dir, table);
        if !path.exists() {
            continue;
        }
        parse_seed_file(
            &path,
            table,
            columns_of(table).expect("TABLES entry has columns"),
        )?;
    }
    Ok(())
}

/// Load every present seed file into a freshly created database.
///
/// Caller's duty: invoke this **only** on `OpenStatus::Created`.
pub fn load_seed_if_present(conn: &Connection, cell_dir: &std::path::Path) -> Result<(), String> {
    for table in TABLES {
        let path = seed_path(cell_dir, table);
        if !path.exists() {
            continue;
        }
        let cols = columns_of(table).expect("TABLES entry has columns");
        let rows = parse_seed_file(&path, table, cols)?;
        if rows.is_empty() {
            continue;
        }

        let placeholders = (1..=cols.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "INSERT INTO {table} ({}) VALUES ({placeholders})",
            cols.join(", ")
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("seed {table}: prepare failed: {e}"))?;

        for (idx, row) in rows.iter().enumerate() {
            let values: Vec<rusqlite::types::Value> = cols
                .iter()
                .map(|c| json_to_sql(row.get(*c).unwrap_or(&Value::Null)))
                .collect();
            let params: Vec<&dyn rusqlite::ToSql> =
                values.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
            stmt.execute(params.as_slice()).map_err(|e| {
                format!(
                    "seed {} row {}: insert failed: {e}",
                    path.display(),
                    idx + 2
                )
            })?;
        }
    }
    Ok(())
}

/// Map one JSON value onto a SQL value.
///
/// Objects and arrays become their JSON text: `props`, `prop_schema` and
/// `editable` are TEXT columns holding JSON, so a seed may write them either as
/// a string or as the structure itself — the second is what a person writing
/// the file by hand reaches for, and refusing it would be pedantry.
fn json_to_sql(v: &Value) -> rusqlite::types::Value {
    use rusqlite::types::Value as S;
    match v {
        Value::Null => S::Null,
        Value::Bool(b) => S::Integer(i64::from(*b)),
        Value::Number(n) => n
            .as_i64()
            .map(S::Integer)
            .or_else(|| n.as_f64().map(S::Real))
            .unwrap_or(S::Null),
        Value::String(s) => S::Text(s.clone()),
        other => S::Text(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let td = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(td.path().join("seed")).unwrap();
        for (name, body) in files {
            std::fs::write(td.path().join("seed").join(name), body).unwrap();
        }
        td
    }

    #[test]
    fn no_seed_directory_is_legal() {
        let td = tempfile::TempDir::new().unwrap();
        check_seed_files(td.path()).unwrap();
    }

    #[test]
    fn a_data_row_on_line_one_is_named_as_a_missing_header() {
        let td = dir_with(&[("pages.jsonl", "{\"route\":\"/\"}\n")]);
        let err = check_seed_files(td.path()).unwrap_err();
        assert!(err.contains("schema"), "{err}");
    }

    #[test]
    fn a_header_missing_a_column_is_refused_with_the_column_named() {
        let td = dir_with(&[("pages.jsonl", "{\"schema\":{\"route\":\"text\"}}\n")]);
        let err = check_seed_files(td.path()).unwrap_err();
        assert!(err.contains("root") || err.contains("title"), "{err}");
    }

    #[test]
    fn a_file_named_after_no_table_is_refused() {
        let td = dir_with(&[("widgets.jsonl", "{\"schema\":{\"a\":\"text\"}}\n")]);
        let err = check_seed_files(td.path()).unwrap_err();
        assert!(err.contains("widgets"), "the typo must be named: {err}");
    }

    #[test]
    fn a_structured_props_value_is_stored_as_its_json_text() {
        let conn = Connection::open_in_memory().unwrap();
        crate::web::db::setup_web_schema(&conn).unwrap();
        let td = dir_with(&[(
            "objects.jsonl",
            concat!(
                r#"{"schema":{"id":"text","parent":"text","component":"text","ord":"int","props":"text"}}"#,
                "\n",
                r#"{"id":"a","parent":null,"component":"text","ord":0,"props":{"body":"hi"}}"#,
                "\n"
            ),
        )]);
        load_seed_if_present(&conn, td.path()).unwrap();
        let props: String = conn
            .query_row("SELECT props FROM objects WHERE id='a'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(props, r#"{"body":"hi"}"#);
    }
}
