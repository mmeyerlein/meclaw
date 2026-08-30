//! GH #456 — `seed_rows`: store rows enter a RUNNING colony through the
//! mutation door, not past it.
//!
//! Seven diff operations change topology. What is *inside* a cell — the rows of
//! its store — the door did not know, and rows reached a colony on exactly two
//! paths: as a `seed/<table>.jsonl` read into a **fresh** `cell.db`, or at run
//! time as a store message. There is a class of rows that are not data but
//! permissions and keys — an `access` policy row, a grant, a firewall rule, an
//! affinity subscriber — and for those the run-time path has three holes: no
//! digest (nobody can check that what landed is what was ordered), no access
//! verdict *before* the write, and no `mutation_log` entry (the change is
//! invisible to the steward and to the operator).
//!
//! `seed_rows` closes them by making the rows an operation of the diff:
//!
//! ```json
//! { "seed_rows": [ { "target": "./store", "table": "policies", "rows": [ {…} ] } ] }
//! ```
//!
//! It is the SAME seed mechanic, not a second one. The rows are bound with the
//! same JSON→SQL mapping the staging seeder uses, the table is created from the
//! store's **declared** `params.schema` exactly as
//! `meclaw_cells::store::ddl::apply_schema_ddl` would create it, and a store-owned
//! table that ends up standing without its key is repaired by `ensure_keyed_table`
//! (GH #255) at the store's next wake — the reconciliation the staging seeder
//! already relies on.
//!
//! Two phases, because a refusal must not leave half a write behind:
//! [`parse_entries`] runs on the raw diff (pre-destructive, before a byte is
//! staged), [`resolve_entries`] answers "is this a store, does it declare that
//! table" against the post-apply registry, and only [`apply_entries`] writes.
//!
//! **Why the colony may write another cell's `cell.db` here.** The colony is the
//! write authority (§ Authority model), and it already builds and seeds a
//! `cell.db` at instantiation time (`mutation::stage::seed_cell_db_if_present`).
//! What made `--vault-add` against a live cell wrong (GH #160) was not the second
//! connection — WAL and `busy_timeout` serialise that — but the vault cell's
//! in-memory view going stale with nothing announcing it. A `store` keeps no such
//! view: every op reads the database it is asked about, so a row committed on a
//! second connection is visible to the very next query. Rows that a store
//! *derives* on write (a `canonical` target column) are backfilled by the store's
//! own DDL at its next spawn, exactly as they are for a seed file.

use crate::mutation::MutationError;
use meclaw_core::JsonValue;
use meclaw_core::serde_json::{Map, Value};
use std::collections::BTreeMap;

/// The diff key this module owns.
pub const KEY: &str = "seed_rows";

/// The cell type a `seed_rows` target must be.
const STORE_TYPE: &str = "store";

/// One `seed_rows[]` entry after the shape check, still unresolved.
#[derive(Debug, Clone)]
pub struct SeedRowsDecl {
    /// The target expression as declared, resolved against the mutation scope
    /// later (`./store`, `store`, `sub/store` — the `add_edges` spelling).
    pub target: String,
    /// The table the rows go into. A plain SQL identifier.
    pub table: String,
    /// The rows, as declared. Each is a JSON object keyed by column name.
    pub rows: Vec<Map<String, Value>>,
}

/// One `seed_rows[]` entry resolved against the registry and the target's
/// declared schema — everything [`apply_entries`] needs, and nothing it has to
/// look up while it writes.
#[derive(Debug, Clone)]
pub struct ResolvedSeedRows {
    /// The resolved logical path of the target store.
    pub target: meclaw_core::Path,
    /// The target's on-disk cell directory (where its `cell.db` lives).
    pub cell_dir: std::path::PathBuf,
    /// The table name, as declared by the store's `params.schema`.
    pub table: String,
    /// The declared column→type map of that table, used to create the table if
    /// the store has never been awake.
    pub columns: BTreeMap<String, String>,
    /// The rows, as declared.
    pub rows: Vec<Map<String, Value>>,
}

/// What one applied declaration did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedRowsApplied {
    /// The store the rows went into.
    pub target: String,
    /// The table they went into.
    pub table: String,
    /// Rows written by this apply.
    pub inserted: usize,
    /// Rows that were already present, byte for byte, and therefore not written
    /// a second time. This is what makes a re-applied manifest a no-op.
    pub already_present: usize,
}

