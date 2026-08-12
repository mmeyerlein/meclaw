//! Phase-9 store ops: structured dispatch (no raw SQL from caller).
//! Args format: `tool_call.text` as JSON-object with `op`-field, analogous
//! to file/bash cells. Each op returns [`OpOutcome`] (rows_affected + payload
//! JSON for selects + error_code for SQL-errors).

use crate::store::query::catalog::{Catalog, CatalogError};
use crate::store::query::parse::{
    parse_filters, parse_limit, parse_order_by, parse_similar, parse_traverse,
};
use crate::store::query::sql::{
    render_similar, render_tail, render_tail_qualified, render_traverse, render_where,
    render_where_qualified,
};
use meclaw_core::serde_json::Value;

/// Outcome of a single store op. `payload` is JSON-serialisable
/// (`select`: array of row-objects; others: null).
#[derive(Debug)]
pub struct OpOutcome {
    /// The op name that produced this outcome (`"insert"`, `"select"`, …).
    pub operation: &'static str,
    /// Number of rows affected (0 for errors or no-op ops).
    pub rows_affected: i64,
    /// Result payload — `Value::Null` for non-select ops, array of row-objects
    /// for selects.
    pub payload: Value,
    /// Short machine-readable error code, or `None` on success.
    /// SQL errors are **normal** `tool_result` outcomes with this field set
    /// (brainstorm E5) — NOT `finish_reason: "error"`.
    pub error_code: Option<&'static str>,
    /// Human-readable error text from rusqlite, or `None` on success.
    pub error_text: Option<String>,
}

/// Map an optional JSON value to a `rusqlite::types::Value`. Missing key
/// or JSON null → SQL NULL.
pub fn json_to_sql_value(v: Option<&Value>) -> rusqlite::types::Value {
    use rusqlite::types::Value as V;
    match v {
        None | Some(Value::Null) => V::Null,
        Some(Value::Bool(b)) => V::Integer(if *b { 1 } else { 0 }),
        Some(Value::Number(n)) => {
            if let Some(i) = n.as_i64() {
                V::Integer(i)
            } else if let Some(f) = n.as_f64() {
                V::Real(f)
            } else {
                V::Text(n.to_string())
            }
        }
        Some(Value::String(s)) => V::Text(s.clone()),
        // Objects/Arrays serialized as JSON text (json columns use TEXT
        // storage class per SQLite JSON1 convention).
        Some(other) => V::Text(meclaw_core::serde_json::to_string(other).unwrap_or_default()),
    }
}

/// Top-level dispatcher. Returns `Err` only for malformed args (caller
/// emits an `invalid_input` error_code via the Error-Message path). SQL
/// errors are returned via [`OpOutcome::error_code`] (= normal
/// `tool_result` with `error_code`, NOT `finish_reason:"error"`; see
/// brainstorm E5).
pub fn dispatch(conn: &rusqlite::Connection, args: &Value) -> Result<OpOutcome, String> {
    dispatch_with(conn, args, &CanonicalMap::new())
}

/// The canonical bindings of a store, keyed by the table they belong to.
///
/// A table carries one binding per identity dimension (0.2.0 P4): the relation
/// (`predicate`) and the entity (`subject`) are separate mappings over the same
/// rows.
pub type CanonicalMap = std::collections::BTreeMap<String, Vec<crate::store::CanonicalSpec>>;

/// [`dispatch`] with the store's canonical bindings in hand (0.2.0 P2, ruling Q3).
///
/// The bindings are what makes the derived column store-owned: `insert` and
/// `update` fill it from the alias table on every write, `canonicalize` re-derives
/// it wholesale, and `set_alias` maintains the mapping. Without a binding for the
/// table in question every op behaves exactly as it did before — [`dispatch`] is
/// that case, and the two production call sites pass `params.canonical`.
pub fn dispatch_with(
    conn: &rusqlite::Connection,
    args: &Value,
    canonical: &CanonicalMap,
) -> Result<OpOutcome, String> {
    let obj = args.as_object().ok_or("args must be JSON object")?;
    // B.1: the input field is canonically `operation` (cell-types.md Z.65),
    // matching the `operation` output header — not `op`.
    let op = obj
        .get("operation")
        .and_then(|v| v.as_str())
        .ok_or("missing operation")?;
    match op {
        "insert" => op_insert(conn, obj, canonical),
        "select" => op_select(conn, obj),
        "update" => op_update(conn, obj, canonical),
        "delete" => op_delete(conn, obj),
        "create_table" => op_create_table(conn, obj),
        "search" => op_search(conn, obj),
        "traverse" => op_traverse(conn, obj),
        "similar" => op_similar(conn, obj),
        "set_alias" => op_set_alias(conn, obj, canonical),
        "reject_pair" => op_reject_pair(conn, obj, canonical),
        "canonicalize" => op_canonicalize(conn, obj, canonical),
        "alias_candidates" => op_alias_candidates(conn, obj, canonical),
        other => Err(format!("unknown op {other:?}")),
    }
}

/// Resolve one written value through the alias table — the ONE hop the store does.
///
/// Deliberately not transitive: `a -> b -> c` resolves to `b`, never to `c`. The
/// judgement that writes an alias (the nightly GC) knows the whole picture and is
/// expected to write it already resolved; a store that chased chains would have to
/// decide what a cycle means, and would answer differently depending on how a
/// mapping happened to be split.
fn resolve_alias(
    conn: &rusqlite::Connection,
    aliases: &str,
    written: &str,
) -> rusqlite::Result<Option<String>> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        &format!("SELECT \"canonical\" FROM \"{aliases}\" WHERE \"alias\" = ?1"),
        [written],
        |r| r.get(0),
    )
    .optional()
}

/// The key one written value is looked up and stored under: the value itself, or
/// its normal form when the binding normalises (0.2.0 P4, ruling Q5).
///
/// One function for the write path (`set_alias`) and the read path
/// (`derive_target`), because an alias is only ever found again if both sides
/// agree on the key.
fn alias_key(spec: &crate::store::CanonicalSpec, value: &str) -> String {
    if spec.normalize {
        crate::store::query::normalize::normalize(value)
    } else {
        value.to_string()
    }
}

/// The canonical value a written row implies, or `None` when the row carries no
/// usable source value (then the target stays NULL — nothing is invented).
///
/// Two steps, in this order: the alias table wins (the GC's judgement), and the
/// key itself is the fallback. Under a normalising binding both are normal forms,
/// which is exactly the automatic merge of ruling Q5 — two spellings that
/// normalise onto each other reach the same identity without any alias at all.
fn derive_target(
    conn: &rusqlite::Connection,
    spec: &crate::store::CanonicalSpec,
    written: Option<&Value>,
) -> rusqlite::Result<Option<String>> {
    let Some(s) = written.and_then(|v| v.as_str()).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let key = alias_key(spec, s);
    if key.is_empty() {
        return Ok(None);
    }
    let hit = resolve_alias(conn, &spec.aliases, &key)?;
    Ok(Some(match hit {
        Some(c) => alias_key(spec, &c),
        None => key,
    }))
}

/// The bindings of the table an op names, narrowed by the optional `column`
/// selector (0.2.0 P4).
///
/// `column` names the binding's SOURCE column — the dimension the caller means
/// (`"subject"`, `"predicate"`). Omitting it means "every binding of the table",
/// which is what keeps P2's `{"operation": "canonicalize", "table": "facts"}`
/// meaning the whole row's identity.
fn bindings_of<'a>(
    args: &meclaw_core::serde_json::Map<String, Value>,
    canonical: &'a CanonicalMap,
    op: &str,
) -> Result<(String, Vec<&'a crate::store::CanonicalSpec>), String> {
    let table = args
        .get("table")
        .and_then(|v| v.as_str())
        .ok_or("missing table")?;
    let specs = canonical
        .get(table)
        .ok_or_else(|| format!("{op}: table {table:?} declares no params.canonical binding"))?;
    let column = match args.get("column") {
        None => None,
        Some(v) => Some(
            v.as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| format!("{op}: column must be a non-empty string"))?,
        ),
    };
    let picked: Vec<&crate::store::CanonicalSpec> = match column {
        None => specs.iter().collect(),
        Some(col) => specs.iter().filter(|s| s.source == col).collect(),
    };
    if picked.is_empty() {
        return Err(format!(
            "{op}: table {table:?} has no canonical binding for column {:?}",
            column.unwrap_or_default()
        ));
    }
    Ok((table.to_string(), picked))
}

/// The ONE binding an op needs that speaks about a single dimension
/// (`set_alias`, `alias_candidates`).
///
/// A table with several bindings has to be told which one, because an alias is a
/// statement about one dimension and guessing would write it into the wrong
/// mapping. A table with exactly one binding needs no selector — that is P2's
/// shape and it keeps working unchanged.
fn one_binding_of<'a>(
    args: &meclaw_core::serde_json::Map<String, Value>,
    canonical: &'a CanonicalMap,
    op: &str,
) -> Result<&'a crate::store::CanonicalSpec, String> {
    let (table, picked) = bindings_of(args, canonical, op)?;
    if picked.len() > 1 {
        return Err(format!(
            "{op}: table {table:?} carries {} canonical bindings — name the dimension via \
             \"column\"",
            picked.len()
        ));
    }
    Ok(picked[0])
}

fn op_insert(
    conn: &rusqlite::Connection,
    args: &meclaw_core::serde_json::Map<String, Value>,
    canonical: &CanonicalMap,
) -> Result<OpOutcome, String> {
    let table = args
        .get("table")
        .and_then(|v| v.as_str())
        .ok_or("missing table")?;
    let row = args
        .get("row")
        .and_then(|v| v.as_object())
        .ok_or("missing row object")?;
    if row.is_empty() {
        return Err("row must declare at least one column".into());
    }
    let cat = match Catalog::load(conn, table) {
        Ok(c) => c,
        Err(e) => return Ok(catalog_error_outcome("insert", e)),
    };
    // The derived column is STORE-OWNED: a caller-supplied value is dropped and
    // replaced by the one the alias table implies, so the invariant "target is
    // always derived from source" holds for every writer without exception.
    let specs = canonical.get(cat.table()).map(Vec::as_slice).unwrap_or(&[]);
    let mut cols: Vec<String> = row
        .keys()
        .filter(|c| !specs.iter().any(|s| *c == &s.target))
        .cloned()
        .collect();
    let mut vals: Vec<rusqlite::types::Value> = cols
        .iter()
        .map(|c| json_to_sql_value(row.get(c.as_str())))
        .collect();
    for spec in specs {
        match derive_target(conn, spec, row.get(&spec.source)) {
            Ok(Some(v)) => {
                cols.push(spec.target.clone());
                vals.push(rusqlite::types::Value::Text(v));
            }
            Ok(None) => {}
            Err(e) => return Ok(sql_error_outcome("insert", &e)),
        }
    }
    if cols.is_empty() {
        return Err("row must declare at least one column".into());
    }
    let resolved = match resolve_columns(&cat, &cols) {
        Ok(c) => c,
        Err(e) => return Ok(catalog_error_outcome("insert", e)),
    };
    let col_list = resolved
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(",");
    let placeholders = cols.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let stmt = format!(
        "INSERT INTO \"{}\" ({col_list}) VALUES ({placeholders})",
        cat.table()
    );
    let bind: Vec<&dyn rusqlite::ToSql> = vals.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
    match conn.execute(&stmt, bind.as_slice()) {
        Ok(rows) => Ok(OpOutcome {
            operation: "insert",
            rows_affected: rows as i64,
            payload: Value::Null,
            error_code: None,
            error_text: None,
        }),
        Err(e) => Ok(sql_error_outcome("insert", &e)),
    }
}

