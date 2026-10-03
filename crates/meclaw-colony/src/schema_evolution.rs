//! GH #822 — additive schema evolution by declaration: one resolver between the
//! schema a row was WRITTEN with and the schema a cell DECLARES.
//!
//! ## Why
//!
//! Every way a row enters a `cell.db` from outside — a seed at birth (the
//! `store`, `llm` and `web` loaders), the mutation staging seeder, and an
//! `import` into a running cell — carries a header naming the columns it was
//! written with. Until GH #822 each of the five readers compared that header
//! with its own table its own way, and none of them could say what an absent
//! column should hold. Measured on the owner's case: an `affinity.proposals`
//! export from before `affinity@3.4.0` has no `audience` column, the store
//! refused it at spawn ("column audience missing in seed header"), and the only
//! way to move such a member was a hand edit of the export.
//!
//! The owner ruled (23.09.) additive evolution per declaration, Avro-style:
//! the reader declares a column's **default**, and a writer that predates the
//! column is resolved against it. Everything else stays a refusal that names
//! the column — at birth, before a migration switches over, never at the
//! second boot.
//!
//! ## The rules ([`resolve`])
//!
//! | writer header | declared | outcome |
//! |---|---|---|
//! | column present, same storage class | column | taken as written |
//! | column absent | column with `default` | filled with the default |
//! | column absent | column without default | `schema_mismatch` naming it |
//! | column present, other storage class | column | `schema_mismatch` (changed type) |
//! | column present | not declared | `schema_mismatch` (unknown column) |
//! | `version` newer than declared | — | `schema_mismatch` (newer writer) |
//!
//! "Storage class", not type word: an export reads its types back from SQLite,
//! where `json` and `text` are both `TEXT` — so a header can only promise
//! `int` or "not int". Comparing type words would refuse every export of a
//! table with a `json` column on the way back in.
//!
//! A header without `version` is version 1; a declaration without one is 1 too.
//!
//! ## Where it lives
//!
//! Here, in `meclaw-colony`, because the five readers sit in two crates and
//! the dependency runs `meclaw-cells` → `meclaw-colony`: this is the one place
//! all of them reach without a new edge (AGENTS.md rule 6).

use meclaw_core::serde_json::{Map, Value};
use std::collections::BTreeMap;

/// The column type words a declaration may use (`params.schema`).
pub const DECLARED_TYPES: &[&str] = &["text", "int", "json"];

/// The column type words a writer header may carry. `blob` is the `web` cell's
/// asset body: its loader binds every non-integer value as TEXT, so the word is
/// a label of the same storage class as `text` (pinned in `web_cell_assets.rs`).
pub const HEADER_TYPES: &[&str] = &["text", "int", "json", "blob"];

/// One declared column: its type word and, optionally, the value a row written
/// before the column existed takes.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnDecl {
    /// `text`, `int` or `json`.
    pub ty: String,
    /// `None` = no default (a writer without the column is refused);
    /// `Some(Value::Null)` = the declared default is SQL `NULL`.
    pub default: Option<Value>,
}

/// One declared table: its version and its columns.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TableDecl {
    /// Declared schema version of the table (`params.schema_versions`), 1 when
    /// nothing is declared.
    pub version: u32,
    /// Column name → declaration.
    pub columns: BTreeMap<String, ColumnDecl>,
}

/// The schema a document or seed file was written with (its header).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WriterSchema {
    /// The header's `version`, 1 when absent.
    pub version: u32,
    /// Column name → type word, as the header states it.
    pub columns: BTreeMap<String, String>,
}

/// What a reader has to do beyond taking the written columns as they are.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResolvePlan {
    /// Declared columns the writer did not know, with the default each row
    /// takes. Empty when writer and declaration agree.
    pub fill: BTreeMap<String, Value>,
}

/// The refusal: `schema_mismatch {table, column, reason}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaMismatch {
    /// The table the header was resolved for.
    pub table: String,
    /// The offending column — empty only for a version refusal, which concerns
    /// the table as a whole.
    pub column: String,
    /// Why, in words an operator can act on.
    pub reason: String,
}