/// A table name that may be formatted into DDL.
///
/// Column names come from the store's own `params.schema` (closed-set validated
/// when the store parses its params) and every row key is checked against that
/// set, so the table name is the only identifier a declaration contributes —
/// and it is checked here rather than trusted.
fn is_plain_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Shape check of `diff.seed_rows`, on the RAW diff and before anything is
/// staged. Returns the declarations in the order the diff lists them; an absent
/// key yields an empty vector.
///
/// Everything checkable without the registry is checked here, because a refusal
/// that costs nothing is worth more than one that arrives after a spawn.
pub fn parse_entries(diff: &JsonValue) -> Result<Vec<SeedRowsDecl>, MutationError> {
    let Some(raw) = diff.get(KEY) else {
        return Ok(Vec::new());
    };
    let entries = raw
        .as_array()
        .ok_or_else(|| MutationError::Schema(format!("{KEY} must be an array of declarations")))?;
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let obj = entry
            .as_object()
            .ok_or_else(|| MutationError::Schema(format!("{KEY}[] entry must be an object")))?;
        let target = obj
            .get("target")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                MutationError::Schema(format!(
                    "{KEY}[].target missing — a row has to name the store it belongs in"
                ))
            })?;
        let table = obj
            .get("table")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                MutationError::Schema(format!(
                    "{KEY}[].table missing — a row has to name the table it belongs in"
                ))
            })?;
        if !is_plain_identifier(table) {
            return Err(MutationError::Schema(format!(
                "{KEY}[].table {table:?} is not a plain table name (letters, digits and \
                 underscores, not starting with a digit)"
            )));
        }
        let rows = obj.get("rows").and_then(|v| v.as_array()).ok_or_else(|| {
            MutationError::Schema(format!("{KEY}[].rows missing or not an array"))
        })?;
        if rows.is_empty() {
            return Err(MutationError::Schema(format!(
                "{KEY}[] for {target}.{table} declares no rows. An operation that writes \
                 nothing and reports `committed` is the shape this door refuses everywhere \
                 else; it is refused here too."
            )));
        }
        let mut parsed_rows = Vec::with_capacity(rows.len());
        for (idx, row) in rows.iter().enumerate() {
            let row_obj = row.as_object().ok_or_else(|| {
                MutationError::Schema(format!(
                    "{KEY}[] {target}.{table} row {}: a row must be a JSON object keyed by \
                     column name",
                    idx + 1
                ))
            })?;
            if row_obj.is_empty() {
                return Err(MutationError::Schema(format!(
                    "{KEY}[] {target}.{table} row {}: a row must name at least one column",
                    idx + 1
                )));
            }
            parsed_rows.push(row_obj.clone());
        }
        out.push(SeedRowsDecl {
            target: target.to_string(),
            table: table.to_string(),
            rows: parsed_rows,
        });
    }
    Ok(out)
}

/// Read the `params.schema` of the store at `cell_dir`.
///
/// The colony writes these `config.json` files and is the authority over them
/// (§ Authority model), so reading one back is the same read the staging path
/// already does — not a database isolation break, which is about `cell.db`.
fn declared_schema(
    cell_dir: &std::path::Path,
) -> Result<BTreeMap<String, BTreeMap<String, String>>, String> {
    let path = cell_dir.join("config.json");
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let cfg: crate::config::ParsedConfig =
        meclaw_core::serde_json::from_str(&raw).map_err(|e| format!("{}: {e}", path.display()))?;
    let Some(schema) = cfg.params.get("schema").and_then(|v| v.as_object()) else {
        return Ok(BTreeMap::new());
    };
    let mut out = BTreeMap::new();
    for (table, cols) in schema {
        let Some(cols) = cols.as_object() else {
            continue;
        };
        let mut m = BTreeMap::new();
        for (col, ty) in cols {
            m.insert(col.clone(), ty.as_str().unwrap_or("text").to_string());
        }
        out.insert(table.clone(), m);
    }
    Ok(out)
}