fn op_select(
    conn: &rusqlite::Connection,
    args: &meclaw_core::serde_json::Map<String, Value>,
) -> Result<OpOutcome, String> {
    let table = args
        .get("table")
        .and_then(|v| v.as_str())
        .ok_or("missing table")?;
    let columns = args
        .get("columns")
        .and_then(|v| v.as_array())
        .ok_or("missing columns array")?;
    let col_names: Vec<String> = columns
        .iter()
        .map(|v| {
            v.as_str()
                .ok_or("columns entry not string")
                .map(|s| s.to_string())
        })
        .collect::<Result<_, _>>()?;
    if col_names.is_empty() {
        return Err("columns must declare at least one entry".into());
    }
    let cat = match Catalog::load(conn, table) {
        Ok(c) => c,
        Err(e) => return Ok(catalog_error_outcome("select", e)),
    };
    let resolved = match resolve_columns(&cat, &col_names) {
        Ok(c) => c,
        Err(e) => return Ok(catalog_error_outcome("select", e)),
    };
    let col_list = resolved
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(",");
    let (where_clause, mut where_vals) = match build_where(args.get("where"), &cat)? {
        Ok(w) => w,
        Err(e) => return Ok(catalog_error_outcome("select", e)),
    };
    let order_by = parse_order_by(args.get("order_by"))?;
    let limit = parse_limit(args.get("limit"))?;
    let tail = match render_tail(&order_by, limit, &cat, &mut where_vals) {
        Ok(t) => t,
        Err(e) => return Ok(catalog_error_outcome("select", e)),
    };
    let stmt = format!(
        "SELECT {col_list} FROM \"{}\"{where_clause}{tail}",
        cat.table()
    );

    let mut prepared = match conn.prepare(&stmt) {
        Ok(p) => p,
        Err(e) => return Ok(sql_error_outcome("select", &e)),
    };
    let bind: Vec<&dyn rusqlite::ToSql> = where_vals
        .iter()
        .map(|v| v as &dyn rusqlite::ToSql)
        .collect();
    let mut rows = match prepared.query(bind.as_slice()) {
        Ok(r) => r,
        Err(e) => return Ok(sql_error_outcome("select", &e)),
    };
    let mut out = Vec::new();
    loop {
        match rows.next() {
            Ok(Some(row)) => {
                let mut row_obj = meclaw_core::serde_json::Map::new();
                for (i, col) in col_names.iter().enumerate() {
                    row_obj.insert(col.clone(), sql_to_json_value(row, i));
                }
                out.push(Value::Object(row_obj));
            }
            Ok(None) => break,
            Err(e) => return Ok(sql_error_outcome("select", &e)),
        }
    }
    Ok(OpOutcome {
        operation: "select",
        rows_affected: out.len() as i64,
        payload: Value::Array(out),
        error_code: None,
        error_text: None,
    })
}

/// P3 `search`: FTS5 `MATCH` over the opt-in index of a table, joined back to
/// the base table for projection and `where` filtering.
///
/// Every row carries an extra `rank` column (`bm25`, **smaller is better**).
/// Without an explicit `order_by` the result is ranked best-first with `rowid`
/// as a deterministic tiebreaker; with one, the caller's order wins and `rank`
/// is still reported. The `match` expression is FTS5 query syntax, bound as a
/// parameter — a syntax error is a regular `sql_error` outcome, never injection.
fn op_search(
    conn: &rusqlite::Connection,
    args: &meclaw_core::serde_json::Map<String, Value>,
) -> Result<OpOutcome, String> {
    let table = args
        .get("table")
        .and_then(|v| v.as_str())
        .ok_or("missing table")?;
    let match_expr = args
        .get("match")
        .and_then(|v| v.as_str())
        .ok_or("missing match expression")?
        .to_string();
    let columns = args
        .get("columns")
        .and_then(|v| v.as_array())
        .ok_or("missing columns array")?;
    let col_names: Vec<String> = columns
        .iter()
        .map(|v| {
            v.as_str()
                .ok_or("columns entry not string")
                .map(|s| s.to_string())
        })
        .collect::<Result<_, _>>()?;
    if col_names.is_empty() {
        return Err("columns must declare at least one entry".into());
    }
    let cat = match Catalog::load(conn, table) {
        Ok(c) => c,
        Err(e) => return Ok(catalog_error_outcome("search", e)),
    };
    // The index table is catalog-resolved too, so a table without a declared
    // FTS index answers `unknown_table` naming the missing index.
    let index = match Catalog::load(conn, &format!("{}_fts", cat.table())) {
        Ok(c) => c,
        Err(e) => return Ok(catalog_error_outcome("search", e)),
    };
    let resolved = match resolve_columns(&cat, &col_names) {
        Ok(c) => c,
        Err(e) => return Ok(catalog_error_outcome("search", e)),
    };
    let col_list = resolved
        .iter()
        .map(|c| format!("\"{}\".\"{c}\"", cat.table()))
        .collect::<Vec<_>>()
        .join(",");
    // The where clause filters the BASE table, so its columns are qualified —
    // base table and index share the indexed column names.
    let filters = parse_filters(args.get("where"))?;
    let (where_clause, where_vals) = match render_where_qualified(&filters, &cat, true) {
        Ok(w) => w,
        Err(e) => return Ok(catalog_error_outcome("search", e)),
    };
    let mut vals: Vec<rusqlite::types::Value> = vec![rusqlite::types::Value::Text(match_expr)];
    vals.extend(where_vals);
    let order_by = parse_order_by(args.get("order_by"))?;
    let limit = parse_limit(args.get("limit"))?;
    let tail = if order_by.is_empty() {
        // Default order: best bm25 first (smaller is better), rowid as the
        // deterministic tiebreaker.
        let mut t = format!(" ORDER BY \"rank\" ASC, \"{}\".\"rowid\" ASC", cat.table());
        if let Some(n) = limit {
            t.push_str(" LIMIT ?");
            vals.push(rusqlite::types::Value::Integer(n));
        }
        t
    } else {
        match render_tail_qualified(&order_by, limit, &cat, &mut vals, true) {
            Ok(t) => t,
            Err(e) => return Ok(catalog_error_outcome("search", e)),
        }
    };
    let base_where = where_clause.trim_start_matches(" WHERE ");
    let filter = if base_where.is_empty() {
        String::new()
    } else {
        format!(" AND {base_where}")
    };
    let stmt = format!(
        "SELECT {col_list}, bm25(\"{idx}\") AS \"rank\" FROM \"{idx}\" \
         JOIN \"{base}\" ON \"{base}\".\"rowid\" = \"{idx}\".\"rowid\" \
         WHERE \"{idx}\" MATCH ?{filter}{tail}",
        idx = index.table(),
        base = cat.table()
    );
    let mut prepared = match conn.prepare(&stmt) {
        Ok(p) => p,
        Err(e) => return Ok(sql_error_outcome("search", &e)),
    };
    let bind: Vec<&dyn rusqlite::ToSql> = vals.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
    let mut rows = match prepared.query(bind.as_slice()) {
        Ok(r) => r,
        Err(e) => return Ok(sql_error_outcome("search", &e)),
    };
    let mut out = Vec::new();
    loop {
        match rows.next() {
            Ok(Some(row)) => {
                let mut row_obj = meclaw_core::serde_json::Map::new();
                for (i, col) in col_names.iter().enumerate() {
                    row_obj.insert(col.clone(), sql_to_json_value(row, i));
                }
                row_obj.insert("rank".into(), sql_to_json_value(row, col_names.len()));
                out.push(Value::Object(row_obj));
            }
            Ok(None) => break,
            Err(e) => return Ok(sql_error_outcome("search", &e)),
        }
    }
    Ok(OpOutcome {
        operation: "search",
        rows_affected: out.len() as i64,
        payload: Value::Array(out),
        error_code: None,
        error_text: None,
    })
}

/// P4 `traverse`: multi-hop walk over an edge table via a recursive CTE
/// (memory-spec A.2.4).
///
/// The payload is an object, not an array: it carries the paths **plus** the
/// `truncated` flag and the guards that produced them. Truncation must be
/// visible — the P1 precedent (`scan_truncated`) applies, and the output header
/// set is frozen, so the payload is where it belongs.
fn op_traverse(
    conn: &rusqlite::Connection,
    args: &meclaw_core::serde_json::Map<String, Value>,
) -> Result<OpOutcome, String> {
    let table = args
        .get("table")
        .and_then(|v| v.as_str())
        .ok_or("missing table")?;
    let spec = parse_traverse(args)?;
    let cat = match Catalog::load(conn, table) {
        Ok(c) => c,
        Err(e) => return Ok(catalog_error_outcome("traverse", e)),
    };
    let (stmt, vals) = match render_traverse(&spec, &cat) {
        Ok(s) => s,
        Err(e) => return Ok(catalog_error_outcome("traverse", e)),
    };
    // CTE column order: node, depth, [weight_sum], path, then the edge
    // attributes in the order the renderer emitted them.
    let mut names = vec!["node".to_string(), "depth".to_string()];
    if spec.weight.is_some() {
        names.push("weight_sum".to_string());
    }
    names.push("path".to_string());
    let mut edge_names: Vec<String> = Vec::new();
    if spec.kind.is_some() {
        edge_names.push("kind".to_string());
    }
    if spec.weight.is_some() {
        edge_names.push("weight".to_string());
    }
    edge_names.extend(spec.columns.iter().cloned());
    names.extend((0..edge_names.len()).map(|i| format!("e{i}")));

    let rows = match query_rows(conn, &stmt, &vals, &names) {
        Ok(r) => r,
        Err(e) => return Ok(sql_error_outcome("traverse", &e)),
    };
    let truncated = rows.len() as i64 > spec.max_nodes;
    let paths: Vec<Value> = rows
        .into_iter()
        .take(spec.max_nodes as usize)
        .map(|r| traverse_row_to_path(&r, &edge_names))
        .collect();
    let payload = meclaw_core::serde_json::json!({
        "paths": paths,
        "truncated": truncated,
        "max_depth": spec.max_depth,
        "max_nodes": spec.max_nodes,
    });
    Ok(OpOutcome {
        operation: "traverse",
        rows_affected: paths_len(&payload),
        payload,
        error_code: None,
        error_text: None,
    })
}

/// Number of paths in a traverse payload (the value of the `rows_affected` header).
fn paths_len(payload: &Value) -> i64 {
    payload["paths"].as_array().map(|a| a.len()).unwrap_or(0) as i64
}

/// Reshape one CTE row into a path object: the visited nodes as an array, and
/// the last edge's attributes under their caller-facing names.
fn traverse_row_to_path(row: &Value, edge_names: &[String]) -> Value {
    let mut out = meclaw_core::serde_json::Map::new();
    out.insert("node".into(), row["node"].clone());
    out.insert("depth".into(), row["depth"].clone());
    if let Some(w) = row.get("weight_sum") {
        out.insert("weight_sum".into(), w.clone());
    }
    let nodes: Vec<Value> = row["path"]
        .as_str()
        .unwrap_or_default()
        .split('\u{1e}')
        .filter(|s| !s.is_empty())
        .map(|s| Value::String(s.to_string()))
        .collect();
    out.insert("path".into(), Value::Array(nodes));
    if !edge_names.is_empty() {
        let mut edge = meclaw_core::serde_json::Map::new();
        for (i, name) in edge_names.iter().enumerate() {
            edge.insert(name.clone(), row[format!("e{i}")].clone());
        }
        out.insert("edge".into(), Value::Object(edge));
    }
    Value::Object(out)
}

