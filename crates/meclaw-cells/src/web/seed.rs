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
use meclaw_colony::schema_evolution::{self, ColumnDecl, ResolvePlan, TableDecl};
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
    // GH #822: resolved against the fixed schema by the one resolver every
    // seed reader shares — a column the header lacks takes the default the
    // DDL declares for it (`web::db`), every other difference is refused.
    let writer = schema_evolution::parse_writer_header(table, header_obj)
        .map_err(|e| format!("seed {}: {e}", path.display()))?;
    let plan = schema_evolution::resolve(table, &writer, &table_decl(table))
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
        if table == "components" {
            check_component_row(row_obj)
                .map_err(|e| format!("seed {} line {}: {e}", path.display(), idx + 2))?;
        }
        rows.push(row_obj.clone());
    }
    Ok((rows, plan))
}

/// The fixed schema of one `web` table as the resolver reads it (GH #822):
/// types and the defaults of `web::db::setup_web_schema`'s `NOT NULL DEFAULT`
/// columns, so a seed written before such a column existed loads with the
/// value the DDL itself would give it. Kept beside the loader; a drift lock
/// (`the_declared_defaults_match_the_ddl`) pins it against the DDL.
fn table_decl(table: &str) -> TableDecl {
    let col = |ty: &str, default: Option<Value>| ColumnDecl {
        ty: ty.to_string(),
        default,
    };
    let text = |d: &str| Some(Value::from(d));
    let columns: Vec<(&str, ColumnDecl)> = match table {
        "objects" => vec![
            ("id", col("text", None)),
            ("parent", col("text", None)),
            ("component", col("text", None)),
            ("ord", col("int", Some(Value::from(0)))),
            ("props", col("json", text("{}"))),
        ],
        "components" => vec![
            ("name", col("text", None)),
            ("template", col("text", None)),
            ("prop_schema", col("json", text("{}"))),
            ("editable", col("json", text("[]"))),
            ("layer", col("text", text("content"))),
        ],
        "pages" => vec![
            ("route", col("text", None)),
            ("root", col("text", None)),
            ("title", col("text", text(""))),
        ],
        // `body` is a BLOB column the loader binds as TEXT; a header may say
        // `blob` or `text` (one storage class, `schema_evolution::HEADER_TYPES`).
        "assets" => vec![
            ("path", col("text", None)),
            ("content_type", col("text", None)),
            ("body", col("text", None)),
        ],
        _ => vec![],
    };
    TableDecl {
        version: 1,
        columns: columns
            .into_iter()
            .map(|(c, d)| (c.to_string(), d))
            .collect(),
    }
}

/// The semantic checks one seeded `components` row has to pass.
///
/// The material rule, and since GH #869 the definition rules on markup and
/// `editable` ([`crate::web::ops::check_definition`]) — the same function
/// `component.define` calls — the template's syntax included since review I1
/// of #868/#869, because the markup rules hold only for a template the parser
/// accepts. A `layer` value outside the vocabulary is
/// treated here as what it is — not navigation, therefore not allowed to wear
/// glass.
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
    crate::web::ops::check_glass_layer(name, template, layer)?;
    crate::web::ops::check_definition(
        name,
        template,
        &json_column(row.get("prop_schema")),
        &json_column(row.get("editable")),
    )
}