/// Resolve every declaration against the registry as it stands AFTER the
/// topology of the same diff has been applied — so a store this very mutation
/// grew is a legal target — and against the target's declared schema.
///
/// Pure lookup and refusal: nothing is written here, which is what lets the
/// caller run it before the point of no return.
pub fn resolve_entries(
    decls: &[SeedRowsDecl],
    scope: &str,
    registry: &std::collections::HashMap<meclaw_core::Path, crate::colony::RegistryEntry>,
    root: &std::path::Path,
) -> Result<Vec<ResolvedSeedRows>, MutationError> {
    let mut out = Vec::with_capacity(decls.len());
    for decl in decls {
        let target = crate::mutation::resolve_scoped_path(scope, &decl.target);
        let Some(entry) = registry.get(&target) else {
            return Err(MutationError::SeedTargetNotAStore(format!(
                "{KEY} names {} and no cell is registered there. Rows go into a store that \
                 exists — the same diff may grow it, but something has to.",
                target.as_str()
            )));
        };
        if entry.cell_type != STORE_TYPE {
            return Err(MutationError::SeedTargetNotAStore(format!(
                "{KEY} names {}, which is a `{}` cell. Only a `{STORE_TYPE}` owns declared \
                 tables, so only a `{STORE_TYPE}` can take rows through this door.",
                target.as_str(),
                entry.cell_type
            )));
        }
        let cell_dir =
            crate::path_truth::resolve_cell_dir(root, "/", target.as_str().trim_start_matches('/'));
        let schema = declared_schema(&cell_dir).map_err(|e| {
            MutationError::SeedTargetNotAStore(format!(
                "{KEY} names {} but its declaration could not be read: {e}",
                target.as_str()
            ))
        })?;
        let Some(columns) = schema.get(&decl.table) else {
            let declared: Vec<&str> = schema.keys().map(String::as_str).collect();
            return Err(MutationError::SeedTableUndeclared(format!(
                "{} declares no table {:?}. The tables it declares are: {}. A row into an \
                 undeclared table would create a shape the store never agreed to.",
                target.as_str(),
                decl.table,
                if declared.is_empty() {
                    "none".to_string()
                } else {
                    declared.join(", ")
                }
            )));
        };
        for (idx, row) in decl.rows.iter().enumerate() {
            for col in row.keys() {
                if !columns.contains_key(col) {
                    let known: Vec<&str> = columns.keys().map(String::as_str).collect();
                    return Err(MutationError::Schema(format!(
                        "{KEY}[] {}.{} row {}: column {col:?} is not declared. The declared \
                         columns are: {}.",
                        target.as_str(),
                        decl.table,
                        idx + 1,
                        known.join(", ")
                    )));
                }
            }
        }
        out.push(ResolvedSeedRows {
            target: target.clone(),
            cell_dir,
            table: decl.table.clone(),
            columns: columns.clone(),
            rows: decl.rows.clone(),
        });
    }
    Ok(out)
}

/// The SQLite type a declared store column maps to — the same three-way mapping
/// `meclaw_cells::store::ddl::sqlite_type` and the staging seeder use.
fn sqlite_type(declared: &str) -> &'static str {
    match declared {
        "int" => "INTEGER",
        _ => "TEXT",
    }
}