/// P4 `similar`: nearest-neighbour ranking over a binarized vector column via
/// the registered `hamming` scalar function (memory-spec A.2.5).
///
/// Every row carries an extra `distance` column (**smaller is better**, like
/// `rank` in `search`). Rows whose vector is NULL — the embedding backfill queue
/// of memory-spec B.1.1 — are excluded by the renderer, because NULL would sort
/// to the top. Comparing across embedding generations is a caller error and
/// surfaces as a regular `sql_error` naming the length mismatch.
fn op_similar(
    conn: &rusqlite::Connection,
    args: &meclaw_core::serde_json::Map<String, Value>,
) -> Result<OpOutcome, String> {
    let table = args
        .get("table")
        .and_then(|v| v.as_str())
        .ok_or("missing table")?;
    let spec = parse_similar(args)?;
    let cat = match Catalog::load(conn, table) {
        Ok(c) => c,
        Err(e) => return Ok(catalog_error_outcome("similar", e)),
    };
    let (stmt, vals) = match render_similar(&spec, &cat) {
        Ok(s) => s,
        Err(e) => return Ok(catalog_error_outcome("similar", e)),
    };
    let mut names = spec.columns.clone();
    names.push(crate::store::query::DISTANCE_COLUMN.to_string());
    match query_rows(conn, &stmt, &vals, &names) {
        Ok(rows) => Ok(OpOutcome {
            operation: "similar",
            rows_affected: rows.len() as i64,
            payload: Value::Array(rows),
            error_code: None,
            error_text: None,
        }),
        Err(e) => Ok(sql_error_outcome("similar", &e)),
    }
}

/// Run a statement and materialize its rows as JSON objects keyed by `names`
/// (positional — the renderer owns the projection order).
fn query_rows(
    conn: &rusqlite::Connection,
    stmt: &str,
    vals: &[rusqlite::types::Value],
    names: &[String],
) -> rusqlite::Result<Vec<Value>> {
    let mut prepared = conn.prepare(stmt)?;
    let bind: Vec<&dyn rusqlite::ToSql> = vals.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
    let mut rows = prepared.query(bind.as_slice())?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let mut row_obj = meclaw_core::serde_json::Map::new();
        for (i, name) in names.iter().enumerate() {
            row_obj.insert(name.clone(), sql_to_json_value(row, i));
        }
        out.push(Value::Object(row_obj));
    }
    Ok(out)
}

fn sql_to_json_value(row: &rusqlite::Row, idx: usize) -> Value {
    use rusqlite::types::ValueRef;
    match row.get_ref_unwrap(idx) {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::from(i),
        ValueRef::Real(f) => Value::from(f),
        ValueRef::Text(b) => Value::from(String::from_utf8_lossy(b).to_string()),
        ValueRef::Blob(b) => Value::from(b.to_vec()),
    }
}

/// Build the `WHERE` clause + bound values for an op (P3).
///
/// Shape errors (unknown operator, malformed spec) are `Err` — the caller turns
/// them into the existing `invalid_input` path. Identifier failures come back as
/// [`CatalogError`] and become a regular `tool_result` with `unknown_column`,
/// exactly where SQLite used to produce one.
fn build_where(
    where_v: Option<&Value>,
    cat: &Catalog,
) -> Result<Result<(String, Vec<rusqlite::types::Value>), CatalogError>, String> {
    let filters = parse_filters(where_v)?;
    Ok(render_where(&filters, cat))
}

/// Map a catalog failure onto the store's existing error codes — no new code is
/// introduced by P3 (`docs/cell-types.md` § store, Failure-Klassifikation).
fn catalog_error_outcome(op: &'static str, e: CatalogError) -> OpOutcome {
    let (code, text) = match e {
        CatalogError::UnknownTable(t) => ("unknown_table", format!("no such table: {t}")),
        CatalogError::UnknownColumn(c) => ("unknown_column", format!("no such column: {c}")),
        CatalogError::Sql(err) => return sql_error_outcome(op, &err),
    };
    OpOutcome {
        operation: op,
        rows_affected: 0,
        payload: Value::Null,
        error_code: Some(code),
        error_text: Some(text),
    }
}

/// Resolve a list of caller-supplied column names to their catalog spellings.
fn resolve_columns<'a>(cat: &'a Catalog, wanted: &[String]) -> Result<Vec<&'a str>, CatalogError> {
    wanted.iter().map(|c| cat.column(c)).collect()
}

fn op_update(
    conn: &rusqlite::Connection,
    args: &meclaw_core::serde_json::Map<String, Value>,
    canonical: &CanonicalMap,
) -> Result<OpOutcome, String> {
    let table = args
        .get("table")
        .and_then(|v| v.as_str())
        .ok_or("missing table")?;
    let set = args
        .get("set")
        .and_then(|v| v.as_object())
        .ok_or("missing set object")?;
    if set.is_empty() {
        return Err("set must declare at least one column".into());
    }
    let cat = match Catalog::load(conn, table) {
        Ok(c) => c,
        Err(e) => return Ok(catalog_error_outcome("update", e)),
    };
    // Same ownership rule as the insert path: a write that moves the source moves
    // the derived column WITH it, and a direct write to the derived column is not
    // a thing a caller can do. Without this a predicate rewrite would leave a
    // stale identity behind, which is the one failure mode that would make a read
    // answer differently than the data says.
    let specs = canonical.get(cat.table()).map(Vec::as_slice).unwrap_or(&[]);
    let mut set_cols: Vec<String> = set
        .keys()
        .filter(|c| !specs.iter().any(|s| *c == &s.target))
        .cloned()
        .collect();
    let mut vals: Vec<rusqlite::types::Value> = set_cols
        .iter()
        .map(|c| json_to_sql_value(set.get(c.as_str())))
        .collect();
    for spec in specs {
        let Some(written) = set.get(&spec.source) else {
            continue;
        };
        match derive_target(conn, spec, Some(written)) {
            Ok(derived) => {
                set_cols.push(spec.target.clone());
                vals.push(match derived {
                    Some(v) => rusqlite::types::Value::Text(v),
                    None => rusqlite::types::Value::Null,
                });
            }
            Err(e) => return Ok(sql_error_outcome("update", &e)),
        }
    }
    if set_cols.is_empty() {
        return Err("set must declare at least one column".into());
    }
    let resolved = match resolve_columns(&cat, &set_cols) {
        Ok(c) => c,
        Err(e) => return Ok(catalog_error_outcome("update", e)),
    };
    let set_clause = resolved
        .iter()
        .map(|c| format!("\"{c}\" = ?"))
        .collect::<Vec<_>>()
        .join(", ");
    let (where_clause, where_vals) = match build_where(args.get("where"), &cat)? {
        Ok(w) => w,
        Err(e) => return Ok(catalog_error_outcome("update", e)),
    };
    vals.extend(where_vals);
    let stmt = format!("UPDATE \"{}\" SET {set_clause}{where_clause}", cat.table());
    let bind: Vec<&dyn rusqlite::ToSql> = vals.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
    match conn.execute(&stmt, bind.as_slice()) {
        Ok(rows) => Ok(OpOutcome {
            operation: "update",
            rows_affected: rows as i64,
            payload: Value::Null,
            error_code: None,
            error_text: None,
        }),
        Err(e) => Ok(sql_error_outcome("update", &e)),
    }
}

/// 0.2.0 P2 `set_alias`: state that one written spelling MEANS another (ruling Q3).
///
/// An upsert, not an insert: the nightly GC re-judges the same pair on every run
/// and the invariance gate may make it run twice over the same store, so writing
/// the same alias again has to be a no-op rather than a second row. Nothing is
/// derived here — ruling Q3 orders the dream run as `set_alias` … then
/// `canonicalize`, so that the aliases of one run land as ONE consistent set
/// before any column moves.
///
/// The reverse is the ordinary `delete` op on the alias table plus a
/// `canonicalize`: that is the revert path, and it destroys no fact — the written
/// predicate has been sitting untouched next to the derived one the whole time.
fn op_set_alias(
    conn: &rusqlite::Connection,
    args: &meclaw_core::serde_json::Map<String, Value>,
    canonical: &CanonicalMap,
) -> Result<OpOutcome, String> {
    let spec = one_binding_of(args, canonical, "set_alias")?;
    let alias = args
        .get("alias")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("set_alias: missing non-empty alias")?;
    let target = args
        .get("canonical")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("set_alias: missing non-empty canonical")?;
    // Under a normalising binding the alias table is keyed on the NORMAL form
    // (0.2.0 P4): one judgement then covers every spelling of the alias that
    // differs only in case, whitespace or Unicode composition, instead of the GC
    // having to write one row per variant it happens to have seen.
    let alias = &alias_key(spec, alias);
    let target = &alias_key(spec, target);
    let recorded_at = args.get("recorded_at").and_then(|v| v.as_str());
    let aliases = &spec.aliases;
    let stmt = format!(
        "INSERT INTO \"{aliases}\" (\"alias\", \"canonical\", \"recorded_at\") VALUES (?1, ?2, ?3) \
         ON CONFLICT(\"alias\") DO UPDATE SET \"canonical\" = excluded.\"canonical\", \
         \"recorded_at\" = excluded.\"recorded_at\""
    );
    match conn.execute(&stmt, rusqlite::params![alias, target, recorded_at]) {
        Ok(rows) => Ok(OpOutcome {
            operation: "set_alias",
            rows_affected: rows as i64,
            payload: Value::Null,
            error_code: None,
            error_text: None,
        }),
        Err(e) => Ok(sql_error_outcome("set_alias", &e)),
    }
}

/// The two values of an unordered pair, keyed the way the binding keys identities
/// and put into a fixed order (0.2.0 P5).
///
/// Ordering here is what makes the pair ONE row: the GC hands a pair over the way
/// [`op_alias_candidates`] served it, and a refusal must not depend on which side
/// it happened to name first.
fn pair_key(
    spec: &crate::store::CanonicalSpec,
    args: &meclaw_core::serde_json::Map<String, Value>,
) -> Result<(String, String), String> {
    let side = |name: &str| -> Result<String, String> {
        let raw = args
            .get(name)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("reject_pair: missing non-empty {name}"))?;
        let key = alias_key(spec, raw);
        if key.is_empty() {
            return Err(format!("reject_pair: {name} is empty after normalisation"));
        }
        Ok(key)
    };
    let (a, b) = (side("left")?, side("right")?);
    if a == b {
        return Err(
            "reject_pair: left and right are the same identity — there is nothing to refuse".into(),
        );
    }
    Ok(if a < b { (a, b) } else { (b, a) })
}