impl std::fmt::Display for SchemaMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.column.is_empty() {
            write!(f, "schema_mismatch: table {}: {}", self.table, self.reason)
        } else {
            write!(
                f,
                "schema_mismatch: table {} column {}: {}",
                self.table, self.column, self.reason
            )
        }
    }
}

/// The storage class a type word lands in: `int` is INTEGER, every other word
/// is TEXT (see the module docs).
pub fn storage_class(ty: &str) -> &'static str {
    if ty == "int" { "int" } else { "text" }
}

/// Parse a schema version: a positive integer.
pub fn parse_version(what: &str, v: &Value) -> Result<u32, String> {
    v.as_u64()
        .filter(|n| *n >= 1 && *n <= u32::MAX as u64)
        .map(|n| n as u32)
        .ok_or_else(|| format!("{what} must be a positive integer, got {v}"))
}

/// Parse ONE declared column: the short form `"text"` or the object form
/// `{"type": "text", "default": ""}` (GH #822, E.1). The short form is what
/// every declaration written before #822 says, and it keeps meaning a column
/// without a default.
///
/// A default must fit its type — an integer for `int`, a string for `text`,
/// any JSON value for `json` — or be `null`. A default that does not fit is a
/// configuration error here rather than a value the database coerces later.
pub fn parse_column(what: &str, v: &Value) -> Result<ColumnDecl, String> {
    let (ty, default) = match v {
        Value::String(s) => (s.as_str(), None),
        Value::Object(o) => {
            for k in o.keys() {
                if k != "type" && k != "default" {
                    return Err(format!(
                        "{what}: unknown key {k:?} in a column declaration (allowed: type, default)"
                    ));
                }
            }
            let ty = o
                .get("type")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{what}.type must be a string"))?;
            (ty, o.get("default").cloned())
        }
        _ => {
            return Err(format!(
                "{what} must be a type string or an object {{\"type\", \"default\"}}"
            ));
        }
    };
    if !DECLARED_TYPES.contains(&ty) {
        return Err(format!(
            "{what}: unsupported type {ty:?} (allowed: text/int/json)"
        ));
    }
    if let Some(d) = &default {
        let fits = match (ty, d) {
            (_, Value::Null) => true,
            ("int", Value::Number(n)) => n.as_i64().is_some(),
            ("text", Value::String(_)) => true,
            ("json", _) => true,
            _ => false,
        };
        if !fits {
            return Err(format!(
                "{what}: default {d} does not fit type {ty:?} (int: an integer, text: a \
                 string, json: any value; null always)"
            ));
        }
    }
    Ok(ColumnDecl {
        ty: ty.to_string(),
        default,
    })
}

/// Parse the writer schema out of a header object `{"schema": {…}, "version"?: n}`.
///
/// A header column is a type word or an object carrying `type` (a header may
/// be copied straight out of a declaration). An unknown type word is refused
/// here, naming the column: a header is the writer's promise about its rows.
pub fn parse_writer_header(
    table: &str,
    header: &Map<String, Value>,
) -> Result<WriterSchema, String> {
    let schema = header
        .get("schema")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("{table}: the header carries no schema object"))?;
    let version = match header.get("version") {
        None => 1,
        Some(v) => parse_version(&format!("{table}: header version"), v)?,
    };
    let mut columns = BTreeMap::new();
    for (col, v) in schema {
        let ty = match v {
            Value::String(s) => s.as_str(),
            Value::Object(o) => o.get("type").and_then(Value::as_str).unwrap_or(""),
            _ => "",
        };
        if !HEADER_TYPES.contains(&ty) {
            return Err(SchemaMismatch {
                table: table.to_string(),
                column: col.clone(),
                reason: format!("the header declares type {v} (allowed: text/int/json)"),
            }
            .to_string());
        }
        columns.insert(col.clone(), ty.to_string());
    }
    Ok(WriterSchema { version, columns })
}

