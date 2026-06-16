//! Phase-9 store ops: structured dispatch (no raw SQL from caller).
//! Args format: `tool_call.text` as JSON-object with `op`-field, analogous
//! to file/bash cells. Each op returns [`OpOutcome`] (rows_affected + payload
//! JSON for selects + error_code for SQL-errors).

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
    let col_list = cols
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(",");
    let placeholders = cols.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let stmt = format!("INSERT INTO \"{table}\" ({col_list}) VALUES ({placeholders})");
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
    let col_list = col_names
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(",");
    let (where_clause, where_vals) = build_where(args.get("where"))?;
    let stmt = format!("SELECT {col_list} FROM \"{table}\"{where_clause}");

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

/// Build a `WHERE col=? AND col=?` clause + bound values. `where`-arg is
/// optional and must be a flat JSON-object (`{col: value, ...}`); other
/// operators (>, <, IN) are deferred.
fn build_where(where_v: Option<&Value>) -> Result<(String, Vec<rusqlite::types::Value>), String> {
    let Some(w) = where_v else {
        return Ok((String::new(), Vec::new()));
    };
    let obj = w.as_object().ok_or("where must be JSON object")?;
    if obj.is_empty() {
        return Ok((String::new(), Vec::new()));
    }
    let mut clauses = Vec::new();
    let mut vals = Vec::new();
    for (col, v) in obj {
        clauses.push(format!("\"{col}\" = ?"));
        vals.push(json_to_sql_value(Some(v)));
    }
    Ok((format!(" WHERE {}", clauses.join(" AND ")), vals))
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
    let set_clause = set
        .keys()
        .map(|c| format!("\"{c}\" = ?"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut vals: Vec<rusqlite::types::Value> =
        set.values().map(|v| json_to_sql_value(Some(v))).collect();
    let (where_clause, where_vals) = build_where(args.get("where"))?;
    vals.extend(where_vals);
    let stmt = format!("UPDATE \"{table}\" SET {set_clause}{where_clause}");
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
    let mut cols = std::collections::BTreeMap::new();
    for (c, ty_v) in cols_obj {
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
    let (where_clause, where_vals) = build_where(args.get("where"))?;
    let stmt = format!("DELETE FROM \"{table}\"{where_clause}");
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