/// 0.2.0 P5 `reject_pair`: remember that two candidates are NOT the same identity.
///
/// The counterpart of `set_alias` and the one P4 left open: only ACCEPTED pairs
/// disappeared from the candidate feed, so every night the GC paid a top-tier
/// model to turn the same handful of pairs down again. An alias row cannot carry
/// the refusal (`canonical` is `NOT NULL`, and a NULL there would read as
/// "resolves to nothing"), so the negative judgement gets a table of its own.
///
/// An upsert on the ordered pair, like `set_alias` is on the alias: re-judging
/// writes no second row, which is what lets a dream run be replayed. The revert
/// is the ordinary `delete` on that table — after which the pair is a question
/// again, and nothing else in the store has changed, because a refusal never
/// touched a fact in the first place.
fn op_reject_pair(
    conn: &rusqlite::Connection,
    args: &meclaw_core::serde_json::Map<String, Value>,
    canonical: &CanonicalMap,
) -> Result<OpOutcome, String> {
    let spec = one_binding_of(args, canonical, "reject_pair")?;
    let rejected = spec.rejected.as_deref().ok_or_else(|| {
        "reject_pair: this binding declares no params.canonical.rejected table".to_string()
    })?;
    let (left, right) = pair_key(spec, args)?;
    let recorded_at = args.get("recorded_at").and_then(|v| v.as_str());
    let stmt = format!(
        "INSERT INTO \"{rejected}\" (\"left_value\", \"right_value\", \"recorded_at\") \
         VALUES (?1, ?2, ?3) ON CONFLICT(\"left_value\", \"right_value\") DO UPDATE SET \
         \"recorded_at\" = excluded.\"recorded_at\""
    );
    match conn.execute(&stmt, rusqlite::params![left, right, recorded_at]) {
        Ok(rows) => Ok(OpOutcome {
            operation: "reject_pair",
            rows_affected: rows as i64,
            payload: Value::Null,
            error_code: None,
            error_text: None,
        }),
        Err(e) => Ok(sql_error_outcome("reject_pair", &e)),
    }
}

/// 0.2.0 P2 `canonicalize`: re-derive the whole target column (ruling Q3).
///
/// This is both the follow-up step of a dream run (aliases written, column pulled
/// after) and the REVERT path (alias row deleted, column falls back to the
/// original). The originals are read, never written, so the operation is
/// idempotent and reversible by construction.
///
/// `rows_affected` counts only the rows whose value actually CHANGED — a second
/// run over unchanged data reports 0, which is what makes it usable as a receipt
/// for the invariance gate.
fn op_canonicalize(
    conn: &rusqlite::Connection,
    args: &meclaw_core::serde_json::Map<String, Value>,
    canonical: &CanonicalMap,
) -> Result<OpOutcome, String> {
    let (table, specs) = bindings_of(args, canonical, "canonicalize")?;
    // Catalog first: on a `cell.db` whose migration did not run, the caller gets
    // `unknown_table` / `unknown_column` instead of a raw SQL error.
    let cat = match Catalog::load(conn, &table) {
        Ok(c) => c,
        Err(e) => return Ok(catalog_error_outcome("canonicalize", e)),
    };
    let mut moved = 0i64;
    for spec in specs {
        if let Err(e) = resolve_columns(&cat, &[spec.source.clone(), spec.target.clone()]) {
            return Ok(catalog_error_outcome("canonicalize", e));
        }
        let expr = crate::store::ddl::canonical_derive_expr(cat.table(), spec);
        let target = &spec.target;
        let stmt = format!(
            "UPDATE \"{}\" SET \"{target}\" = {expr} WHERE \"{target}\" IS NOT {expr}",
            cat.table()
        );
        match conn.execute(&stmt, []) {
            // One statement per dimension, summed: `rows_affected` is the receipt
            // "the run moved N identities", and a row whose subject AND predicate
            // both moved is two of them.
            Ok(rows) => moved += rows as i64,
            Err(e) => return Ok(sql_error_outcome("canonicalize", &e)),
        }
    }
    Ok(OpOutcome {
        operation: "canonicalize",
        rows_affected: moved,
        payload: Value::Null,
        error_code: None,
        error_text: None,
    })
}

/// Default similarity below which a pair is not worth the GC's attention.
const CANDIDATE_MIN_SCORE: f64 = 0.5;
/// Default number of pairs returned.
const CANDIDATE_LIMIT: i64 = 20;
/// Default and ceiling for the number of DISTINCT identities compared.
///
/// The comparison is quadratic in this number, and it is the only knob that can
/// make the op expensive — hence a bound the caller may lower but not raise. Even
/// at the ceiling this is a few million short string comparisons in one op, which
/// is a nightly job, not a hot path.
const CANDIDATE_MAX_VALUES: i64 = 500;
const CANDIDATE_MAX_VALUES_CAP: i64 = 5000;

/// One caller-supplied numeric argument of `alias_candidates`, bounded.
fn candidate_number(
    args: &meclaw_core::serde_json::Map<String, Value>,
    key: &str,
    default: i64,
    max: i64,
) -> Result<i64, String> {
    match args.get(key) {
        None => Ok(default),
        Some(v) => {
            let n = v
                .as_i64()
                .ok_or_else(|| format!("alias_candidates: {key} must be an integer"))?;
            if n < 1 || n > max {
                return Err(format!("alias_candidates: {key} must be 1..={max}"));
            }
            Ok(n)
        }
    }
}

/// 0.2.0 P4 `alias_candidates`: pairs of identities that MIGHT be the same entity
/// (ruling Q5).
///
/// The feed of the nightly GC, and it is deliberately only that: the op reads,
/// scores and sorts — it merges nothing, writes nothing and decides nothing.
/// Ruling Q5 puts the judgement at the GC with a top-tier model and both nodes in
/// context, because a similarity threshold that merges on its own is wrong
/// exactly where it matters (sibling names, one-letter place names). What the GC
/// then persists is an ordinary `set_alias` — additive, revertible, behind the
/// invariance gate.
///
/// Pairs already SETTLED are excluded, in both directions (0.2.0 P5): a pair the
/// GC accepted maps onto one identity through the alias table, and a pair it
/// turned down sits in the binding's rejected-pair table. Re-proposing either
/// would bury the open questions under the settled ones — and, for the refusals,
/// pay a top-tier model nightly for an answer that is already on disk.
///
/// Candidates are drawn from the DERIVED column, so what is compared is the axes
/// that actually exist — two spellings that the normalisation already merged are
/// one value here and never show up as a pair.
fn op_alias_candidates(
    conn: &rusqlite::Connection,
    args: &meclaw_core::serde_json::Map<String, Value>,
    canonical: &CanonicalMap,
) -> Result<OpOutcome, String> {
    let spec = one_binding_of(args, canonical, "alias_candidates")?;
    let limit = candidate_number(args, "limit", CANDIDATE_LIMIT, i64::MAX)?;
    let max_values = candidate_number(
        args,
        "max_values",
        CANDIDATE_MAX_VALUES,
        CANDIDATE_MAX_VALUES_CAP,
    )?;
    let min_score = match args.get("min_score") {
        None => CANDIDATE_MIN_SCORE,
        Some(v) => {
            let f = v
                .as_f64()
                .ok_or("alias_candidates: min_score must be a number")?;
            if !(0.0..=1.0).contains(&f) {
                return Err("alias_candidates: min_score must be 0.0..=1.0".into());
            }
            f
        }
    };
    let named = args
        .get("table")
        .and_then(|v| v.as_str())
        .ok_or("missing table")?;
    let table = match Catalog::load(conn, named) {
        Ok(c) => c,
        Err(e) => return Ok(catalog_error_outcome("alias_candidates", e)),
    };
    if let Err(e) = resolve_columns(&table, std::slice::from_ref(&spec.target)) {
        return Ok(catalog_error_outcome("alias_candidates", e));
    }
    let target = &spec.target;
    let values: Vec<String> = match conn
        .prepare(&format!(
            "SELECT DISTINCT \"{target}\" FROM \"{}\" WHERE \"{target}\" IS NOT NULL \
             AND \"{target}\" <> '' ORDER BY \"{target}\" LIMIT ?1",
            table.table()
        ))
        .and_then(|mut st| {
            st.query_map([max_values], |r| r.get::<_, String>(0))?
                .collect()
        }) {
        Ok(v) => v,
        Err(e) => return Ok(sql_error_outcome("alias_candidates", &e)),
    };
    let aliases: std::collections::BTreeMap<String, String> = match conn
        .prepare(&format!(
            "SELECT \"alias\", \"canonical\" FROM \"{}\"",
            spec.aliases
        ))
        .and_then(|mut st| {
            st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect()
        }) {
        Ok(m) => m,
        Err(e) => return Ok(sql_error_outcome("alias_candidates", &e)),
    };
    // 0.2.0 P5: the pairs a previous run judged to be DIFFERENT. Without this the
    // feed re-proposes every refusal every night and the GC pays a top-tier model
    // to answer the same question again.
    let refused: std::collections::BTreeSet<(String, String)> = match &spec.rejected {
        None => Default::default(),
        Some(rejected) => match conn
            .prepare(&format!(
                "SELECT \"left_value\", \"right_value\" FROM \"{rejected}\""
            ))
            .and_then(|mut st| {
                st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                    .collect()
            }) {
            Ok(s) => s,
            Err(e) => return Ok(sql_error_outcome("alias_candidates", &e)),
        },
    };
    // One hop, exactly like the derivation: whoever writes an alias writes it
    // already resolved.
    let resolved = |v: &str| -> String {
        let key = alias_key(spec, v);
        aliases.get(&key).cloned().unwrap_or(key)
    };
    let mut pairs: Vec<(String, String, f64)> = Vec::new();
    for (i, left) in values.iter().enumerate() {
        for right in &values[i + 1..] {
            if resolved(left) == resolved(right) {
                continue;
            }
            let (a, b) = (alias_key(spec, left), alias_key(spec, right));
            let ordered = if a < b { (a, b) } else { (b, a) };
            if refused.contains(&ordered) {
                continue;
            }
            let score = crate::store::query::trigram::similarity(
                &crate::store::query::normalize::normalize(left),
                &crate::store::query::normalize::normalize(right),
            );
            if score + f64::EPSILON < min_score {
                continue;
            }
            // Four decimals: enough to order pairs, few enough that the payload
            // is byte-stable across runs and platforms.
            pairs.push((
                left.clone(),
                right.clone(),
                (score * 10_000.0).round() / 10_000.0,
            ));
        }
    }
    // Best first, then alphabetical — a total order, so two runs over the same
    // store hand the GC the same list.
    pairs.sort_by(|a, b| {
        b.2.partial_cmp(&a.2)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
            .then_with(|| a.1.cmp(&b.1))
    });
    pairs.truncate(limit as usize);
    let payload: Vec<Value> = pairs
        .into_iter()
        .map(|(left, right, score)| {
            meclaw_core::serde_json::json!({"left": left, "right": right, "score": score})
        })
        .collect();
    Ok(OpOutcome {
        operation: "alias_candidates",
        rows_affected: payload.len() as i64,
        payload: Value::Array(payload),
        error_code: None,
        error_text: None,
    })
}

fn op_create_table(
    conn: &rusqlite::Connection,
    args: &meclaw_core::serde_json::Map<String, Value>,
) -> Result<OpOutcome, String> {
    let table = args
        .get("table")
        .and_then(|v| v.as_str())
        .ok_or("missing table")?;
    let cols_obj = args
        .get("columns")
        .and_then(|v| v.as_object())
        .ok_or("missing columns object")?;
    if cols_obj.is_empty() {
        return Err("columns must declare at least one entry".into());
    }
    crate::store::ddl::check_new_identifier("create_table table", table)?;
    let mut cols = std::collections::BTreeMap::new();
    for (c, ty_v) in cols_obj {
        crate::store::ddl::check_new_identifier("create_table column", c)?;
        let ty = ty_v.as_str().ok_or("column type must be string")?;
        if !matches!(ty, "text" | "int" | "json") {
            return Err(format!("unsupported column type {ty:?}"));
        }
        cols.insert(c.clone(), ty.to_string());
    }
    let mut schema = std::collections::BTreeMap::new();
    schema.insert(table.to_string(), cols);
    match crate::store::ddl::apply_schema_ddl(conn, &schema) {
        Ok(()) => Ok(OpOutcome {
            operation: "create_table",
            rows_affected: 0,
            payload: Value::Null,
            error_code: None,
            error_text: None,
        }),
        Err(e) => Ok(sql_error_outcome("create_table", &e)),
    }
}