/// THE resolver (GH #822, E.3): the writer schema against the declaration.
///
/// Deterministic and pure; the checks run in a fixed order (version, unknown
/// columns, changed types, missing columns), each in column-name order, so the
/// same pair always names the same column.
pub fn resolve(
    table: &str,
    writer: &WriterSchema,
    declared: &TableDecl,
) -> Result<ResolvePlan, SchemaMismatch> {
    let refuse = |column: &str, reason: String| SchemaMismatch {
        table: table.to_string(),
        column: column.to_string(),
        reason,
    };
    let declared_version = declared.version.max(1);
    if writer.version > declared_version {
        return Err(refuse(
            "",
            format!(
                "the writer schema is version {} and this cell declares version {} — the \
                 source is newer; growing a schema is a template change",
                writer.version, declared_version
            ),
        ));
    }
    for col in writer.columns.keys() {
        if !declared.columns.contains_key(col) {
            return Err(refuse(
                col,
                "unknown column — the writer carries a column this cell does not declare".into(),
            ));
        }
    }
    for (col, ty) in &writer.columns {
        let decl = &declared.columns[col];
        if storage_class(ty) != storage_class(&decl.ty) {
            return Err(refuse(
                col,
                format!("changed type — written as {ty}, declared as {}", decl.ty),
            ));
        }
    }
    let mut plan = ResolvePlan::default();
    for (col, decl) in &declared.columns {
        if writer.columns.contains_key(col) {
            continue;
        }
        match &decl.default {
            Some(d) => {
                plan.fill.insert(col.clone(), d.clone());
            }
            None => {
                return Err(refuse(
                    col,
                    "missing in the writer schema and declared without a default".into(),
                ));
            }
        }
    }
    Ok(plan)
}

/// The SQL literal of a declared default, for `CREATE TABLE … DEFAULT` and
/// `ALTER TABLE … ADD COLUMN … DEFAULT` (GH #822 + N-A F review M-2: a column
/// added to a table that already holds rows must give those rows the default,
/// not `NULL` — `tries < 3` is false for `NULL`, and such a row was never
/// pulled again).
///
/// `None` when the column declares no default. Strings are single-quoted with
/// `'` doubled; a `json` default that is not a string is stored as its JSON
/// text, the same text an insert of that value writes.
pub fn sql_default_literal(decl: &ColumnDecl) -> Option<String> {
    let quote = |s: &str| format!("'{}'", s.replace('\'', "''"));
    decl.default.as_ref().map(|d| match d {
        Value::Null => "NULL".to_string(),
        Value::Number(n) if decl.ty == "int" => n.as_i64().unwrap_or_default().to_string(),
        Value::String(s) => quote(s),
        other => quote(&other.to_string()),
    })
}

/// A declared table in the short form only (no defaults, version 1) — what a
/// reader whose declaration is a plain `{column: type}` map hands the resolver.
pub fn table_decl_of_types(types: &BTreeMap<String, String>) -> TableDecl {
    TableDecl {
        version: 1,
        columns: types
            .iter()
            .map(|(c, t)| {
                (
                    c.clone(),
                    ColumnDecl {
                        ty: t.clone(),
                        default: None,
                    },
                )
            })
            .collect(),
    }
}

/// The `meta` key under which a cell records the declared version of one of
/// its tables. `meta` is substrate bookkeeping and never travels as a row
/// (`db_transfer::SUBSTRATE_TABLES`); the version travels in the export header.
pub fn version_meta_key(table: &str) -> String {
    format!("table_version:{table}")
}

