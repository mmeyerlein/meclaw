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
    let obj = args.as_object().ok_or("args must be JSON object")?;
    // B.1: the input field is canonically `operation` (cell-types.md Z.65),
    // matching the `operation` output header — not `op`.
    let op = obj
        .get("operation")
        .and_then(|v| v.as_str())
        .ok_or("missing operation")?;
    match op {
        "insert" => op_insert(conn, obj),
        "select" => op_select(conn, obj),
        "update" => op_update(conn, obj),
        "delete" => op_delete(conn, obj),
        "create_table" => op_create_table(conn, obj),
        "search" => op_search(conn, obj),
        "traverse" => op_traverse(conn, obj),
        "similar" => op_similar(conn, obj),
        other => Err(format!("unknown op {other:?}")),
    }
}

fn op_insert(
    conn: &rusqlite::Connection,
    args: &meclaw_core::serde_json::Map<String, Value>,
) -> Result<OpOutcome, String> {
    let table = args
        .get("table")
        .and_then(|v| v.as_str())
        .ok_or("missing table")?;
    let row = args
        .get("row")
        .and_then(|v| v.as_object())
        .ok_or("missing row object")?;
    let cols: Vec<&String> = row.keys().collect();
    if cols.is_empty() {
        return Err("row must declare at least one column".into());
    }
    let cat = match Catalog::load(conn, table) {
        Ok(c) => c,
        Err(e) => return Ok(catalog_error_outcome("insert", e)),
    };
    let wanted: Vec<String> = cols.iter().map(|c| (*c).clone()).collect();
    let resolved = match resolve_columns(&cat, &wanted) {
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
    let vals: Vec<rusqlite::types::Value> = cols
        .iter()
        .map(|c| json_to_sql_value(row.get(c.as_str())))
        .collect();
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
    let set_cols: Vec<String> = set.keys().cloned().collect();
    let resolved = match resolve_columns(&cat, &set_cols) {
        Ok(c) => c,
        Err(e) => return Ok(catalog_error_outcome("update", e)),
    };
    let set_clause = resolved
        .iter()
        .map(|c| format!("\"{c}\" = ?"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut vals: Vec<rusqlite::types::Value> =
        set.values().map(|v| json_to_sql_value(Some(v))).collect();
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
        let conn = rusqlite::Connection::open_in_memory().unwrap();
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