fn op_delete(
    conn: &rusqlite::Connection,
    args: &meclaw_core::serde_json::Map<String, Value>,
) -> Result<OpOutcome, String> {
    let table = args
        .get("table")
        .and_then(|v| v.as_str())
        .ok_or("missing table")?;
    let cat = match Catalog::load(conn, table) {
        Ok(c) => c,
        Err(e) => return Ok(catalog_error_outcome("delete", e)),
    };
    let (where_clause, where_vals) = match build_where(args.get("where"), &cat)? {
        Ok(w) => w,
        Err(e) => return Ok(catalog_error_outcome("delete", e)),
    };
    let stmt = format!("DELETE FROM \"{}\"{where_clause}", cat.table());
    let bind: Vec<&dyn rusqlite::ToSql> = where_vals
        .iter()
        .map(|v| v as &dyn rusqlite::ToSql)
        .collect();
    match conn.execute(&stmt, bind.as_slice()) {
        Ok(rows) => Ok(OpOutcome {
            operation: "delete",
            rows_affected: rows as i64,
            payload: Value::Null,
            error_code: None,
            error_text: None,
        }),
        Err(e) => Ok(sql_error_outcome("delete", &e)),
    }
}

/// Map a rusqlite error to an [`OpOutcome`] with `error_code` set.
/// Per brainstorm E5: SQL-errors are NORMAL tool_results with
/// `error_code` header, NOT `finish_reason:"error"`.
pub(crate) fn sql_error_outcome(op: &'static str, e: &rusqlite::Error) -> OpOutcome {
    let (code, text) = classify_sql_error(e);
    OpOutcome {
        operation: op,
        rows_affected: 0,
        payload: Value::Null,
        error_code: Some(code),
        error_text: Some(text),
    }
}

/// Classify a rusqlite error into one of the six store `error_code` values
/// (cell-types.md Z.71). `type_mismatch` maps to a clean `ErrorCode::TypeMismatch`
/// arm. `unknown_table` / `unknown_column` surface as `ErrorCode::Unknown`
/// (SQLITE_ERROR) and are disambiguated by conservative substring match on the
/// SQLite message text. Empirically verified message strings (rusqlite 0.x):
///   missing table  → `"no such table: <name>"`      (contains "no such table")
///   unknown column → `"table <t> has no column named <c>"` (contains "has no column named")
/// Every unrecognised error falls back to `"sql_error"` (backstop — D-015
/// Watchlist: SQLite message drift degrades to coarser code, never mismatch).
fn classify_sql_error(e: &rusqlite::Error) -> (&'static str, String) {
    let msg = e.to_string();
    match e {
        rusqlite::Error::SqliteFailure(err, _) => match err.code {
            rusqlite::ErrorCode::ConstraintViolation => ("constraint_violation", msg),
            rusqlite::ErrorCode::TypeMismatch => ("type_mismatch", msg),
            // SQLITE_ERROR (ErrorCode::Unknown) carries "no such table/column" in
            // the message. Conservative substring match; any miss falls through to
            // sql_error (backstop — misclassification impossible, only degradation).
            _ => classify_by_message(&msg),
        },
        rusqlite::Error::SqlInputError { .. } => classify_by_message(&msg),
        _ => ("sql_error", msg),
    }
}