/// The declared version of `table` as recorded in the cell's `meta` table,
/// 1 when nothing is recorded (or the database has no `meta` at all).
pub fn recorded_version(conn: &rusqlite::Connection, table: &str) -> u32 {
    conn.query_row(
        "SELECT value FROM meta WHERE key = ?1",
        [version_meta_key(table)],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .and_then(|s| s.parse::<u32>().ok())
    .filter(|v| *v >= 1)
    .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    fn decl(v: Value) -> TableDecl {
        let mut t = TableDecl {
            version: 1,
            columns: BTreeMap::new(),
        };
        for (c, cv) in v.as_object().unwrap() {
            t.columns.insert(c.clone(), parse_column(c, cv).unwrap());
        }
        t
    }

    fn writer(v: Value) -> WriterSchema {
        parse_writer_header("t", v.as_object().unwrap()).unwrap()
    }

    /// One row of the resolver table: case name, writer header, expected outcome.
    type Case<'a> = (&'a str, Value, Result<Vec<&'a str>, &'a str>);

    /// The shared test vector of GH #822: one row per rule of the module table.
    /// `Ok(fill)` lists the filled columns, `Err(column)` the named one.
    #[test]
    fn the_resolver_table() {
        let declared = || {
            decl(json!({
                "id": "text",
                "audience": {"type": "text", "default": ""},
                "n": {"type": "int", "default": 0},
                "doc": "json"
            }))
        };
        let cases: Vec<Case<'_>> = vec![
            (
                "same columns",
                json!({"schema": {"id": "text", "audience": "text", "n": "int", "doc": "json"}}),
                Ok(vec![]),
            ),
            (
                "missing with default is filled",
                json!({"schema": {"id": "text", "doc": "json"}}),
                Ok(vec!["audience", "n"]),
            ),
            (
                "missing without default is refused",
                json!({"schema": {"audience": "text", "n": "int", "doc": "json"}}),
                Err("id"),
            ),
            (
                "changed type is refused",
                json!({"schema": {"id": "int", "audience": "text", "n": "int", "doc": "json"}}),
                Err("id"),
            ),
            (
                "json and text are one storage class",
                json!({"schema": {"id": "text", "audience": "text", "n": "int", "doc": "text"}}),
                Ok(vec![]),
            ),
            (
                "unknown column is refused",
                json!({"schema": {"id": "text", "audience": "text", "n": "int", "doc": "json",
                                  "extra": "text"}}),
                Err("extra"),
            ),
            (
                "a newer writer is refused",
                json!({"schema": {"id": "text", "audience": "text", "n": "int", "doc": "json"},
                       "version": 2}),
                Err(""),
            ),
        ];
        for (name, header, want) in cases {
            let got = resolve("t", &writer(header), &declared());
            match (got, want) {
                (Ok(plan), Ok(cols)) => {
                    let filled: Vec<&str> = plan.fill.keys().map(String::as_str).collect();
                    assert_eq!(filled, cols, "{name}");
                }
                (Err(e), Err(col)) => assert_eq!(e.column, col, "{name}: {e}"),
                (got, want) => panic!("{name}: got {got:?}, want {want:?}"),
            }
        }
    }

    #[test]
    fn a_header_without_version_is_version_one() {
        assert_eq!(writer(json!({"schema": {"id": "text"}})).version, 1);
        assert_eq!(
            writer(json!({"schema": {"id": "text"}, "version": 3})).version,
            3
        );
        assert!(
            parse_writer_header(
                "t",
                json!({"schema": {}, "version": 0}).as_object().unwrap()
            )
            .is_err()
        );
    }

    #[test]
    fn the_short_column_form_still_parses() {
        let c = parse_column("t.c", &json!("int")).unwrap();
        assert_eq!(
            c,
            ColumnDecl {
                ty: "int".into(),
                default: None
            }
        );
        let c = parse_column("t.c", &json!({"type": "text", "default": "x"})).unwrap();
        assert_eq!(c.default, Some(json!("x")));
        assert!(parse_column("t.c", &json!({"type": "int", "default": "x"})).is_err());
        assert!(parse_column("t.c", &json!({"type": "text", "dflt": ""})).is_err());
        assert!(parse_column("t.c", &json!("real")).is_err());
        assert!(parse_column("t.c", &json!(1)).is_err());
    }

    #[test]
    fn the_mismatch_names_table_and_column() {
        let e = resolve(
            "proposals",
            &writer(json!({"schema": {}})),
            &decl(json!({"audience": "text"})),
        )
        .unwrap_err();
        let s = e.to_string();
        assert!(s.starts_with("schema_mismatch"), "{s}");
        assert!(s.contains("proposals") && s.contains("audience"), "{s}");
    }

    #[test]
    fn a_default_renders_as_a_sql_literal() {
        let lit = |v: Value| sql_default_literal(&parse_column("c", &v).unwrap());
        assert_eq!(
            lit(json!({"type": "int", "default": 0})).as_deref(),
            Some("0")
        );
        assert_eq!(
            lit(json!({"type": "text", "default": "it's"})).as_deref(),
            Some("'it''s'")
        );
        assert_eq!(
            lit(json!({"type": "json", "default": []})).as_deref(),
            Some("'[]'")
        );
        assert_eq!(
            lit(json!({"type": "text", "default": null})).as_deref(),
            Some("NULL")
        );
        assert_eq!(lit(json!("text")), None);
    }
}