/// A JSON column of a seed row, as the structure it holds.
///
/// A seed may write `prop_schema` and `editable` either as the structure or
/// as its JSON text (see [`json_to_sql`]); both have to reach the same check.
/// Text that does not parse reads as `null`, which no rule refuses — the load
/// then stores it as it always did.
fn json_column(v: Option<&Value>) -> Value {
    match v {
        Some(Value::String(text)) => meclaw_core::serde_json::from_str(text).unwrap_or(Value::Null),
        Some(other) => other.clone(),
        None => Value::Null,
    }
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
        parse_seed_file(&path, table)?;
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
        let (rows, plan) = parse_seed_file(&path, table)?;
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
                .map(|c| json_to_sql(plan.fill.get(*c).or(row.get(*c)).unwrap_or(&Value::Null)))
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

    // ---- GH #822: the header is resolved against the fixed schema. ----

    #[test]
    fn a_missing_column_with_a_default_is_filled_at_birth() {
        // A components seed from before `layer` existed.
        let td = dir_with(&[(
            "components.jsonl",
            concat!(
                r#"{"schema":{"name":"text","template":"text","prop_schema":"json","editable":"json"}}"#,
                "\n",
                r#"{"name":"a","template":"<p>x</p>","prop_schema":{},"editable":[]}"#,
                "\n"
            ),
        )]);
        check_seed_files(td.path()).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        crate::web::db::setup_web_schema(&conn).unwrap();
        load_seed_if_present(&conn, td.path()).unwrap();
        let layer: String = conn
            .query_row("SELECT layer FROM components WHERE name = 'a'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(layer, "content");
    }

    #[test]
    fn a_missing_column_without_default_is_refused_at_birth_naming_it() {
        let td = dir_with(&[(
            "pages.jsonl",
            "{\"schema\":{\"route\":\"text\",\"title\":\"text\"}}\n",
        )]);
        let err = check_seed_files(td.path()).unwrap_err();
        assert!(
            err.contains("schema_mismatch") && err.contains("column root"),
            "{err}"
        );
    }

    #[test]
    fn a_changed_type_is_refused() {
        let td = dir_with(&[(
            "pages.jsonl",
            "{\"schema\":{\"route\":\"text\",\"root\":\"int\",\"title\":\"text\"}}\n",
        )]);
        let err = check_seed_files(td.path()).unwrap_err();
        assert!(
            err.contains("schema_mismatch") && err.contains("column root"),
            "{err}"
        );
    }

    #[test]
    fn an_unknown_column_is_refused() {
        let td = dir_with(&[(
            "pages.jsonl",
            "{\"schema\":{\"route\":\"text\",\"root\":\"text\",\"title\":\"text\",\"x\":\"text\"}}\n",
        )]);
        let err = check_seed_files(td.path()).unwrap_err();
        assert!(
            err.contains("schema_mismatch") && err.contains("column x"),
            "{err}"
        );
    }

    /// The resolver's view of the fixed schema names exactly the DDL's columns,
    /// and every declared default is the DDL's own `DEFAULT`.
    #[test]
    fn the_declared_defaults_match_the_ddl() {
        let conn = Connection::open_in_memory().unwrap();
        crate::web::db::setup_web_schema(&conn).unwrap();
        for table in TABLES {
            let decl = table_decl(table);
            let mut st = conn
                .prepare("SELECT name, dflt_value FROM pragma_table_info(?1)")
                .unwrap();
            let ddl: Vec<(String, Option<String>)> = st
                .query_map([table], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            let names: Vec<&str> = decl.columns.keys().map(String::as_str).collect();
            let mut ddl_names: Vec<&str> = ddl.iter().map(|(n, _)| n.as_str()).collect();
            ddl_names.sort();
            assert_eq!(names, ddl_names, "{table}");
            assert_eq!(columns_of(table).unwrap().len(), names.len(), "{table}");
            for (name, dflt) in &ddl {
                let lit = schema_evolution::sql_default_literal(&decl.columns[name]);
                assert_eq!(lit.as_deref(), dflt.as_deref(), "{table}.{name}");
            }
        }
    }

    #[test]
    fn a_file_named_after_no_table_is_refused() {
        let td = dir_with(&[("widgets.jsonl", "{\"schema\":{\"a\":\"text\"}}\n")]);
        let err = check_seed_files(td.path()).unwrap_err();
        assert!(err.contains("widgets"), "the typo must be named: {err}");
    }

    /// GH #869: a seeded component meets the rules `component.define` does,
    /// with `prop_schema`/`editable` written as text or as structure.
    #[test]
    fn a_seeded_component_meets_the_definition_rules() {
        let header = r#"{"schema":{"name":"text","template":"text","prop_schema":"text","editable":"text","layer":"text"}}"#;
        for (row, needle) in [
            (
                r#"{"name":"a","template":"<a onclick=\"x()\">","prop_schema":"{}","editable":"[]","layer":"content"}"#,
                "event-handler",
            ),
            (
                r#"{"name":"b","template":"<a href=\"javascript:x()\">","prop_schema":{},"editable":[],"layer":"content"}"#,
                "javascript",
            ),
            (
                r#"{"name":"c","template":"<p>{{&b}}</p>","prop_schema":"{\"b\":\"html\"}","editable":"[\"b\"]","layer":"content"}"#,
                "editable",
            ),
            (
                r#"{"name":"d","template":"<p>{{&b}}</p>","prop_schema":{"b":"html"},"editable":["b"],"layer":"content"}"#,
                "editable",
            ),
            (
                r#"{"name":"f","template":"<scr{{x}}ipt>x</scr{{x}}ipt>","prop_schema":{},"editable":[],"layer":"content"}"#,
                "stands",
            ),
        ] {
            let body = format!("{header}\n{row}\n");
            let td = dir_with(&[("components.jsonl", body.as_str())]);
            let err = check_seed_files(td.path()).unwrap_err();
            assert!(err.contains(needle), "{row}: {err}");
        }
        let body = format!(
            "{header}\n{}\n",
            r#"{"name":"e","template":"<b data-tone=\"{{t}}\" phx-click=\"{{t}}\">{{t}}</b>","prop_schema":"{\"t\":\"text\"}","editable":"[\"t\"]","layer":"content"}"#
        );
        let td = dir_with(&[("components.jsonl", body.as_str())]);
        check_seed_files(td.path()).unwrap();
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