/// Backstop classification via SQLite message text. Only the two empirically
/// verified stable substrings; otherwise `sql_error` (catch-all, D-015 Watchlist).
fn classify_by_message(msg: &str) -> (&'static str, String) {
    let lower = msg.to_ascii_lowercase();
    if lower.contains("no such table") {
        ("unknown_table", msg.to_string())
    } else if lower.contains("has no column named") {
        ("unknown_column", msg.to_string())
    } else {
        ("sql_error", msg.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    // ---- 0.2.0 P2: canonical column, alias table, re-derive (ruling Q3) ----

    fn canon_map() -> CanonicalMap {
        CanonicalMap::from([("facts".to_string(), vec![predicate_binding()])])
    }

    fn predicate_binding() -> crate::store::CanonicalSpec {
        crate::store::CanonicalSpec {
            source: "predicate".to_string(),
            target: "canonical_predicate".to_string(),
            aliases: "predicate_aliases".to_string(),
            normalize: false,
            rejected: Some("predicate_rejected_pairs".to_string()),
        }
    }

    /// The entity dimension of 0.2.0 P4: same binding shape, normalising.
    fn subject_binding() -> crate::store::CanonicalSpec {
        crate::store::CanonicalSpec {
            source: "subject".to_string(),
            target: "canonical_subject".to_string(),
            aliases: "subject_aliases".to_string(),
            normalize: true,
            rejected: Some("subject_rejected_pairs".to_string()),
        }
    }

    /// Both dimensions on `facts`, the shape the memory hive ships after P4.
    fn two_dimension_map() -> CanonicalMap {
        CanonicalMap::from([(
            "facts".to_string(),
            vec![predicate_binding(), subject_binding()],
        )])
    }

    /// A `cell.db` carrying BOTH derived columns and both alias tables.
    fn two_dimension_fixture() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::query::normalize::register(&conn).unwrap();
        conn.execute_batch(
            "CREATE TABLE facts (id TEXT, subject TEXT, canonical_subject TEXT, predicate TEXT,
                                 claim TEXT, canonical_predicate TEXT);
             CREATE TABLE predicate_aliases (alias TEXT PRIMARY KEY, canonical TEXT NOT NULL,
                                             recorded_at TEXT);
             CREATE TABLE subject_aliases (alias TEXT PRIMARY KEY, canonical TEXT NOT NULL,
                                           recorded_at TEXT);
             CREATE TABLE predicate_rejected_pairs (left_value TEXT NOT NULL,
                 right_value TEXT NOT NULL, recorded_at TEXT,
                 PRIMARY KEY (left_value, right_value));
             CREATE TABLE subject_rejected_pairs (left_value TEXT NOT NULL,
                 right_value TEXT NOT NULL, recorded_at TEXT,
                 PRIMARY KEY (left_value, right_value));",
        )
        .unwrap();
        conn
    }

    /// A `cell.db` after the P2 migration: the derived column and the alias table
    /// exist, both empty.
    fn canon_fixture() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE facts (id TEXT, subject TEXT, predicate TEXT, claim TEXT,
                                 canonical_predicate TEXT);
             CREATE TABLE predicate_aliases (alias TEXT PRIMARY KEY, canonical TEXT NOT NULL,
                                             recorded_at TEXT);
             CREATE TABLE predicate_rejected_pairs (left_value TEXT NOT NULL,
                 right_value TEXT NOT NULL, recorded_at TEXT,
                 PRIMARY KEY (left_value, right_value));",
        )
        .unwrap();
        conn
    }

    fn row_of(conn: &rusqlite::Connection, id: &str) -> (String, Option<String>) {
        conn.query_row(
            "SELECT predicate, canonical_predicate FROM facts WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    }

    fn mint(conn: &rusqlite::Connection, id: &str, predicate: &str) -> OpOutcome {
        dispatch_with(
            conn,
            &json!({"operation":"insert","table":"facts",
                    "row":{"id":id,"subject":"user:m","predicate":predicate,"claim":"Helix"}}),
            &canon_map(),
        )
        .unwrap()
    }

    // ---- 0.2.0 P4: the entity dimension, normalisation, candidates (ruling Q5) ----

    fn mint_subject(conn: &rusqlite::Connection, id: &str, subject: &str) -> OpOutcome {
        dispatch_with(
            conn,
            &json!({"operation":"insert","table":"facts",
                    "row":{"id":id,"subject":subject,"predicate":"lives_in","claim":"x"}}),
            &two_dimension_map(),
        )
        .unwrap()
    }

    fn subject_row(conn: &rusqlite::Connection, id: &str) -> (String, Option<String>) {
        conn.query_row(
            "SELECT subject, canonical_subject FROM facts WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    }

    /// Ruling Q5's automatic half, and the ONLY merge the store performs alone:
    /// two spellings equal after normalisation are one entity at MINT time, with
    /// no alias, no judgement and no model involved.
    #[test]
    fn normalisation_equality_merges_two_spellings_at_mint() {
        let conn = two_dimension_fixture();
        mint_subject(&conn, "f1", "Sonnenhof");
        mint_subject(&conn, "f2", "  sonnenhof ");
        mint_subject(&conn, "f3", "SONNEN\tHOF");
        assert_eq!(
            subject_row(&conn, "f1"),
            ("Sonnenhof".into(), Some("sonnenhof".into()))
        );
        assert_eq!(
            subject_row(&conn, "f2"),
            ("  sonnenhof ".into(), Some("sonnenhof".into())),
            "case and whitespace never make two entities"
        );
        assert_eq!(
            subject_row(&conn, "f3").1.as_deref(),
            Some("sonnen hof"),
            "an inner whitespace run collapses but does not vanish — that would be \
             a different name"
        );
        // Unicode composition: the same name in NFD and NFC is one identity
        mint_subject(&conn, "f4", "Elve\u{0301}se");
        mint_subject(&conn, "f5", "elv\u{00E9}se");
        assert_eq!(subject_row(&conn, "f4").1, subject_row(&conn, "f5").1);
    }

    /// The protection direction of ruling Q5: similarity is NOT identity. Two
    /// short names that a threshold would happily merge stay apart, because the
    /// store never merges on similarity at all.
    #[test]
    fn a_fuzzy_neighbour_is_never_merged_automatically() {
        let conn = two_dimension_fixture();
        mint_subject(&conn, "f1", "Leroy");
        mint_subject(&conn, "f2", "Leon");
        mint_subject(&conn, "f3", "elvese");
        mint_subject(&conn, "f4", "elvse");
        let axes: i64 = conn
            .query_row(
                "SELECT count(DISTINCT canonical_subject) FROM facts",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            axes, 4,
            "four names, four identities — nothing merged itself"
        );
    }

    /// The GC's judgement on the entity dimension, end to end: `set_alias` +
    /// `canonicalize` puts two spellings of one subject on ONE axis, and the
    /// relation dimension of the same rows is untouched by it.
    #[test]
    fn an_alias_on_the_subject_unifies_the_axis() {
        let conn = two_dimension_fixture();
        mint_subject(&conn, "f1", "user");
        mint_subject(&conn, "f2", "user:alpha");
        let out = dispatch_with(
            &conn,
            &json!({"operation":"set_alias","table":"facts","column":"subject",
                    "alias":"user:alpha","canonical":"user",
                    "recorded_at":"2026-08-12T03:00:00Z"}),
            &two_dimension_map(),
        )
        .unwrap();
        assert_eq!(out.rows_affected, 1);
        let out = dispatch_with(
            &conn,
            &json!({"operation":"canonicalize","table":"facts"}),
            &two_dimension_map(),
        )
        .unwrap();
        assert_eq!(out.rows_affected, 1, "exactly one identity moved");
        assert_eq!(
            subject_row(&conn, "f2"),
            ("user:alpha".into(), Some("user".into())),
            "the written subject is untouched — that is what makes this revertible"
        );
        let axes: i64 = conn
            .query_row(
                "SELECT count(DISTINCT canonical_subject) FROM facts",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(axes, 1);
        // and the relation dimension is not collateral damage
        assert_eq!(
            row_of(&conn, "f2"),
            ("lives_in".into(), Some("lives_in".into()))
        );

        // revert: drop the alias row, re-derive — the axis splits again and no
        // fact was destroyed on the way
        dispatch_with(
            &conn,
            &json!({"operation":"delete","table":"subject_aliases",
                    "where":{"alias":"user:alpha"}}),
            &two_dimension_map(),
        )
        .unwrap();
        dispatch_with(
            &conn,
            &json!({"operation":"canonicalize","table":"facts","column":"subject"}),
            &two_dimension_map(),
        )
        .unwrap();
        assert_eq!(subject_row(&conn, "f2").1.as_deref(), Some("user:alpha"));
    }

    /// The alias table of a normalising binding is keyed on the NORMAL form, so
    /// one judgement covers every spelling of the alias — otherwise the GC would
    /// have to enumerate the variants it happened to see.
    #[test]
    fn an_alias_covers_every_spelling_that_normalises_onto_it() {
        let conn = two_dimension_fixture();
        dispatch_with(
            &conn,
            &json!({"operation":"set_alias","table":"facts","column":"subject",
                    "alias":"User:Alpha","canonical":"User"}),
            &two_dimension_map(),
        )
        .unwrap();
        mint_subject(&conn, "f1", "  USER:alpha  ");
        assert_eq!(
            subject_row(&conn, "f1").1.as_deref(),
            Some("user"),
            "the alias was written in one spelling and hits in another"
        );
    }

    /// A table with two dimensions cannot be told an alias without being told
    /// WHICH dimension — guessing would write a subject judgement into the
    /// predicate mapping.
    #[test]
    fn a_two_dimension_table_demands_the_column_for_set_alias() {
        let conn = two_dimension_fixture();
        let err = dispatch_with(
            &conn,
            &json!({"operation":"set_alias","table":"facts",
                    "alias":"user:alpha","canonical":"user"}),
            &two_dimension_map(),
        )
        .unwrap_err();
        assert!(err.contains("column"), "got: {err}");
        // an unknown dimension is a refusal, never a silent no-op
        let err = dispatch_with(
            &conn,
            &json!({"operation":"canonicalize","table":"facts","column":"claim"}),
            &two_dimension_map(),
        )
        .unwrap_err();
        assert!(err.contains("no canonical binding"), "got: {err}");
    }

    /// `canonicalize` without a `column` re-derives the whole row identity, which
    /// is what keeps P2's dream-run order (`set_alias` … then `canonicalize`)
    /// correct for a store that has since grown a second dimension.
    #[test]
    fn canonicalize_covers_every_dimension_of_the_table() {
        let conn = two_dimension_fixture();
        mint_subject(&conn, "f1", "user:alpha");
        for (col, alias, canonical) in [
            ("subject", "user:alpha", "user"),
            ("predicate", "lives_in", "resides_in"),
        ] {
            dispatch_with(
                &conn,
                &json!({"operation":"set_alias","table":"facts","column":col,
                        "alias":alias,"canonical":canonical}),
                &two_dimension_map(),
            )
            .unwrap();
        }
        let out = dispatch_with(
            &conn,
            &json!({"operation":"canonicalize","table":"facts"}),
            &two_dimension_map(),
        )
        .unwrap();
        assert_eq!(
            out.rows_affected, 2,
            "one row, two identities moved — the receipt counts dimensions"
        );
        assert_eq!(subject_row(&conn, "f1").1.as_deref(), Some("user"));
        assert_eq!(row_of(&conn, "f1").1.as_deref(), Some("resides_in"));
        // idempotent: a second run over unchanged data reports 0
        let out = dispatch_with(
            &conn,
            &json!({"operation":"canonicalize","table":"facts"}),
            &two_dimension_map(),
        )
        .unwrap();
        assert_eq!(out.rows_affected, 0);
    }

    fn candidates(conn: &rusqlite::Connection, args: Value) -> Vec<(String, String)> {
        let out = dispatch_with(conn, &args, &two_dimension_map()).unwrap();
        assert_eq!(out.operation, "alias_candidates");
        out.payload
            .as_array()
            .expect("candidate array")
            .iter()
            .map(|p| {
                (
                    p["left"].as_str().unwrap().to_string(),
                    p["right"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    }

    /// The candidate feed of ruling Q5: it FINDS the typo pair, and finding is
    /// all it does — the store is unchanged after the call.
    #[test]
    fn the_candidate_feed_reports_a_typo_pair_and_merges_nothing() {
        let conn = two_dimension_fixture();
        mint_subject(&conn, "f1", "elvese");
        mint_subject(&conn, "f2", "elvse");
        mint_subject(&conn, "f3", "helix");
        let got = candidates(
            &conn,
            json!({"operation":"alias_candidates","table":"facts","column":"subject"}),
        );
        assert_eq!(got, vec![("elvese".to_string(), "elvse".to_string())]);
        let axes: i64 = conn
            .query_row(
                "SELECT count(DISTINCT canonical_subject) FROM facts",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(axes, 3, "the feed proposes, it never merges");
        assert_eq!(
            conn.query_row::<i64, _, _>("SELECT count(*) FROM subject_aliases", [], |r| r.get(0))
                .unwrap(),
            0,
            "and it writes nothing at all"
        );
    }

    /// A pair the GC has already judged must not come back every night: once the
    /// alias exists both sides resolve onto one identity and the pair is gone.
    #[test]
    fn an_already_aliased_pair_is_not_proposed_again() {
        let conn = two_dimension_fixture();
        mint_subject(&conn, "f1", "elvese");
        mint_subject(&conn, "f2", "elvse");
        let args = json!({"operation":"alias_candidates","table":"facts","column":"subject"});
        assert_eq!(candidates(&conn, args.clone()).len(), 1);
        dispatch_with(
            &conn,
            &json!({"operation":"set_alias","table":"facts","column":"subject",
                    "alias":"elvse","canonical":"elvese"}),
            &two_dimension_map(),
        )
        .unwrap();
        assert!(
            candidates(&conn, args.clone()).is_empty(),
            "a settled pair stays settled — even BEFORE the re-derive has run"
        );
        dispatch_with(
            &conn,
            &json!({"operation":"canonicalize","table":"facts"}),
            &two_dimension_map(),
        )
        .unwrap();
        assert!(candidates(&conn, args).is_empty());
    }

    /// 0.2.0 P5: a NEGATIVE judgement has to survive the night too, or the GC
    /// pays a top-tier model to turn the same pair down again every run. The
    /// refusal is a row of its own — the alias table cannot carry it, its
    /// `canonical` column is NOT NULL and would also mean "resolves to nothing".
    #[test]
    fn a_rejected_pair_is_not_proposed_again() {
        let conn = two_dimension_fixture();
        mint_subject(&conn, "f1", "elvese");
        mint_subject(&conn, "f2", "elvse");
        let args = json!({"operation":"alias_candidates","table":"facts","column":"subject"});
        assert_eq!(candidates(&conn, args.clone()).len(), 1);

        // The pair is UNORDERED: the GC hands it over the way the feed served it,
        // and the answer must not depend on which side it names first.
        let out = dispatch_with(
            &conn,
            &json!({"operation":"reject_pair","table":"facts","column":"subject",
                    "left":"elvse","right":"elvese","recorded_at":"2026-08-12T03:00:00Z"}),
            &two_dimension_map(),
        )
        .unwrap();
        assert_eq!(out.operation, "reject_pair");
        assert_eq!(out.rows_affected, 1);
        assert!(
            candidates(&conn, args.clone()).is_empty(),
            "a pair the GC turned down is not a question any more"
        );

        // Re-judging is an upsert, not a second row — the same property that lets
        // the invariance gate re-run a whole dream run.
        dispatch_with(
            &conn,
            &json!({"operation":"reject_pair","table":"facts","column":"subject",
                    "left":"elvese","right":"elvse","recorded_at":"2026-08-13T03:00:00Z"}),
            &two_dimension_map(),
        )
        .unwrap();
        let rows: Vec<(String, String, String)> = conn
            .prepare("SELECT left_value, right_value, recorded_at FROM subject_rejected_pairs")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            rows,
            vec![(
                "elvese".to_string(),
                "elvse".to_string(),
                "2026-08-13T03:00:00Z".to_string()
            )],
            "stored in a fixed order, so the pair has ONE key"
        );

        // The revert path is the ordinary delete, exactly like the alias table's.
        dispatch_with(
            &conn,
            &json!({"operation":"delete","table":"subject_rejected_pairs",
                    "where":{"left_value":"elvese"}}),
            &two_dimension_map(),
        )
        .unwrap();
        assert_eq!(
            candidates(&conn, args).len(),
            1,
            "a deleted refusal makes the pair a question again"
        );
    }

    /// A refusal is a statement about the IDENTITIES the feed serves, so it is
    /// keyed the way those identities are: under a normalising binding, on the
    /// normal form. Otherwise the GC turns down `Elvese`/`elvse` and the feed
    /// keeps offering `elvese`/`elvse`.
    #[test]
    fn a_rejection_is_keyed_like_the_identity_it_speaks_about() {
        let conn = two_dimension_fixture();
        mint_subject(&conn, "f1", "elvese");
        mint_subject(&conn, "f2", "elvse");
        dispatch_with(
            &conn,
            &json!({"operation":"reject_pair","table":"facts","column":"subject",
                    "left":"  ELVESE ","right":"Elvse"}),
            &two_dimension_map(),
        )
        .unwrap();
        assert!(
            candidates(
                &conn,
                json!({"operation":"alias_candidates","table":"facts","column":"subject"})
            )
            .is_empty()
        );
    }

    /// The op is refused where it cannot mean anything, instead of writing into
    /// a table nobody declared.
    #[test]
    fn reject_pair_refuses_an_undeclared_log_and_a_missing_dimension() {
        let conn = two_dimension_fixture();
        let mut no_log = two_dimension_map();
        for spec in no_log.get_mut("facts").unwrap() {
            spec.rejected = None;
        }
        assert!(
            dispatch_with(
                &conn,
                &json!({"operation":"reject_pair","table":"facts","column":"subject",
                        "left":"a","right":"b"}),
                &no_log,
            )
            .is_err(),
            "without a declared table there is nowhere to remember it"
        );
        for bad in [
            // the dimension is mandatory on a two-binding table
            json!({"operation":"reject_pair","table":"facts","left":"a","right":"b"}),
            json!({"operation":"reject_pair","table":"facts","column":"subject","left":"a"}),
            json!({"operation":"reject_pair","table":"facts","column":"subject",
                   "left":"a","right":""}),
            // a value is not a candidate of itself
            json!({"operation":"reject_pair","table":"facts","column":"subject",
                   "left":"a","right":"a"}),
        ] {
            assert!(
                dispatch_with(&conn, &bad, &two_dimension_map()).is_err(),
                "accepted {bad}"
            );
        }
    }

    /// Deterministic output: best first, then alphabetical. The GC reads the head
    /// of this list, so two runs over one store must hand it the same head.
    #[test]
    fn the_candidate_feed_is_ordered_and_bounded() {
        let conn = two_dimension_fixture();
        for (i, name) in ["helix", "helox", "sonnenhof", "sonnehof", "zzz"]
            .iter()
            .enumerate()
        {
            mint_subject(&conn, &format!("f{i}"), name);
        }
        let all = candidates(
            &conn,
            json!({"operation":"alias_candidates","table":"facts","column":"subject",
                   "min_score":0.4}),
        );
        assert_eq!(
            all,
            vec![
                ("sonnehof".to_string(), "sonnenhof".to_string()),
                ("helix".to_string(), "helox".to_string()),
            ],
            "the longer pair shares more trigrams and therefore ranks first"
        );
        let capped = candidates(
            &conn,
            json!({"operation":"alias_candidates","table":"facts","column":"subject",
                   "min_score":0.4,"limit":1}),
        );
        assert_eq!(capped.len(), 1);
        assert_eq!(capped[0], all[0], "a limit cuts the tail, not the order");
        // a threshold nobody can reach yields an empty feed, not an error
        assert!(
            candidates(
                &conn,
                json!({"operation":"alias_candidates","table":"facts","column":"subject",
                       "min_score":0.99}),
            )
            .is_empty()
        );
    }

    /// Every bound is validated, because this op is the one that can be made
    /// expensive from the outside.
    #[test]
    fn the_candidate_feed_refuses_impossible_bounds() {
        let conn = two_dimension_fixture();
        for bad in [
            json!({"operation":"alias_candidates","table":"facts","column":"subject","limit":0}),
            json!({"operation":"alias_candidates","table":"facts","column":"subject",
                   "max_values":100000}),
            json!({"operation":"alias_candidates","table":"facts","column":"subject",
                   "min_score":2.0}),
            json!({"operation":"alias_candidates","table":"facts","column":"subject",
                   "min_score":"high"}),
            // the dimension is mandatory on a two-binding table
            json!({"operation":"alias_candidates","table":"facts"}),
        ] {
            assert!(
                dispatch_with(&conn, &bad, &two_dimension_map()).is_err(),
                "accepted {bad}"
            );
        }
    }

    #[test]
    fn a_mint_without_a_known_alias_derives_the_written_predicate() {
        // "otherwise identical to the predicate": with nothing to say
        // otherwise, the canonical value IS the original. No NULL, because all
        // three read legs consume the derived column and a NULL there is an
        // invisible fact.
        let conn = canon_fixture();
        assert_eq!(mint(&conn, "f1", "favorite_editor").rows_affected, 1);
        assert_eq!(
            row_of(&conn, "f1"),
            ("favorite_editor".into(), Some("favorite_editor".into()))
        );
    }

    #[test]
    fn a_mint_resolves_a_known_alias_and_leaves_the_original_alone() {
        let conn = canon_fixture();
        dispatch_with(
            &conn,
            &json!({"operation":"set_alias","table":"facts",
                    "alias":"Lieblingseditor","canonical":"favorite_editor"}),
            &canon_map(),
        )
        .unwrap();
        mint(&conn, "f1", "Lieblingseditor");
        assert_eq!(
            row_of(&conn, "f1"),
            ("Lieblingseditor".into(), Some("favorite_editor".into())),
            "the written spelling stays byte-identical next to the derived one"
        );
    }

    #[test]
    fn the_derived_column_is_store_owned_on_insert_and_update() {
        // A caller cannot write the identity of a row: whatever it puts in the
        // target column is replaced by what the alias table implies. Without this
        // rule one careless writer could make a fact answer under an identity its
        // own predicate does not carry.
        let conn = canon_fixture();
        dispatch_with(
            &conn,
            &json!({"operation":"insert","table":"facts",
                    "row":{"id":"f1","predicate":"favorite_editor",
                           "canonical_predicate":"something_else"}}),
            &canon_map(),
        )
        .unwrap();
        assert_eq!(row_of(&conn, "f1").1.as_deref(), Some("favorite_editor"));

        dispatch_with(
            &conn,
            &json!({"operation":"update","table":"facts",
                    "set":{"canonical_predicate":"hijacked"},"where":{"id":"f1"}}),
            &canon_map(),
        )
        .unwrap_err();

        // an update that MOVES the source moves the derived column with it
        dispatch_with(
            &conn,
            &json!({"operation":"update","table":"facts",
                    "set":{"predicate":"preferred_editor"},"where":{"id":"f1"}}),
            &canon_map(),
        )
        .unwrap();
        assert_eq!(
            row_of(&conn, "f1"),
            ("preferred_editor".into(), Some("preferred_editor".into()))
        );
    }

    #[test]
    fn set_alias_is_an_upsert_so_a_second_dream_run_is_a_no_op() {
        let conn = canon_fixture();
        let args = json!({"operation":"set_alias","table":"facts",
                          "alias":"Lieblingseditor","canonical":"favorite_editor",
                          "recorded_at":"2026-08-12T03:00:00Z"});
        assert_eq!(
            dispatch_with(&conn, &args, &canon_map())
                .unwrap()
                .rows_affected,
            1
        );
        dispatch_with(&conn, &args, &canon_map()).unwrap();
        // the GC may change its mind: the same alias points somewhere else, still one row
        dispatch_with(
            &conn,
            &json!({"operation":"set_alias","table":"facts",
                    "alias":"Lieblingseditor","canonical":"editor"}),
            &canon_map(),
        )
        .unwrap();
        let (n, canonical): (i64, String) = conn
            .query_row(
                "SELECT count(*), max(canonical) FROM predicate_aliases WHERE alias='Lieblingseditor'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(n, 1, "an alias is a mapping, never a log of judgements");
        assert_eq!(canonical, "editor");

        // shape guards
        assert!(
            dispatch_with(
                &conn,
                &json!({"operation":"set_alias","table":"facts","alias":"","canonical":"x"}),
                &canon_map()
            )
            .is_err()
        );
        assert!(
            dispatch_with(
                &conn,
                &json!({"operation":"set_alias","table":"episodes","alias":"a","canonical":"b"}),
                &canon_map()
            )
            .is_err(),
            "a table without a binding has no alias table to write to"
        );
    }

    #[test]
    fn canonicalize_re_derives_the_column_and_reverts_with_the_alias() {
        // The whole point of ruling Q3 in one test: aliases arrive AFTER the facts,
        // `canonicalize` pulls the column to them, and deleting the alias row plus a
        // second `canonicalize` puts every row back on its original — because the
        // original was never rewritten.
        let conn = canon_fixture();
        mint(&conn, "f1", "Lieblingseditor");
        mint(&conn, "f2", "favorite_editor");
        assert_eq!(row_of(&conn, "f1").1.as_deref(), Some("Lieblingseditor"));

        dispatch_with(
            &conn,
            &json!({"operation":"set_alias","table":"facts",
                    "alias":"Lieblingseditor","canonical":"favorite_editor"}),
            &canon_map(),
        )
        .unwrap();
        let out = dispatch_with(
            &conn,
            &json!({"operation":"canonicalize","table":"facts"}),
            &canon_map(),
        )
        .unwrap();
        assert_eq!(out.operation, "canonicalize");
        assert_eq!(out.rows_affected, 1, "only the row that MOVED is counted");
        assert_eq!(row_of(&conn, "f1").1.as_deref(), Some("favorite_editor"));

        // idempotent: the second run over unchanged data is the receipt for the
        // invariance gate
        assert_eq!(
            dispatch_with(
                &conn,
                &json!({"operation":"canonicalize","table":"facts"}),
                &canon_map()
            )
            .unwrap()
            .rows_affected,
            0
        );

        // revert: drop the judgement, re-derive
        dispatch_with(
            &conn,
            &json!({"operation":"delete","table":"predicate_aliases",
                    "where":{"alias":"Lieblingseditor"}}),
            &canon_map(),
        )
        .unwrap();
        assert_eq!(
            dispatch_with(
                &conn,
                &json!({"operation":"canonicalize","table":"facts"}),
                &canon_map()
            )
            .unwrap()
            .rows_affected,
            1
        );
        assert_eq!(
            row_of(&conn, "f1"),
            ("Lieblingseditor".into(), Some("Lieblingseditor".into())),
            "no truth was destroyed on the way out"
        );
    }

    #[test]
    fn canonicalize_needs_a_binding_and_a_migrated_table() {
        let conn = canon_fixture();
        assert!(
            dispatch_with(
                &conn,
                &json!({"operation":"canonicalize","table":"episodes"}),
                &canon_map()
            )
            .is_err(),
            "a table without a binding cannot be canonicalised"
        );
        // a cell.db whose column migration did not run answers with a named code,
        // not with a raw SQL failure
        let bare = rusqlite::Connection::open_in_memory().unwrap();
        bare.execute("CREATE TABLE facts (id TEXT, predicate TEXT)", [])
            .unwrap();
        let out = dispatch_with(
            &bare,
            &json!({"operation":"canonicalize","table":"facts"}),
            &canon_map(),
        )
        .unwrap();
        assert_eq!(out.error_code, Some("unknown_column"));
    }

    /// Without a binding every op behaves exactly as it did before P2 — that is
    /// what keeps the store a general-purpose cell rather than a memory-hive part.
    #[test]
    fn a_store_without_a_binding_is_untouched() {
        let conn = canon_fixture();
        dispatch(
            &conn,
            &json!({"operation":"insert","table":"facts",
                    "row":{"id":"f1","predicate":"favorite_editor"}}),
        )
        .unwrap();
        assert_eq!(row_of(&conn, "f1"), ("favorite_editor".into(), None));
    }

    #[test]
    fn insert_one_row() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE items (id INTEGER, name TEXT)", [])
            .unwrap();
        let args = json!({"operation":"insert","table":"items","row":{"id":1,"name":"alice"}});
        let outcome = dispatch(&conn, &args).unwrap();
        assert_eq!(outcome.rows_affected, 1);
        assert_eq!(outcome.error_code, None);
    }

    #[test]
    fn insert_into_nonexistent_table_returns_sql_error_outcome() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let args = json!({"operation":"insert","table":"missing","row":{"x":1}});
        let outcome = dispatch(&conn, &args).unwrap();
        assert_eq!(outcome.operation, "insert");
        assert_eq!(outcome.rows_affected, 0);
        // paket-7 D1: missing table now classifies as unknown_table (Zero-Drift-with-intent:
        // spec cell-types.md Z.71 always declared this code; now wired up).
        assert_eq!(outcome.error_code, Some("unknown_table"));
        assert!(outcome.error_text.is_some());
    }

    #[test]
    fn insert_constraint_violation_returns_constraint_violation_code() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE u (id INTEGER PRIMARY KEY)", [])
            .unwrap();
        conn.execute("INSERT INTO u VALUES (1)", []).unwrap();
        let args = json!({"operation":"insert","table":"u","row":{"id":1}});
        let outcome = dispatch(&conn, &args).unwrap();
        assert_eq!(outcome.error_code, Some("constraint_violation"));
    }

    #[test]
    fn select_returns_rows_as_json_array() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE items (id INTEGER, name TEXT)", [])
            .unwrap();
        conn.execute("INSERT INTO items VALUES (1, 'a'), (2, 'b')", [])
            .unwrap();
        let args = meclaw_core::serde_json::json!({
            "operation":"select","table":"items","columns":["id","name"]
        });
        let out = dispatch(&conn, &args).unwrap();
        assert_eq!(out.operation, "select");
        assert_eq!(out.error_code, None);
        let rows = out.payload.as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["id"], 1);
        assert_eq!(rows[0]["name"], "a");
    }

    #[test]
    fn select_with_where_filters_rows() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE items (id INTEGER, name TEXT)", [])
            .unwrap();
        conn.execute("INSERT INTO items VALUES (1, 'a'), (2, 'b')", [])
            .unwrap();
        let args = meclaw_core::serde_json::json!({
            "operation":"select","table":"items","columns":["id","name"],
            "where":{"id":2}
        });
        let out = dispatch(&conn, &args).unwrap();
        let rows = out.payload.as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["name"], "b");
    }

    // ── P3: operator where, catalog-validated identifiers ──

    #[test]
    fn select_with_operator_where_filters_rows() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE t (id INTEGER, ts TEXT)", [])
            .unwrap();
        conn.execute(
            "INSERT INTO t VALUES (1,'2026-01-01'),(2,'2026-06-01'),(3,NULL)",
            [],
        )
        .unwrap();
        let args = json!({"operation":"select","table":"t","columns":["id"],
                          "where":{"ts":{"or_null":{"gt":"2026-03-01"}}}});
        let out = dispatch(&conn, &args).unwrap();
        let ids: Vec<i64> = out
            .payload
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_i64().unwrap())
            .collect();
        assert_eq!(ids, vec![2, 3]);
    }

    #[test]
    fn select_with_unknown_column_classifies_unknown_column() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE t (id INTEGER)", []).unwrap();
        let args = json!({"operation":"select","table":"t","columns":["id"],"where":{"nope":1}});
        let out = dispatch(&conn, &args).unwrap();
        assert_eq!(out.error_code, Some("unknown_column"));
    }

    #[test]
    fn select_with_unknown_projection_column_classifies_unknown_column() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE t (id INTEGER)", []).unwrap();
        let args = json!({"operation":"select","table":"t","columns":["nope"]});
        let out = dispatch(&conn, &args).unwrap();
        assert_eq!(out.error_code, Some("unknown_column"));
    }

    /// R1 obligation: the catalog is read per op, never cached across messages —
    /// a table created by one op is usable by the very next one.
    #[test]
    fn table_created_by_an_op_is_selectable_in_the_next_op() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        dispatch(
            &conn,
            &json!({"operation":"create_table","table":"fresh",
                    "columns":{"id":"int","name":"text"}}),
        )
        .unwrap();
        dispatch(
            &conn,
            &json!({"operation":"insert","table":"fresh","row":{"id":1,"name":"a"}}),
        )
        .unwrap();
        let out = dispatch(
            &conn,
            &json!({"operation":"select","table":"fresh","columns":["id","name"],
                    "where":{"id":{"gte":1}}}),
        )
        .unwrap();
        assert_eq!(
            out.error_code, None,
            "no stale catalog may reject a fresh table"
        );
        assert_eq!(out.rows_affected, 1);
    }

    #[test]
    fn order_by_and_limit_render_and_apply() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE t (id INTEGER, ts TEXT)", [])
            .unwrap();
        conn.execute("INSERT INTO t VALUES (1,'a'),(2,'c'),(3,'b')", [])
            .unwrap();
        let args = json!({"operation":"select","table":"t","columns":["id"],
                          "order_by":[{"col":"ts","dir":"desc"}],"limit":2});
        let out = dispatch(&conn, &args).unwrap();
        let ids: Vec<i64> = out
            .payload
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_i64().unwrap())
            .collect();
        assert_eq!(ids, vec![2, 3]);
        assert_eq!(out.rows_affected, 2);
    }

    #[test]
    fn order_by_multi_column_and_unknown_column_classify() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE t (id INTEGER, g TEXT)", [])
            .unwrap();
        conn.execute("INSERT INTO t VALUES (1,'b'),(2,'a'),(3,'a')", [])
            .unwrap();
        let out = dispatch(
            &conn,
            &json!({"operation":"select","table":"t","columns":["id"],
                    "order_by":[{"col":"g","dir":"asc"},{"col":"id","dir":"desc"}]}),
        )
        .unwrap();
        let ids: Vec<i64> = out
            .payload
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_i64().unwrap())
            .collect();
        assert_eq!(ids, vec![3, 2, 1], "second term breaks the tie");

        let bad = dispatch(
            &conn,
            &json!({"operation":"select","table":"t","columns":["id"],
                    "order_by":[{"col":"nope"}]}),
        )
        .unwrap();
        assert_eq!(bad.error_code, Some("unknown_column"));
    }

    #[test]
    fn update_rows_with_where() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE items (id INTEGER, name TEXT)", [])
            .unwrap();
        conn.execute("INSERT INTO items VALUES (1,'a'),(2,'b')", [])
            .unwrap();
        let args = meclaw_core::serde_json::json!({
            "operation":"update","table":"items",
            "set":{"name":"x"},
            "where":{"id":1}
        });
        let out = dispatch(&conn, &args).unwrap();
        assert_eq!(out.operation, "update");
        assert_eq!(out.rows_affected, 1);
        assert_eq!(out.error_code, None);
    }

    #[test]
    fn delete_rows_with_where() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE items (id INTEGER, name TEXT)", [])
            .unwrap();
        conn.execute("INSERT INTO items VALUES (1,'a'),(2,'b')", [])
            .unwrap();
        let args = meclaw_core::serde_json::json!({
            "operation":"delete","table":"items","where":{"id":1}
        });
        let out = dispatch(&conn, &args).unwrap();
        assert_eq!(out.operation, "delete");
        assert_eq!(out.rows_affected, 1);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn create_table_creates_new_table() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let args = meclaw_core::serde_json::json!({
            "operation":"create_table","table":"events","columns":{"id":"int","payload":"json"}
        });
        let out = dispatch(&conn, &args).unwrap();
        assert_eq!(out.operation, "create_table");
        assert_eq!(out.error_code, None);
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='events'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    /// R9: `create_table` is the one op that formats a *new* identifier into
    /// DDL — the catalog cannot vet a name that does not exist yet, so a syntax
    /// gate does. Positive receipt: the neighbouring table survives.
    #[test]
    fn create_table_rejects_injection_shaped_identifiers() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE keep (id INTEGER)", []).unwrap();
        let args = json!({"operation":"create_table",
                          "table":"x\" (a); DROP TABLE keep; --","columns":{"a":"int"}});
        assert!(dispatch(&conn, &args).is_err(), "must reject, not execute");
        let bad_col = json!({"operation":"create_table","table":"ok",
                             "columns":{"a\" INTEGER); DROP TABLE keep; --":"int"}});
        assert!(dispatch(&conn, &bad_col).is_err());
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='keep'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "keep must survive");
    }

    #[test]
    fn create_table_rejects_reserved_identifier_shapes() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        for t in ["sqlite_secret", "facts_fts", "9lives", ""] {
            let args = json!({"operation":"create_table","table":t,"columns":{"a":"int"}});
            assert!(dispatch(&conn, &args).is_err(), "must reject {t:?}");
        }
    }

    #[test]
    fn create_table_is_idempotent() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let args = meclaw_core::serde_json::json!({
            "operation":"create_table","table":"events","columns":{"id":"int"}
        });
        let _ = dispatch(&conn, &args).unwrap();
        let out = dispatch(&conn, &args).unwrap();
        assert_eq!(out.error_code, None, "IF NOT EXISTS → second call OK");
    }

    // ── P3: search op (FTS5 + bm25) ──

    fn search_fixture() -> rusqlite::Connection {
        // Equipped the way the factory equips a store connection: since 0.2.0 P3
        // the FTS index declares the `meclaw_stem` tokenizer and cannot be
        // opened on a connection that does not know it.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::query::install_connection_extensions(&conn).unwrap();
        conn.execute("CREATE TABLE facts (id TEXT, claim TEXT, subject TEXT)", [])
            .unwrap();
        conn.execute(
            "INSERT INTO facts VALUES ('f1','keto keto keto','user:m'),\
             ('f2','keto once','user:m'),('f3','none of that','user:x')",
            [],
        )
        .unwrap();
        crate::store::ddl::apply_fts_ddl(
            &conn,
            &std::collections::BTreeMap::from([("facts".to_string(), vec!["claim".to_string()])]),
            &std::collections::BTreeMap::new(),
        )
        .unwrap();
        conn
    }

    #[test]
    fn search_returns_rows_with_bm25_rank_best_first() {
        let conn = search_fixture();
        let args = json!({"operation":"search","table":"facts","columns":["id","claim"],
                          "match":"keto"});
        let out = dispatch(&conn, &args).unwrap();
        assert_eq!(out.operation, "search");
        assert_eq!(out.error_code, None);
        let rows = out.payload.as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["id"], "f1", "denser match ranks first");
        assert!(rows[0]["rank"].as_f64().unwrap() <= rows[1]["rank"].as_f64().unwrap());
    }

    #[test]
    fn search_combines_where_order_by_and_limit() {
        let conn = search_fixture();
        let args = json!({"operation":"search","table":"facts","columns":["id"],
                          "match":"keto","where":{"subject":"user:m"},
                          "order_by":[{"col":"id","dir":"desc"}],"limit":1});
        let out = dispatch(&conn, &args).unwrap();
        let rows = out.payload.as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0]["id"], "f2",
            "order_by wins over rank, limit caps to one"
        );
    }

    #[test]
    fn search_where_filters_on_the_base_table() {
        let conn = search_fixture();
        let args = json!({"operation":"search","table":"facts","columns":["id"],
                          "match":"keto","where":{"subject":"user:x"}});
        let out = dispatch(&conn, &args).unwrap();
        assert_eq!(out.rows_affected, 0);
    }

    #[test]
    fn search_on_table_without_fts_index_is_unknown_table() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE facts (id TEXT, claim TEXT)", [])
            .unwrap();
        let args = json!({"operation":"search","table":"facts","columns":["id"],"match":"keto"});
        let out = dispatch(&conn, &args).unwrap();
        assert_eq!(out.error_code, Some("unknown_table"));
        assert!(
            out.error_text.unwrap().contains("facts_fts"),
            "must name the missing index"
        );
    }

    #[test]
    fn search_with_broken_match_syntax_is_a_regular_sql_error() {
        let conn = search_fixture();
        let args =
            json!({"operation":"search","table":"facts","columns":["id"],"match":"\"unbalanced"});
        let out = dispatch(&conn, &args).unwrap();
        assert_eq!(out.error_code, Some("sql_error"));
        assert_eq!(out.rows_affected, 0);
    }

    #[test]
    fn search_requires_columns_and_match() {
        let conn = search_fixture();
        assert!(
            dispatch(
                &conn,
                &json!({"operation":"search","table":"facts","match":"keto"})
            )
            .is_err()
        );
        assert!(
            dispatch(
                &conn,
                &json!({"operation":"search","table":"facts","columns":["id"]})
            )
            .is_err()
        );
        assert!(
            dispatch(
                &conn,
                &json!({"operation":"search","table":"facts","columns":[],"match":"keto"})
            )
            .is_err()
        );
    }

    // ── D1: new classify_sql_error codes (paket-7) ──

    #[test]
    fn select_from_missing_table_classifies_unknown_table() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let args = json!({"operation":"select","table":"nope","columns":["x"]});
        let outcome = dispatch(&conn, &args).unwrap();
        assert_eq!(outcome.error_code, Some("unknown_table"));
    }

    #[test]
    fn insert_unknown_column_classifies_unknown_column() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE t (a INTEGER)", []).unwrap();
        let args = json!({"operation":"insert","table":"t","row":{"nosuch":1}});
        let outcome = dispatch(&conn, &args).unwrap();
        assert_eq!(outcome.error_code, Some("unknown_column"));
    }

    #[test]
    fn type_mismatch_classifies_type_mismatch() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        // INTEGER PRIMARY KEY with a non-integer string → SQLITE_MISMATCH (ErrorCode::TypeMismatch).
        conn.execute("CREATE TABLE t (id INTEGER PRIMARY KEY)", [])
            .unwrap();
        let args = json!({"operation":"insert","table":"t","row":{"id":"not-an-int"}});
        let outcome = dispatch(&conn, &args).unwrap();
        assert_eq!(outcome.error_code, Some("type_mismatch"));
    }

    #[test]
    fn unrecognized_sql_error_falls_back_to_sql_error() {
        // Backstop invariant: anything not specifically classified stays sql_error.
        // Trigger BOGUS SQL directly against rusqlite → SQLITE_ERROR with a message
        // containing no known prefix → classify_sql_error must return "sql_error".
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let err = conn.execute("BOGUS SQL", []).unwrap_err();
        let (code, _) = classify_sql_error(&err);
        assert_eq!(code, "sql_error");
    }
}