/// Write the resolved declarations.
///
/// Per declaration: the table is created from the store's declared schema when
/// it is not standing yet (the store has never been awake), then every row that
/// is not already present, column for column, is inserted.
///
/// **Idempotent by declaration, not by key.** A store's declared tables carry no
/// primary key — `params.schema` is a column→type map — so `INSERT OR REPLACE`
/// would collapse nothing and re-applying a manifest would duplicate every row.
/// `seed_rows` therefore says *these rows are present*: a row that already
/// matches on every column it names is counted and not written again. That is
/// what makes `meclaw --apply` of the same manifest twice a no-op, and it is why
/// the digest is the thing that changes when a row changes.
pub fn apply_entries(resolved: &[ResolvedSeedRows]) -> Result<Vec<SeedRowsApplied>, MutationError> {
    let mut out = Vec::with_capacity(resolved.len());
    for r in resolved {
        let cell_db = r.cell_dir.join("cell.db");
        let conn = rusqlite::Connection::open(&cell_db).map_err(|e| {
            MutationError::SeedTargetNotAStore(format!(
                "{KEY} could not open {}: {e}",
                cell_db.display()
            ))
        })?;
        crate::persist::setup_cell_db(&conn).map_err(|e| {
            MutationError::SeedTargetNotAStore(format!(
                "{KEY} could not prepare {}: {e}",
                cell_db.display()
            ))
        })?;
        let col_defs = r
            .columns
            .iter()
            .map(|(c, t)| format!("\"{c}\" {}", sqlite_type(t)))
            .collect::<Vec<_>>()
            .join(", ");
        let table = &r.table;
        conn.execute(
            &format!("CREATE TABLE IF NOT EXISTS \"{table}\" ({col_defs})"),
            [],
        )
        .map_err(|e| {
            MutationError::SeedTargetNotAStore(format!(
                "{KEY} could not stand table {table} in {}: {e}",
                cell_db.display()
            ))
        })?;
        let mut inserted = 0usize;
        let mut already_present = 0usize;
        for (idx, row) in r.rows.iter().enumerate() {
            let cols: Vec<&String> = row.keys().collect();
            let values: Vec<rusqlite::types::Value> = cols
                .iter()
                .map(|c| crate::mutation::stage::json_to_sql_value(row.get(c.as_str())))
                .collect();
            let bind: Vec<&dyn rusqlite::ToSql> =
                values.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
            // NULL-safe equality (`IS`), so a declared NULL matches a stored one.
            let where_clause = cols
                .iter()
                .enumerate()
                .map(|(i, c)| format!("\"{c}\" IS ?{}", i + 1))
                .collect::<Vec<_>>()
                .join(" AND ");
            let present: i64 = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM \"{table}\" WHERE {where_clause}"),
                    bind.as_slice(),
                    |r| r.get(0),
                )
                .map_err(|e| {
                    MutationError::SeedTargetNotAStore(format!(
                        "{KEY} {}.{table} row {}: could not be checked: {e}",
                        r.target.as_str(),
                        idx + 1
                    ))
                })?;
            if present > 0 {
                already_present += 1;
                continue;
            }
            let col_list = cols
                .iter()
                .map(|c| format!("\"{c}\""))
                .collect::<Vec<_>>()
                .join(",");
            let placeholders = cols.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            conn.execute(
                &format!("INSERT INTO \"{table}\" ({col_list}) VALUES ({placeholders})"),
                bind.as_slice(),
            )
            .map_err(|e| {
                MutationError::SeedTargetNotAStore(format!(
                    "{KEY} {}.{table} row {}: insert failed: {e}",
                    r.target.as_str(),
                    idx + 1
                ))
            })?;
            inserted += 1;
        }
        tracing::info!(
            target_path = %r.target.as_str(),
            table = %r.table,
            inserted,
            already_present,
            "seed_rows: rows entered a store through the mutation door (GH #456)"
        );
        out.push(SeedRowsApplied {
            target: r.target.as_str().to_string(),
            table: r.table.clone(),
            inserted,
            already_present,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    #[test]
    fn absent_key_parses_to_nothing() {
        let decls = parse_entries(&json!({"add_nodes": []})).expect("no seed_rows is legal");
        assert!(decls.is_empty());
    }

    #[test]
    fn one_entry_parses() {
        let decls = parse_entries(&json!({
            "seed_rows": [{"target": "./store", "table": "policies", "rows": [{"id": "p1"}]}]
        }))
        .expect("well-formed");
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].target, "./store");
        assert_eq!(decls[0].table, "policies");
        assert_eq!(decls[0].rows.len(), 1);
    }

    #[test]
    fn an_entry_without_rows_is_refused() {
        let err = parse_entries(&json!({
            "seed_rows": [{"target": "./store", "table": "policies", "rows": []}]
        }))
        .expect_err("an operation that writes nothing is refused");
        assert_eq!(err.error_code(), "schema");
    }

    #[test]
    fn a_table_name_that_is_not_an_identifier_is_refused() {
        let err = parse_entries(&json!({
            "seed_rows": [{"target": "./s", "table": "poli\"cies", "rows": [{"id": 1}]}]
        }))
        .expect_err("a table name is formatted into DDL and is checked, not trusted");
        assert_eq!(err.error_code(), "schema");
    }

    #[test]
    fn a_row_that_is_not_an_object_is_refused() {
        let err = parse_entries(&json!({
            "seed_rows": [{"target": "./s", "table": "t", "rows": ["nope"]}]
        }))
        .expect_err("a row is a JSON object");
        assert_eq!(err.error_code(), "schema");
    }

    #[test]
    fn identifier_check_covers_the_shapes_ddl_cannot_take() {
        assert!(is_plain_identifier("policies"));
        assert!(is_plain_identifier("_x1"));
        assert!(!is_plain_identifier(""));
        assert!(!is_plain_identifier("1t"));
        assert!(!is_plain_identifier("a b"));
        assert!(!is_plain_identifier("a-b"));
    }
}
