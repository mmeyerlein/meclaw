//! GH #253 — content transfer for a cell that has a `cell.db`: the inverse of
//! the JSONL seed loader, plus the half the loader cannot reach.
//!
//! ## Why this is not a cell-type feature
//!
//! The seed loader already works for every cell type and always has.
//! `mutation::stage::seed_cell_db_if_present` walks every `seed/*.jsonl`,
//! derives the table name from the file stem and hands each file to
//! `apply_seed_jsonl` — *"Generic JSONL-Seed-Loader"*, in its own words. It asks
//! the cell type exactly one question, and only since GH #398: a type that owns
//! its schema (`CellFactory::owns_schema`) creates and seeds its own database at
//! first spawn, because a seed header cannot describe a fixed schema. For every
//! other type the loader is what it always was. So the way **in** was never
//! per-type; only the way **out**
//! was missing, and it was missing for all eight types that carry a `cell.db`
//! (`harness`, `llm`, `mcp`, `proxy`, `store`, `subcolony`, `timer`,
//! `vault`).
//!
//! [`export_document`] is therefore written as the **inverse of
//! `apply_seed_jsonl`**: same table-per-file unit, same `{"schema": {…}}`
//! header, same `text`/`int`/`json` vocabulary, same one-row-per-line body.
//! Write a document's `schema` object as line 1 and one row per line after it
//! and the result **is** a `seed/<table>.jsonl` that the existing loader parses
//! without knowing this module exists.
//!
//! ## The half the loader cannot reach
//!
//! `seed_cell_db_if_present` runs during **staging**, into a `cell.db` that was
//! just created. An existing database opens as `Resumed` and the seed is inert —
//! not merged, not appended, not diffed; it is not read (`docs/config.md`: *"in
//! that moment and never again"*). There is no message, no operation and no
//! flag that makes a **running** cell load one. [`import_document`] is that
//! missing half, and it answers the three questions a seeder never had to:
//!
//! * **Collision on an existing key: the target wins, always.** An import never
//!   updates and never overwrites. A row the target already decided — including
//!   the participant set it was decided in front of — is not something a
//!   document from elsewhere may replace. The operation is a **merge**.
//! * **Additive, never replacing.** No delete, no update, no truncate-and-load.
//!   A replacing import is a different operation and would need the no-delete
//!   policy's blessing before it could exist.
//! * **A partial import is a STATE, not a failure.** Everything checkable is
//!   checked before the first write and the writes run in ONE transaction, so a
//!   part applies whole or is refused whole. A whole cell is many parts, and
//!   stopping between them leaves a prefix — which is safe precisely because
//!   re-applying is idempotent. The repair is "send it again"; there is no
//!   compensating action to get wrong.
//!
//! ## Why both halves run on the CELL's own connection
//!
//! An `import` writes through ordinary `INSERT` statements on the connection the
//! cell itself opened, which is what keeps a maintained index maintained: the
//! `store`'s FTS5 indexes are declared with the `meclaw_stem_v1` tokenizer,
//! which is registered per connection, and their triggers are
//! `AFTER INSERT/UPDATE/DELETE ON <table>`. A write from any other connection
//! fires a trigger that cannot resolve its tokenizer and fails — which is why
//! the substrate hands this module the live `DbConn` instead of opening the file
//! (`cell_task`), and why the database-isolation rule is never bent to make a
//! transfer work.
//!
//! ## Provenance
//!
//! Since 0.16.0 rows carry the participant set they were learned in front of
//! (`audience_set`, `channel`, `speaker`). A transfer that drops one launders
//! the row: an imported row whose audience did not survive may be told to
//! anyone. That is prevented **structurally, not by a list of names**: an export
//! projects every column of a table, and an import refuses any part whose
//! declared schema disagrees with the target by a single column, in either
//! direction. A column that cannot be dropped covers every table of every cell
//! type, where a name list would only cover the ones somebody wrote down.

use meclaw_core::serde_json::{Map, Value};
use std::collections::BTreeSet;

/// The document format [`export_document`] answers with and [`import_document`]
/// accepts.
///
/// A version, because a document outlives the code that wrote it: a reader that
/// does not know this string refuses instead of guessing.
pub const TRANSFER_FORMAT: &str = "meclaw-cell-export/1";

/// The `cell.db` tables the **substrate** owns, not the cell
/// (`persist::schema::CELL_DB_DDL`). They are excluded from a transfer and this
/// is a decision, not an omission:
///
/// * `last_input` is the last inbound message — restoring it into another cell
///   would hand it a message it never received;
/// * `meta` is the schema version, written by `setup_cell_db` on every open;
/// * `params` is the β runtime overlay, whose baseline is the `config.json` of
///   the receiving instance, not of the source.
///
/// `system` is deliberately **not** on this list. The `llm` cell's accumulated
/// `system.*` tree is content — `seed/system.jsonl` is a documented seed input
/// (GH #99), so it has to be a documented output too.
pub const SUBSTRATE_TABLES: &[&str] = &["last_input", "meta", "params"];

/// What one transfer call ended as.
///
/// Three cases rather than an error type: the substrate turns each into a
/// reply, and the classification of a SQLite failure belongs to whoever
/// documents an error-code vocabulary.
#[derive(Debug)]
pub enum TransferOutcome {
    /// The call ran. `payload` is the document (export) or the receipt (import).
    Done {
        /// Rows read (export) or rows written (import).
        rows_affected: i64,
        /// The JSON payload the caller emits.
        payload: Value,
    },
    /// Refused before anything changed, with a short machine-readable code and
    /// a reason a human can act on.
    Refused {
        /// `unknown_table`, `unknown_column` or `import_schema_drift`.
        code: &'static str,
        /// Why — naming the table and the columns involved.
        detail: String,
    },
    /// SQLite itself failed.
    Sql(rusqlite::Error),
}

/// The content tables of a `cell.db`, in name order.
///
/// Everything SQLite reports as a table, minus its own `sqlite_%` bookkeeping,
/// minus [`SUBSTRATE_TABLES`], minus every virtual table together with its
/// shadow tables. An FTS5 index is `<t>_fts` plus `<t>_fts_data`, `<t>_fts_idx`,
/// … — derived data a transfer must never carry, because the triggers rebuild it
/// from the base table on the way in, and because a connection without the
/// index's tokenizer cannot even open it.
///
/// The default is **every remaining table**, which is exactly what the seed
/// loader does in reverse: it writes whatever `seed/*.jsonl` names. This
/// function makes no per-type judgement and still makes none: it does not know
/// what cell it is looking at, and a silent per-table exclusion list here would
/// be invisible in the `config.json` of the cell it applies to.
///
/// The opt-out that GH #314 ruled therefore does not live here. It is a
/// **declaration** — `contract.transfer: "none"` (`meclaw_core::TransferPolicy`)
/// — and it is answered one layer up, in [`handle_transfer_slot`], for the whole
/// database rather than for a table: a cell either answers this seam or it does
/// not. Whichever cells reach this function have already said they travel.
pub fn content_tables(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<String>> {
    let mut st =
        conn.prepare("SELECT name, COALESCE(sql, '') FROM sqlite_master WHERE type = 'table'")?;
    let all: Vec<(String, String)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let virtuals: Vec<&str> = all
        .iter()
        .filter(|(_, sql)| {
            sql.trim_start()
                .to_ascii_uppercase()
                .starts_with("CREATE VIRTUAL TABLE")
        })
        .map(|(name, _)| name.as_str())
        .collect();
    let mut out: Vec<String> = all
        .iter()
        .map(|(name, _)| name.clone())
        .filter(|name| {
            !name.starts_with("sqlite_")
                && !SUBSTRATE_TABLES.contains(&name.as_str())
                && !virtuals
                    .iter()
                    .any(|v| name == v || name.starts_with(&format!("{v}_")))
        })
        .collect();
    out.sort();
    Ok(out)
}

/// One column as the SQLite catalog spells it.
struct Column {
    name: String,
    decl_type: String,
    pk: i64,
}

/// The columns of `table`, in storage order, straight from SQLite.
///
/// Every identifier that later reaches a statement comes from here — caller text
/// only ever arrives as a bound parameter or as a name matched against this
/// list. That is the same injection barrier the `store`'s query layer draws.
fn columns_of(conn: &rusqlite::Connection, table: &str) -> rusqlite::Result<Vec<Column>> {
    let mut st = conn.prepare("SELECT name, type, pk FROM pragma_table_info(?1) ORDER BY cid")?;
    st.query_map([table], |r| {
        Ok(Column {
            name: r.get(0)?,
            decl_type: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            pk: r.get(2)?,
        })
    })?
    .collect()
}

/// The table's own primary key, in key order.
///
/// Empty for every table a `store` declares in `params.schema` — that
/// declaration cannot express a key — and for every table the seed loader built,
/// because `apply_seed_jsonl`'s `CREATE TABLE` has no key either. Both
/// operations therefore also accept a `key` argument.
fn primary_key(cols: &[Column]) -> Vec<String> {
    let mut keyed: Vec<&Column> = cols.iter().filter(|c| c.pk > 0).collect();
    keyed.sort_by_key(|c| c.pk);
    keyed.into_iter().map(|c| c.name.clone()).collect()
}

/// The seed vocabulary a column's declaration maps onto.
///
/// `apply_seed_jsonl` maps the other way (`int` → `INTEGER`, `json`/`text`/
/// anything → `TEXT`), so `json` and `text` are indistinguishable once a table
/// exists. A round trip through this pair is therefore type-stable even though
/// it is not type-preserving, and the loader's own fallback (`_ => TEXT`) is what
/// makes that harmless.
fn seed_type(decl_type: &str) -> &'static str {
    match decl_type.to_ascii_uppercase().as_str() {
        "INTEGER" | "INT" => "int",
        _ => "text",
    }
}

/// Resolve a caller-supplied table name against the cell's content tables.
///
/// A table that exists but is substrate machinery is refused with the same code
/// as one that does not exist — a caller cannot address either — but the detail
/// says which of the two it is, because "that table is the substrate's" and
/// "that table is a typo" want different fixes.
fn resolve_table(conn: &rusqlite::Connection, table: &str) -> Result<String, TransferOutcome> {
    let tables = match content_tables(conn) {
        Ok(t) => t,
        Err(e) => return Err(TransferOutcome::Sql(e)),
    };
    if let Some(t) = tables.iter().find(|t| t.as_str() == table) {
        return Ok(t.clone());
    }
    let exists = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type IN ('table','view') AND name = ?1",
            [table],
            |_| Ok(()),
        )
        .is_ok();
    Err(TransferOutcome::Refused {
        code: "unknown_table",
        detail: if exists {
            format!(
                "{table:?} is not content — it belongs to the substrate or is a derived index, \
                 and is deliberately not transferable (transferable: {})",
                tables.join(", ")
            )
        } else {
            format!("no such table: {table}")
        },
    })
}

/// A caller-supplied `key`: the columns that make a row THE SAME row across two
/// cells. Every entry is matched against the catalog, so an unknown column is a
/// named refusal rather than a quoted identifier.
fn parse_key(
    args: &Map<String, Value>,
    cols: &[Column],
    op: &str,
) -> Result<Result<Vec<String>, TransferOutcome>, String> {
    let Some(v) = args.get("key") else {
        return Ok(Ok(Vec::new()));
    };
    let arr = v
        .as_array()
        .ok_or_else(|| format!("{op}: key must be an array of column names"))?;
    let mut out = Vec::new();
    for entry in arr {
        let name = entry
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("{op}: key entry must be a non-empty column name"))?;
        match cols.iter().find(|c| c.name == name) {
            Some(c) => out.push(c.name.clone()),
            None => {
                return Ok(Err(TransferOutcome::Refused {
                    code: "unknown_column",
                    detail: format!("no such column: {name}"),
                }));
            }
        }
    }
    Ok(Ok(out))
}

/// `export` — one content table as a document, or, without `table`, the
/// inventory of what this cell has to offer.
///
/// The document is `{format, table, key, schema, rows}` and its `schema` object
/// is a seed header: `{"schema": …}` on line 1, one row per line after it, and
/// the file is a `seed/<table>.jsonl` the existing loader reads. That is the
/// whole point of the shape — the birth path and the transfer path speak one
/// format, so "export the old cell, birth a new one from it" is a mechanical
/// operation instead of a script that has to understand the content.
///
/// Unbounded on purpose: a truncated part lies about being a table, and a limit
/// no reader can see is worse than a large message. Ordered by the key when
/// there is one, otherwise by every column, so the same content yields the same
/// document — a backup nobody can diff against yesterday's hides what changed.
///
/// An export is a **read**. It discloses exactly what a read of the same table
/// already discloses, so it is not bounded by a cell's write surface.
pub fn export_document(
    conn: &rusqlite::Connection,
    args: &Map<String, Value>,
) -> Result<TransferOutcome, String> {
    let Some(table_arg) = args.get("table") else {
        let tables = match content_tables(conn) {
            Ok(t) => t,
            Err(e) => return Ok(TransferOutcome::Sql(e)),
        };
        let mut payload = Map::new();
        payload.insert("format".into(), Value::from(TRANSFER_FORMAT));
        payload.insert(
            "tables".into(),
            Value::Array(tables.into_iter().map(Value::from).collect()),
        );
        return Ok(TransferOutcome::Done {
            rows_affected: 0,
            payload: Value::Object(payload),
        });
    };
    let table = table_arg.as_str().ok_or("export: table must be a string")?;
    let table = match resolve_table(conn, table) {
        Ok(t) => t,
        Err(refusal) => return Ok(refusal),
    };
    let cols = match columns_of(conn, &table) {
        Ok(c) => c,
        Err(e) => return Ok(TransferOutcome::Sql(e)),
    };
    if cols.is_empty() {
        return Err(format!("export: table {table:?} declares no columns"));
    }
    let key = match parse_key(args, &cols, "export")? {
        Ok(k) if !k.is_empty() => k,
        Ok(_) => primary_key(&cols),
        Err(refusal) => return Ok(refusal),
    };

    let names: Vec<String> = cols.iter().map(|c| c.name.clone()).collect();
    let order_cols = if key.is_empty() { &names } else { &key };
    let stmt = format!(
        "SELECT {} FROM \"{table}\" ORDER BY {}",
        names
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(","),
        order_cols
            .iter()
            .map(|c| format!("\"{c}\" ASC"))
            .collect::<Vec<_>>()
            .join(","),
    );
    let mut prepared = match conn.prepare(&stmt) {
        Ok(p) => p,
        Err(e) => return Ok(TransferOutcome::Sql(e)),
    };
    let mut rows = match prepared.query([]) {
        Ok(r) => r,
        Err(e) => return Ok(TransferOutcome::Sql(e)),
    };
    let mut out = Vec::new();
    loop {
        match rows.next() {
            Ok(Some(row)) => {
                let mut obj = Map::new();
                for (i, name) in names.iter().enumerate() {
                    obj.insert(name.clone(), sql_to_json(row, i));
                }
                out.push(Value::Object(obj));
            }
            Ok(None) => break,
            Err(e) => return Ok(TransferOutcome::Sql(e)),
        }
    }

    let mut schema = Map::new();
    for c in &cols {
        schema.insert(c.name.clone(), Value::from(seed_type(&c.decl_type)));
    }
    let mut payload = Map::new();
    payload.insert("format".into(), Value::from(TRANSFER_FORMAT));
    payload.insert("table".into(), Value::from(table));
    payload.insert(
        "key".into(),
        Value::Array(key.into_iter().map(Value::from).collect()),
    );
    payload.insert("schema".into(), Value::Object(schema));
    let count = out.len() as i64;
    payload.insert("rows".into(), Value::Array(out));
    Ok(TransferOutcome::Done {
        rows_affected: count,
        payload: Value::Object(payload),
    })
}

/// The value tuple one row is identified by, rendered so two rows can be
/// compared without asking the database twice.
fn key_signature(row: &Map<String, Value>, key: &[String]) -> String {
    key.iter()
        .map(|c| match row.get(c) {
            Some(Value::String(s)) => s.clone(),
            Some(other) => other.to_string(),
            None => String::new(),
        })
        .collect::<Vec<_>>()
        .join("\u{1e}")
}

/// `import` — merge one exported document into a **running** cell.
///
/// See the module docs for the three decisions this encodes. The gate that runs
/// before anything is written is column-set equality between the part's declared
/// schema and the target table, in **both** directions: a part that lost a
/// column dropped it in transit, and a part that carries one the target does not
/// have comes from a newer source, where growing a schema is a template change
/// rather than something an import may do silently. That one rule is what makes
/// `audience_set`, `channel` and `speaker` non-optional payload.
///
/// Values are written as they came. Re-deriving an identity is not this
/// operation's business — the `store`'s `canonicalize` is the documented repair
/// and re-derives every canonical column from the alias tables, which travel in
/// their own parts. The seed loader has always behaved exactly this way, and a
/// transfer that quietly decided differently would be a second mechanism.
pub fn import_document(
    conn: &rusqlite::Connection,
    args: &Map<String, Value>,
) -> Result<TransferOutcome, String> {
    let table = args
        .get("table")
        .and_then(|v| v.as_str())
        .ok_or("import: missing table")?;
    let part_schema = args
        .get("schema")
        .and_then(|v| v.as_object())
        .ok_or("import: missing schema object — a row list without a header is a guess")?;
    let rows = args
        .get("rows")
        .and_then(|v| v.as_array())
        .ok_or("import: missing rows array")?;

    let table = match resolve_table(conn, table) {
        Ok(t) => t,
        Err(refusal) => return Ok(refusal),
    };
    let cols = match columns_of(conn, &table) {
        Ok(c) => c,
        Err(e) => return Ok(TransferOutcome::Sql(e)),
    };

    // THE gate (#244, the 0.16.0 audience-gate contract). An audience that
    // is present but EMPTY travels as it stands — empty means invisible, which is
    // the honest fate of a row from before the gate, and inventing one would be
    // the laundering itself.
    let target_cols: BTreeSet<&str> = cols.iter().map(|c| c.name.as_str()).collect();
    let declared_cols: BTreeSet<&str> = part_schema.keys().map(String::as_str).collect();
    if target_cols != declared_cols {
        let missing: Vec<&str> = target_cols.difference(&declared_cols).copied().collect();
        let extra: Vec<&str> = declared_cols.difference(&target_cols).copied().collect();
        let mut parts = Vec::new();
        if !missing.is_empty() {
            parts.push(format!(
                "the part does not carry {} — a column that does not travel is a column that \
                 was dropped in transit, and provenance is never reconstructed",
                missing.join(", ")
            ));
        }
        if !extra.is_empty() {
            parts.push(format!(
                "the part declares {} which this cell does not have — the source is newer, and \
                 growing a schema is a template change, not something an import may do silently",
                extra.join(", ")
            ));
        }
        return Ok(TransferOutcome::Refused {
            code: "import_schema_drift",
            detail: format!("import refused for table {table:?}: {}", parts.join("; ")),
        });
    }

    let key = match parse_key(args, &cols, "import")? {
        Ok(k) if !k.is_empty() => k,
        Ok(_) => primary_key(&cols),
        Err(refusal) => return Ok(refusal),
    };
    if key.is_empty() {
        return Err(format!(
            "import: table {table:?} has no primary key, so the part must name the \"key\" \
             columns that identify a row — without one an import can only duplicate, and \
             re-applying the same document would stop being idempotent"
        ));
    }

    // Row shape, checked before the transaction opens: a row that cannot be
    // identified cannot be merged, and finding that out halfway would be a
    // refusal after a write.
    let mut objects: Vec<&Map<String, Value>> = Vec::with_capacity(rows.len());
    for (idx, row) in rows.iter().enumerate() {
        let obj = row
            .as_object()
            .ok_or_else(|| format!("import: row {} is not a JSON object", idx + 1))?;
        for k in &key {
            if obj.get(k).is_none_or(|v| v.is_null()) {
                return Err(format!(
                    "import: row {} carries no value for key column {k:?}",
                    idx + 1
                ));
            }
        }
        objects.push(obj);
    }

    let probe = format!(
        "SELECT 1 FROM \"{table}\" WHERE {} LIMIT 1",
        key.iter()
            .enumerate()
            .map(|(i, c)| format!("\"{c}\" = ?{}", i + 1))
            .collect::<Vec<_>>()
            .join(" AND ")
    );
    let names: Vec<String> = cols.iter().map(|c| c.name.clone()).collect();
    let insert = format!(
        "INSERT INTO \"{table}\" ({}) VALUES ({})",
        names
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(","),
        names.iter().map(|_| "?").collect::<Vec<_>>().join(",")
    );

    let tx = match conn.unchecked_transaction() {
        Ok(t) => t,
        Err(e) => return Ok(TransferOutcome::Sql(e)),
    };
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut written = 0i64;
    for row in &objects {
        if !seen.insert(key_signature(row, &key)) {
            continue;
        }
        let key_vals: Vec<rusqlite::types::Value> =
            key.iter().map(|c| json_to_sql(row.get(c))).collect();
        let bind: Vec<&dyn rusqlite::ToSql> =
            key_vals.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
        match conn.query_row(&probe, bind.as_slice(), |_| Ok(())) {
            // The target wins. Always, and without looking at what the document
            // would have said instead.
            Ok(()) => continue,
            Err(rusqlite::Error::QueryReturnedNoRows) => {}
            Err(e) => return Ok(TransferOutcome::Sql(e)),
        }
        let vals: Vec<rusqlite::types::Value> = names
            .iter()
            .map(|c| json_to_sql(row.get(c.as_str())))
            .collect();
        let bind: Vec<&dyn rusqlite::ToSql> =
            vals.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
        match conn.execute(&insert, bind.as_slice()) {
            Ok(n) => written += n as i64,
            // Dropping `tx` on the way out rolls the whole part back: whole, or
            // nothing.
            Err(e) => return Ok(TransferOutcome::Sql(e)),
        }
    }
    if let Err(e) = tx.commit() {
        return Ok(TransferOutcome::Sql(e));
    }

    let mut payload = Map::new();
    payload.insert("format".into(), Value::from(TRANSFER_FORMAT));
    payload.insert("table".into(), Value::from(table));
    payload.insert("rows_in_part".into(), Value::from(rows.len() as i64));
    payload.insert("rows_written".into(), Value::from(written));
    payload.insert(
        "rows_skipped".into(),
        Value::from(rows.len() as i64 - written),
    );
    Ok(TransferOutcome::Done {
        rows_affected: written,
        payload: Value::Object(payload),
    })
}

/// Dispatch one `transfer` body slot (`{"operation": "export"|"import", …}`).
///
/// `Err` is an args-level fault the caller reports as `invalid_input`; an
/// [`TransferOutcome::Refused`] is a refusal the caller reports with the code it
/// carries.
pub fn dispatch(
    conn: &rusqlite::Connection,
    args: &Map<String, Value>,
) -> Result<TransferOutcome, String> {
    match args.get("operation").and_then(|v| v.as_str()) {
        Some("export") => export_document(conn, args),
        Some("import") => import_document(conn, args),
        Some(other) => Err(format!(
            "transfer: unknown operation {other:?} (known: export, import)"
        )),
        None => Err("transfer: missing operation (export or import)".into()),
    }
}

/// One SQLite value as JSON.
fn sql_to_json(row: &rusqlite::Row, idx: usize) -> Value {
    use rusqlite::types::ValueRef;
    match row.get_ref_unwrap(idx) {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::from(i),
        ValueRef::Real(f) => Value::from(f),
        ValueRef::Text(b) => Value::from(String::from_utf8_lossy(b).to_string()),
        ValueRef::Blob(b) => Value::from(b.to_vec()),
    }
}

/// One JSON value as a bound SQLite value. Missing key or JSON null → SQL NULL;
/// objects and arrays as JSON text — the same mapping `apply_seed_jsonl` uses,
/// so a row survives a round trip through a seed file unchanged.
fn json_to_sql(v: Option<&Value>) -> rusqlite::types::Value {
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
        Some(other) => V::Text(meclaw_core::serde_json::to_string(other).unwrap_or_default()),
    }
}

// ---------------------------------------------------------------------------
// GH #555 — the file half: the slot writes and reads its own seed set.
// ---------------------------------------------------------------------------

/// The completion marker a finished `export … to:` leaves in the directory it
/// filled. Written LAST and through the same rename as the data files, so a
/// reader that watches this name never finds a directory that is still
/// filling.
pub const EXPORT_MARKER: &str = "export_final.json";

/// The subdirectory a transfer writes into and reads from, under whatever
/// `to`/`from` named. It is `seed/` because that is the name the birth path
/// already watches (`mutation::stage::seed_cell_db_if_present`): a directory
/// this slot filled can be handed to a staging without renaming anything.
const SEED_DIR: &str = "seed";

/// Where one transfer call touches the filesystem.
///
/// [`TransferSite::Message`] is the form that has always existed and stays the
/// default: no path named, no file written or read, the document travels as the
/// reply body. A `to` (export) or `from` (import) switches to
/// [`TransferSite::File`] — and NOTHING else does. A caller that names no path
/// cannot accidentally write one.
#[derive(Debug, PartialEq, Eq)]
pub enum TransferSite {
    /// The document travels as the reply body.
    Message,
    /// A directory RELATIVE to `params.transfer.base_path`. `""` and `"."` are
    /// the base itself.
    File {
        /// The relative path exactly as the caller wrote it — the string a
        /// repair takes, and the one the receipt names back.
        rel: String,
    },
}

/// Read `to`/`from` out of the arguments of one call.
///
/// `export` looks at `to`, `import` at `from`; each is optional, and a value
/// that is not a string is an args-level fault (`invalid_input`), never a
/// silent fall back to the message form.
fn transfer_site(args: &Map<String, Value>, operation: &str) -> Result<TransferSite, String> {
    let key = match operation {
        "export" => "to",
        "import" => "from",
        // An unknown operation has no file half; `dispatch` refuses it by name.
        _ => return Ok(TransferSite::Message),
    };
    match args.get(key) {
        None | Some(Value::Null) => Ok(TransferSite::Message),
        Some(Value::String(rel)) => Ok(TransferSite::File { rel: rel.clone() }),
        Some(other) => Err(format!(
            "{operation}: {key} must be a string naming a directory relative to \
             params.transfer.base_path, got: {other}"
        )),
    }
}

/// Does `rel` climb above the fence **lexically** — without asking the
/// filesystem anything?
///
/// A local copy of the rule `meclaw_cells::boundary::lexically_escapes` states
/// for the `file` cell, for the same reason [`json_to_sql`]'s sibling in
/// `mutation::stage` is a local copy: meclaw-colony MUST NOT import
/// meclaw-cells. The rule is a plain component walk — `.` is nothing, a name
/// descends, `..` ascends, and popping past the base is an escape even if a
/// later component would come back in.
///
/// Running BEFORE any filesystem call is the whole point (GH #107):
/// `canonicalize` fails `NotFound` on a missing target, so the other order
/// would let `../missing` answer one way and `../existing` another — a (weak)
/// existence oracle for the world outside the fence. With the pre-check every
/// escape attempt answers identically, whatever is or is not out there.
fn lexically_escapes(rel: &std::path::Path) -> bool {
    use std::path::Component;
    let mut depth: usize = 0;
    for c in rel.components() {
        match c {
            Component::CurDir => {}
            Component::Normal(_) => depth += 1,
            Component::ParentDir => {
                if depth == 0 {
                    return true;
                }
                depth -= 1;
            }
            Component::RootDir | Component::Prefix(_) => return true,
        }
    }
    false
}

/// Resolve one `to`/`from` against the cell's own fence.
///
/// The ONE boundary function of the file half, and the only place a caller
/// string becomes a path. `rel` is the path of the directory that will actually
/// be written or read — `<to>/seed`, not `<to>` — because a check that stops
/// one component short of the write is not a check. Four refusals, in this
/// order:
///
/// 1. **no fence** — this cell declared no `params.transfer.base_path`, so it
///    has nowhere of its own to write. It does not fall back to the cell's tree,
///    to a temp directory or to the working directory: every one of those would
///    be a second output channel no edge carries and no drain sees.
/// 2. **absolute** — a path outside the fence by construction.
/// 3. **lexical escape** — refused before the filesystem is touched at all.
/// 4. **symlink escape** — the deepest existing ancestor of the target is
///    canonicalised and must still lie under the canonical fence.
///
/// (1)–(3) answer `transfer_path_out_of_bounds`; a fence that cannot be opened
/// — missing, not a directory, unreadable — answers `transfer_io_error`,
/// because that is a fact about the filesystem and not about the caller's path.
fn resolve_transfer_dir(
    base: Option<&std::path::Path>,
    rel: &str,
) -> Result<std::path::PathBuf, TransferOutcome> {
    let out_of_bounds = |detail: String| TransferOutcome::Refused {
        code: "transfer_path_out_of_bounds",
        detail,
    };
    let Some(base) = base else {
        return Err(out_of_bounds(format!(
            "transfer refused: {rel:?} names a directory, but this cell declares no \
             params.transfer.base_path — a cell writes files only inside a fence it \
             declared for itself"
        )));
    };
    let rel_path = std::path::Path::new(rel);
    if rel_path.is_absolute() || lexically_escapes(rel_path) {
        return Err(out_of_bounds(format!(
            "transfer refused: {rel:?} does not stay inside this cell's \
             params.transfer.base_path — the path is refused by name, and the answer is \
             the same whether anything exists out there or not"
        )));
    }
    let canon_base = base.canonicalize().map_err(|e| TransferOutcome::Refused {
        code: "transfer_io_error",
        detail: format!(
            "transfer refused: params.transfer.base_path could not be opened ({e}) — \
             the fence is not created here, it is declared, and it has to exist by the \
             time a path is named"
        ),
    })?;
    let joined = canon_base.join(rel_path);
    // The target need not exist yet (an export creates it), so canonicalise the
    // deepest ancestor that does. The walk terminates: `canon_base` itself is
    // canonical and exists.
    let mut probe = joined.as_path();
    loop {
        match probe.canonicalize() {
            Ok(canon) => {
                if !canon.starts_with(&canon_base) {
                    return Err(out_of_bounds(format!(
                        "transfer refused: {rel:?} resolves outside this cell's \
                         params.transfer.base_path"
                    )));
                }
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => match probe.parent() {
                Some(parent) => probe = parent,
                None => break,
            },
            Err(e) => {
                return Err(TransferOutcome::Refused {
                    code: "transfer_io_error",
                    detail: format!("transfer refused: {rel:?} could not be resolved ({e})"),
                });
            }
        }
    }
    Ok(joined)
}

/// A table name that may become a file name: one plain component, no leading
/// dot.
///
/// SQLite will happily hold a table called `../evil` if somebody quoted it into
/// existence, and `<seed_dir>/<table>.jsonl` would then leave the fence without
/// any of the checks above ever seeing a caller string. The names this function
/// accepts are exactly the ones the seed loader can derive a table from, so the
/// rule costs nothing that was ever reachable.
fn plain_table_name(table: &str) -> bool {
    !table.is_empty()
        && !table.starts_with('.')
        && !table.contains('/')
        && !table.contains('\\')
        && !table.contains('\0')
}

/// Write one table as `<seed_dir>/<table>.jsonl` and return the row count.
///
/// The document form is the one [`export_document`] was written as the inverse
/// of, and the one `apply_seed_jsonl` reads: the `schema` object on line 1, one
/// row per line after it, a trailing newline. Keys are sorted — `serde_json`'s
/// `Map` is a `BTreeMap` here, which is the same ordering Python's
/// `sort_keys=True` produced for the interim sink. Byte identity with that sink
/// was never the goal (its separators carry spaces); format identity is, and
/// the round trip is proven against the loader itself.
///
/// Staged as `<table>.jsonl.part` in the SAME directory, flushed with
/// `sync_all`, then moved with one `rename(2)`: a reader of the name a seed
/// loader watches finds the old file or the new one, never half of one. The
/// `fsync` is deliberately stronger than the interim sink's — that one bought a
/// concurrent *reader*, and a substrate writer that a backup depends on owes
/// durability too. `.part` is invisible to the loader, which takes only
/// `*.jsonl`.
async fn write_seed_file(
    seed_dir: &std::path::Path,
    table: &str,
    schema: &Value,
    rows: &[Value],
) -> std::io::Result<i64> {
    let mut text =
        meclaw_core::serde_json::to_string(&meclaw_core::serde_json::json!({ "schema": schema }))
            .unwrap_or_else(|_| "{\"schema\":{}}".to_string());
    for row in rows {
        text.push('\n');
        text.push_str(&meclaw_core::serde_json::to_string(row).unwrap_or_else(|_| "{}".into()));
    }
    text.push('\n');
    write_whole(&seed_dir.join(format!("{table}.jsonl")), &text).await?;
    Ok(rows.len() as i64)
}

/// Put a document under the name a reader watches — whole, or not at all.
///
/// `<name>.part` in the same directory, `sync_all`, one `rename(2)`. A failure
/// can leave the `.part` standing; that is the same known limitation
/// `mutation::rename`'s `targeted_overwrite_config` documents, and it is
/// harmless because nothing reads a `.part`.
async fn write_whole(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    let part = path.with_extension(match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{ext}.part"),
        None => "part".to_string(),
    });
    let mut file = tokio::fs::File::create(&part).await?;
    tokio::io::AsyncWriteExt::write_all(&mut file, text.as_bytes()).await?;
    file.sync_all().await?;
    drop(file);
    tokio::fs::rename(&part, path).await
}

/// One `seed/<table>.jsonl` read back into the pair an `import` takes.
///
/// The reader half of [`write_seed_file`], and deliberately as strict as
/// `apply_seed_jsonl` is on the birth path: a file whose first line is not a
/// `{"schema": {…}}` object describes nothing, and a data line that is not a
/// JSON object cannot be a row. Both are `transfer_seed_malformed`, both name
/// the file, and the second names the LINE, because that is the part an
/// operator repairs. Blank lines are skipped, exactly as the loader skips them.
async fn read_seed_file(path: &std::path::Path) -> Result<(Value, Vec<Value>), TransferOutcome> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let malformed = |detail: String| TransferOutcome::Refused {
        code: "transfer_seed_malformed",
        detail,
    };
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| TransferOutcome::Refused {
            code: "transfer_io_error",
            detail: format!("transfer refused: seed/{name} could not be read ({e})"),
        })?;
    let mut lines = content.lines();
    let header_line = lines
        .next()
        .ok_or_else(|| malformed(format!("transfer refused: seed/{name} is empty")))?;
    let header: Value = meclaw_core::serde_json::from_str(header_line).map_err(|e| {
        malformed(format!(
            "transfer refused: seed/{name} line 1 does not parse ({e}) — line 1 is the \
             {{\"schema\": …}} header"
        ))
    })?;
    let schema = header
        .get("schema")
        .filter(|v| v.is_object())
        .ok_or_else(|| {
            malformed(format!(
                "transfer refused: seed/{name} line 1 carries no schema object — a row list \
                 without a header is a guess"
            ))
        })?
        .clone();
    let mut rows = Vec::new();
    for (idx, raw) in lines.enumerate() {
        if raw.trim().is_empty() {
            continue;
        }
        let row: Value = meclaw_core::serde_json::from_str(raw).map_err(|e| {
            malformed(format!(
                "transfer refused: seed/{name} line {}: {e}",
                idx + 2
            ))
        })?;
        if !row.is_object() {
            return Err(malformed(format!(
                "transfer refused: seed/{name} line {}: not an object",
                idx + 2
            )));
        }
        rows.push(row);
    }
    Ok((schema, rows))
}

/// The `*.jsonl` table names lying in `<dir>/seed`, in name order.
///
/// The marker is not one of them (it is `.json`, not `.jsonl`), and neither is
/// a `.part` left over from a failed write — the same extension rule the seed
/// loader applies, which is why a directory this slot filled can be handed to a
/// staging unchanged.
async fn seed_tables_in(dir: &std::path::Path) -> Result<Vec<String>, TransferOutcome> {
    let mut read = tokio::fs::read_dir(dir)
        .await
        .map_err(|e| TransferOutcome::Refused {
            code: "transfer_io_error",
            detail: format!(
                "transfer refused: {} could not be listed ({e})",
                dir.display()
            ),
        })?;
    let mut out = Vec::new();
    loop {
        let entry = read
            .next_entry()
            .await
            .map_err(|e| TransferOutcome::Refused {
                code: "transfer_io_error",
                detail: format!(
                    "transfer refused: {} could not be walked ({e})",
                    dir.display()
                ),
            })?;
        let Some(entry) = entry else { break };
        let path = entry.path();
        if path.extension().is_some_and(|x| x == "jsonl")
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        {
            out.push(stem.to_string());
        }
    }
    out.sort();
    Ok(out)
}

/// What one transfer call ended as, in the four fields the reply carries.
type SlotAnswer = (i64, Value, Option<&'static str>, Option<String>);

/// Turn one refusal into the reply fields, so every exit of the file half
/// speaks the same shape.
fn answer_from(outcome: TransferOutcome) -> SlotAnswer {
    match outcome {
        TransferOutcome::Done {
            rows_affected,
            payload,
        } => (rows_affected, payload, None, None),
        TransferOutcome::Refused { code, detail } => (0, Value::Null, Some(code), Some(detail)),
        TransferOutcome::Sql(e) => (0, Value::Null, Some("sql_error"), Some(e.to_string())),
    }
}

fn io_refusal(what: &str, e: &std::io::Error) -> SlotAnswer {
    (
        0,
        Value::Null,
        Some("transfer_io_error"),
        Some(format!("transfer refused: {what} ({e})")),
    )
}

/// GH #555 — the file half of one call: resolve the fence, then run the
/// operation against the directory behind it.
///
/// The filesystem work lives HERE and not inside [`dispatch`], which means
/// above the `db.call` boundary: `db.call` runs a `move` closure on the
/// database task, and a task that exists to own one SQLite connection is not
/// where a `rename(2)` belongs. Each table's SQL half is its own `db.call`; the
/// bytes are written between them.
async fn run_file_transfer(
    operation: &str,
    rel: &str,
    args: &Map<String, Value>,
    db: &mut crate::DbConn,
    base: Option<&std::path::Path>,
    own_path: &meclaw_core::Path,
) -> SlotAnswer {
    // The path the receipt names back: what the CALLER wrote, never the host
    // prefix. A receipt travels further than the fence does, and the fence is
    // in this cell's own config anyway (the same reason `refusal_name` trims a
    // staging path, GH #507).
    let named = match rel.trim_matches('/') {
        "" | "." => SEED_DIR.to_string(),
        r => format!("{r}/{SEED_DIR}"),
    };
    // The fence check has to see the directory the bytes actually land in, so
    // `seed/` is appended BEFORE the resolve and not after it. Resolving `rel`
    // alone and joining afterwards left one hole open: with `<fence>/<rel>/seed`
    // a symlink, `create_dir_all` follows it and every file plus the marker
    // lands outside a fence the check said was intact. The join is done on the
    // caller's string as written — `Path::join` keeps an absolute `rel`
    // absolute, so `/etc` stays `/etc/seed` and is still refused by name.
    let rel_seed = std::path::Path::new(rel).join(SEED_DIR);
    let seed_dir = match resolve_transfer_dir(base, &rel_seed.to_string_lossy()) {
        Ok(d) => d,
        Err(refusal) => return answer_from(refusal),
    };
    match operation {
        "export" => export_to_dir(args, db, &seed_dir, &named, own_path).await,
        "import" => import_from_dir(args, db, &seed_dir, &named).await,
        other => (
            0,
            Value::Null,
            Some("invalid_input"),
            Some(format!(
                "transfer: unknown operation {other:?} (known: export, import)"
            )),
        ),
    }
}

/// The tables one file-half call addresses, in the order it will touch them.
///
/// `table` names exactly one. `tables` names a list AND its order — which is
/// what a schema with insert-order dependencies needs, because name order is
/// not that order. Neither: `fallback`, which is the cell's whole content
/// inventory on the way out and the whole directory on the way in.
fn addressed_tables(
    args: &Map<String, Value>,
    fallback: Vec<String>,
) -> Result<Vec<String>, String> {
    if let Some(one) = args.get("table") {
        let name = one
            .as_str()
            .ok_or("transfer: table must be a string".to_string())?;
        return Ok(vec![name.to_string()]);
    }
    let Some(list) = args.get("tables") else {
        return Ok(fallback);
    };
    let arr = list
        .as_array()
        .ok_or("transfer: tables must be an array of table names".to_string())?;
    arr.iter()
        .map(|v| {
            v.as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .ok_or("transfer: tables entry must be a non-empty table name".to_string())
        })
        .collect()
}

/// `export … to:` — one `seed/<table>.jsonl` per table, then the marker.
async fn export_to_dir(
    args: &Map<String, Value>,
    db: &mut crate::DbConn,
    seed_dir: &std::path::Path,
    named: &str,
    own_path: &meclaw_core::Path,
) -> SlotAnswer {
    // Without `table` and without `tables`: every content table of this cell.
    // That is the form a whole-cell backup takes, and it is why an export walk
    // no longer needs a script that knows the schema.
    let inventory = if args.contains_key("table") || args.contains_key("tables") {
        Vec::new()
    } else {
        let probe = Map::from_iter([("operation".to_string(), Value::from("export"))]);
        match db.call(move |c| export_document(c, &probe)).await {
            Ok(TransferOutcome::Done { payload, .. }) => payload["tables"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            Ok(other) => return answer_from(other),
            Err(detail) => return (0, Value::Null, Some("invalid_input"), Some(detail)),
        }
    };
    let tables = match addressed_tables(args, inventory) {
        Ok(t) => t,
        Err(detail) => return (0, Value::Null, Some("invalid_input"), Some(detail)),
    };
    for table in &tables {
        if !plain_table_name(table) {
            return (
                0,
                Value::Null,
                Some("transfer_path_out_of_bounds"),
                Some(format!(
                    "transfer refused: {table:?} cannot be a file name, so it cannot leave \
                     as one — a seed file is <table>.jsonl and nothing else"
                )),
            );
        }
    }
    if let Err(e) = tokio::fs::create_dir_all(seed_dir).await {
        return io_refusal(&format!("{named} could not be created"), &e);
    }

    // A `key` belongs to ONE table's identity, so it only travels when the call
    // named one table. A whole-cell export uses each table's own primary key.
    let single_key = if tables.len() == 1 {
        args.get("key")
    } else {
        None
    };
    let mut rows = Map::new();
    let mut total = 0i64;
    for table in &tables {
        let mut call = Map::new();
        call.insert("operation".into(), Value::from("export"));
        call.insert("table".into(), Value::from(table.as_str()));
        if let Some(k) = single_key {
            call.insert("key".into(), k.clone());
        }
        let doc = match db.call(move |c| export_document(c, &call)).await {
            Ok(TransferOutcome::Done { payload, .. }) => payload,
            Ok(other) => return answer_from(other),
            Err(detail) => return (0, Value::Null, Some("invalid_input"), Some(detail)),
        };
        let empty = Vec::new();
        let table_rows = doc["rows"].as_array().unwrap_or(&empty);
        match write_seed_file(seed_dir, table, &doc["schema"], table_rows).await {
            Ok(n) => {
                total += n;
                rows.insert(table.clone(), Value::from(n));
            }
            Err(e) => {
                return io_refusal(&format!("{named}/{table}.jsonl could not be written"), &e);
            }
        }
    }

    // The marker LAST, and by the same rename: a reader that watches it never
    // sees a directory that is still filling.
    let mut marker = Map::new();
    marker.insert("format".into(), Value::from(TRANSFER_FORMAT));
    marker.insert("cell".into(), Value::from(own_path.as_str()));
    marker.insert("exported_at".into(), Value::from(unix_seconds()));
    marker.insert(
        "tables".into(),
        Value::Array(tables.iter().map(|t| Value::from(t.as_str())).collect()),
    );
    marker.insert("rows".into(), Value::Object(rows));
    let text = meclaw_core::serde_json::to_string(&Value::Object(marker.clone()))
        .unwrap_or_else(|_| "{}".into())
        + "\n";
    if let Err(e) = write_whole(&seed_dir.join(EXPORT_MARKER), &text).await {
        return io_refusal(&format!("{named}/{EXPORT_MARKER} could not be written"), &e);
    }

    // The receipt is the marker plus the one thing the marker cannot carry:
    // where the caller will find it.
    let mut payload = marker;
    payload.insert("seed_dir".into(), Value::from(named));
    (total, Value::Object(payload), None, None)
}

/// Seconds since the epoch — the same stamp the blob store writes, and for the
/// same reason: a marker needs an ordering, not a calendar.
fn unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

/// `import … from:` — every part of one directory, one transaction per table.
///
/// Every file is PARSED before the first one is applied. A malformed file in
/// the set therefore writes nothing at all, rather than leaving the prefix that
/// a per-file walk would: the parse is checkable ahead of time, so it is
/// checked ahead of time — the same discipline `import_document` applies inside
/// one part.
async fn import_from_dir(
    args: &Map<String, Value>,
    db: &mut crate::DbConn,
    seed_dir: &std::path::Path,
    named: &str,
) -> SlotAnswer {
    let walked = if args.contains_key("table") || args.contains_key("tables") {
        Vec::new()
    } else {
        match seed_tables_in(seed_dir).await {
            Ok(t) => t,
            Err(refusal) => return answer_from(refusal),
        }
    };
    let tables = match addressed_tables(args, walked) {
        Ok(t) => t,
        Err(detail) => return (0, Value::Null, Some("invalid_input"), Some(detail)),
    };

    let mut parts = Vec::with_capacity(tables.len());
    for table in &tables {
        if !plain_table_name(table) {
            return (
                0,
                Value::Null,
                Some("transfer_path_out_of_bounds"),
                Some(format!(
                    "transfer refused: {table:?} cannot be a file name, so there is no \
                     seed file it could name"
                )),
            );
        }
        match read_seed_file(&seed_dir.join(format!("{table}.jsonl"))).await {
            Ok((schema, rows)) => parts.push((table.clone(), schema, rows)),
            Err(refusal) => return answer_from(refusal),
        }
    }

    let single_key = if parts.len() == 1 {
        args.get("key")
    } else {
        None
    };
    let mut receipts = Vec::with_capacity(parts.len());
    let mut written = 0i64;
    let mut skipped = 0i64;
    for (table, schema, rows) in parts {
        let mut call = Map::new();
        call.insert("operation".into(), Value::from("import"));
        call.insert("table".into(), Value::from(table.as_str()));
        call.insert("schema".into(), schema);
        call.insert("rows".into(), Value::Array(rows));
        if let Some(k) = single_key {
            call.insert("key".into(), k.clone());
        }
        match db.call(move |c| import_document(c, &call)).await {
            Ok(TransferOutcome::Done { payload, .. }) => {
                written += payload["rows_written"].as_i64().unwrap_or(0);
                skipped += payload["rows_skipped"].as_i64().unwrap_or(0);
                let mut one = Map::new();
                one.insert("table".into(), Value::from(table.as_str()));
                for k in ["rows_in_part", "rows_written", "rows_skipped"] {
                    one.insert(k.into(), payload[k].clone());
                }
                receipts.push(Value::Object(one));
            }
            Ok(other) => return answer_from(other),
            Err(detail) => return (0, Value::Null, Some("invalid_input"), Some(detail)),
        }
    }

    let mut payload = Map::new();
    payload.insert("format".into(), Value::from(TRANSFER_FORMAT));
    payload.insert("seed_dir".into(), Value::from(named));
    payload.insert("tables".into(), Value::Array(receipts));
    payload.insert("rows_written".into(), Value::from(written));
    payload.insert("rows_skipped".into(), Value::from(skipped));
    (written, Value::Object(payload), None, None)
}

// ---------------------------------------------------------------------------
// The substrate seam: one body slot, every cell type with a `cell.db`.
// ---------------------------------------------------------------------------

/// The body slot that carries a transfer. Top-level, like the β `params` slot.
pub const TRANSFER_SLOT: &str = "transfer";

/// GH #260 — the transfer operations that CHANGE the cell's database.
///
/// `import` is the only one. `export` is a read: it gives up exactly what a read
/// of the same table gives up anyway, and no write surface has ever bounded a
/// read — the same sentence `store`'s own `WRITE_OPS` list says one layer down.
/// An unknown operation is not on this list either, and does not need to be: it
/// is refused as `invalid_input` before it can touch anything.
const TRANSFER_WRITE_OPS: &[&str] = &["import"];

/// The scope that owns a cell: its own parent path.
///
/// `/main/memory/store` is owned by `/main/memory` — the hive it sits in. A cell
/// directly under the colony root gets `/`, which contains every cell, so the
/// declaration is inert there. Documented rather than special-cased, because at
/// the root there is no enclosing hive a boundary could follow. Deliberately the
/// same rule as `store`'s cell-level `owner_scope`, so a cell that declares both
/// halves gets ONE boundary, not two that disagree.
fn owner_scope(own_path: &meclaw_core::Path) -> String {
    let s = own_path.as_str();
    match s.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(i) => s[..i].to_string(),
    }
}

/// True iff `sender` lies inside `scope` (equal to it or below it).
fn within_scope(scope: &str, sender: &meclaw_core::Path) -> bool {
    let s = sender.as_str();
    if scope == "/" {
        return true;
    }
    s == scope || s.starts_with(&format!("{scope}/"))
}

/// GH #260 — refuse a substrate-answered WRITE from outside the owning scope.
///
/// A **provenance** rule, not a content rule: the only two things it looks at
/// are where this cell sits and who sent the message. It says nothing about what
/// the document contains, and it is the whole reason the rule can live above
/// every cell type — the substrate needs no `params`, no schema and no type
/// knowledge to answer it.
///
/// Fail-closed on an unknown sender, exactly as the cell-level rule is: a
/// message without `reply_to` is a source message (an HTTP ingress, an event)
/// that no edge inside the scope produced, so it is outside by definition.
///
/// Returns `Some(detail)` when the transfer must be refused.
fn write_surface_violation(
    write_surface: meclaw_core::WriteSurface,
    own_path: &meclaw_core::Path,
    sender: Option<&meclaw_core::Path>,
    operation: &str,
) -> Option<String> {
    if write_surface != meclaw_core::WriteSurface::Internal {
        return None;
    }
    if !TRANSFER_WRITE_OPS.contains(&operation) {
        return None;
    }
    let scope = owner_scope(own_path);
    match sender {
        Some(p) if within_scope(&scope, p) => None,
        Some(p) => Some(format!(
            "transfer '{operation}' refused: this cell declares \
             contract.write_surface \"internal\", so only senders inside '{scope}' may write — \
             '{p}' is outside it. An export is a read and is unaffected.",
            p = p.as_str()
        )),
        None => Some(format!(
            "transfer '{operation}' refused: this cell declares \
             contract.write_surface \"internal\", so only senders inside '{scope}' may write — \
             this message carries no sender. An export is a read and is unaffected."
        )),
    }
}

/// Handle a `transfer` body slot before the cell's own `handle()` runs, and
/// report whether it did.
///
/// This is the seam that makes the operation type-agnostic in the same sense
/// the seed loader already is: it lives in `cell_task`, above every cell type,
/// and it runs against the cell's **own** `DbConn` — the connection the cell
/// opened, with whatever extensions that cell registered on it. That is not a
/// convenience: an FTS5 index declared with `meclaw_stem_v1` can only be written
/// through a connection that has the tokenizer, so a transfer executed anywhere
/// else would fail on exactly the stores that need it most.
///
/// A message carrying this slot never reaches `handle()`. That is deliberate:
/// no cell type has to know the operation exists, and none can accidentally
/// shadow it with a slot of its own.
///
/// `bounds` are the cell's own three declarations about this seam
/// (`contract.write_surface`, GH #260; `contract.transfer`, GH #314;
/// `params.transfer.base_path`, GH #555). They are the reason the position
/// above `handle()` costs no boundary: the first bounds WHO may write, the
/// second whether this cell's database answers the seam at all, and the third
/// WHERE this cell's own files live — taken by reference because a path is not
/// a machine word.
pub(crate) async fn handle_transfer_slot(
    msg: &meclaw_core::Message,
    sink: &meclaw_core::OutputSink,
    db: &mut crate::DbConn,
    bounds: &meclaw_core::TransferBounds,
) -> bool {
    let meclaw_core::Body::Inline(body) = &msg.body else {
        return false;
    };
    let Some(slot) = body.get(TRANSFER_SLOT) else {
        return false;
    };
    let started = std::time::Instant::now();
    let reply_to = msg.reply_to.clone();
    let target = reply_to.clone().unwrap_or_else(|| msg.target.clone());

    // GH #314: the exemption is answered FIRST — before the arguments are even
    // read, and therefore before anything could name a table. A refusal that
    // says "unknown_table" for `vault_secrets` and something else for
    // `vault_audit` is an inventory; this one says the same sentence to every
    // question. It covers export and import alike: a store that cannot leave
    // also cannot be overwritten through the same seam.
    if bounds.is_exempt() {
        let operation = slot
            .as_object()
            .and_then(|a| a.get("operation"))
            .and_then(|v| v.as_str())
            .unwrap_or("transfer")
            .to_string();
        emit_transfer_reply(
            sink,
            &target,
            reply_to.is_some(),
            &operation,
            0,
            Value::Null,
            Some("transfer_exempt"),
            Some(format!(
                "transfer '{operation}' refused: this cell declares \
                 contract.transfer \"none\" — its database is exempt from the transfer slot, \
                 export and import alike. Nothing was read and nothing was written."
            )),
            started,
        )
        .await;
        return true;
    }

    let Some(args) = slot.as_object().cloned() else {
        emit_transfer_reply(
            sink,
            &target,
            reply_to.is_some(),
            "transfer",
            0,
            Value::Null,
            Some("invalid_input"),
            Some("transfer slot must be a JSON object".to_string()),
            started,
        )
        .await;
        return true;
    };
    let operation: String = args
        .get("operation")
        .and_then(|v| v.as_str())
        .unwrap_or("transfer")
        .to_string();
    // GH #260: the substrate half of `write_surface`, checked between the args
    // and the dispatch — after the operation name is real, and BEFORE the DB
    // task starts, so a refused import touches nothing. `store`'s cell-level
    // check cannot reach here: a transfer never gets to `handle()`.
    if let Some(detail) = write_surface_violation(
        bounds.write_surface,
        sink.sender_path(),
        reply_to.as_ref(),
        &operation,
    ) {
        emit_transfer_reply(
            sink,
            &target,
            reply_to.is_some(),
            &operation,
            0,
            Value::Null,
            Some("write_denied"),
            Some(detail),
            started,
        )
        .await;
        return true;
    }
    // GH #555: `to`/`from` decide WHERE the document goes, and nothing else
    // does. A call that names no path takes the path it always took — the
    // message form is the default, not a fallback.
    let site = match transfer_site(&args, &operation) {
        Ok(s) => s,
        Err(detail) => {
            emit_transfer_reply(
                sink,
                &target,
                reply_to.is_some(),
                &operation,
                0,
                Value::Null,
                Some("invalid_input"),
                Some(detail),
                started,
            )
            .await;
            return true;
        }
    };
    let (rows, payload, code, text) = match site {
        TransferSite::Message => match db.call(move |c| dispatch(c, &args)).await {
            Ok(outcome) => answer_from(outcome),
            Err(detail) => (0, Value::Null, Some("invalid_input"), Some(detail)),
        },
        TransferSite::File { rel } => {
            run_file_transfer(
                &operation,
                &rel,
                &args,
                db,
                bounds.base_path.as_deref(),
                sink.sender_path(),
            )
            .await
        }
    };
    emit_transfer_reply(
        sink,
        &target,
        reply_to.is_some(),
        &operation,
        rows,
        payload,
        code,
        text,
        started,
    )
    .await;
    true
}

/// The single reply a transfer produces, in the universal body format.
///
/// The header is the same three fields a `store` op answers with (`operation`,
/// `rows_affected`, `duration_ms`, plus `error_code` when something was
/// refused), so a topology reads a transfer receipt the way it reads any other
/// receipt.
#[allow(clippy::too_many_arguments)]
async fn emit_transfer_reply(
    sink: &meclaw_core::OutputSink,
    target: &meclaw_core::Path,
    direct: bool,
    operation: &str,
    rows_affected: i64,
    payload: Value,
    error_code: Option<&str>,
    error_text: Option<String>,
    started: std::time::Instant,
) {
    let mut header = Map::new();
    header.insert("operation".into(), Value::from(operation));
    header.insert("rows_affected".into(), Value::from(rows_affected));
    header.insert(
        "duration_ms".into(),
        Value::from(started.elapsed().as_millis() as i64),
    );
    if let Some(code) = error_code {
        header.insert("error_code".into(), Value::from(code));
    }
    let text = match &error_text {
        Some(t) => t.clone(),
        None => meclaw_core::serde_json::to_string(&payload).unwrap_or_else(|_| "null".into()),
    };
    let content = meclaw_core::serde_json::json!({
        "header": header,
        "messages": [{"origin": "tool", "type": "tool_result", "text": text, "id": ""}]
    });
    let out = meclaw_core::CellOutput {
        target: target.clone(),
        content,
    };
    let _ = if direct {
        sink.push_direct_reply(out).await
    } else {
        sink.push(out).await
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    /// THE symmetry claim, and the reason this module is written as an inverse
    /// rather than as a new mechanism: an exported document, written out with
    /// its `schema` object as line 1 and one row per line after it, is a
    /// `seed/<table>.jsonl` that the EXISTING loader
    /// (`mutation::stage::seed_cell_db_if_present` → `apply_seed_jsonl`) reads
    /// without knowing this module exists.
    #[test]
    fn exported_document_is_a_seed_file_the_existing_loader_reads() {
        let source = tempfile::TempDir::new().unwrap();
        let conn = crate::persist::open_or_create_cell_db(&source.path().join("cell.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE notes (id TEXT PRIMARY KEY, body TEXT, n INTEGER);
             INSERT INTO notes VALUES ('a', 'first', 1), ('b', 'second', 2);",
        )
        .unwrap();
        let args = json!({"operation": "export", "table": "notes"});
        let TransferOutcome::Done { payload: doc, .. } =
            dispatch(&conn, args.as_object().unwrap()).unwrap()
        else {
            panic!("export must succeed");
        };

        let mut jsonl =
            meclaw_core::serde_json::to_string(&json!({"schema": doc["schema"]})).unwrap();
        for row in doc["rows"].as_array().unwrap() {
            jsonl.push('\n');
            jsonl.push_str(&meclaw_core::serde_json::to_string(row).unwrap());
        }
        let born = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(born.path().join("seed")).unwrap();
        std::fs::write(born.path().join("seed/notes.jsonl"), jsonl).unwrap();
        crate::mutation::stage::seed_cell_db_if_present(born.path(), "store", &Default::default())
            .expect("the existing seed loader must read an exported document");

        let reborn = rusqlite::Connection::open(born.path().join("cell.db")).unwrap();
        let args = json!({"operation": "export", "table": "notes", "key": ["id"]});
        let TransferOutcome::Done { payload: there, .. } =
            dispatch(&reborn, args.as_object().unwrap()).unwrap()
        else {
            panic!("export must succeed");
        };
        assert_eq!(
            there["rows"], doc["rows"],
            "what left the source is what the seeded cell holds"
        );
    }

    /// The other half of the symmetry: an FTS5 index and its shadow tables are
    /// derived data. They must never appear as content — a transfer carrying
    /// them would move an index a receiving cell rebuilds from its own rows.
    #[test]
    fn a_virtual_table_and_its_shadows_are_never_content() {
        let td = tempfile::TempDir::new().unwrap();
        let conn = crate::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE facts (id TEXT PRIMARY KEY, claim TEXT);
             CREATE VIRTUAL TABLE facts_fts USING fts5(claim, content='facts');",
        )
        .unwrap();
        let tables = content_tables(&conn).unwrap();
        assert_eq!(tables, vec!["facts".to_string(), "system".to_string()]);
    }

    /// The substrate seam itself: a `transfer` body slot reaching a **running**
    /// cell is answered against that cell's own `DbConn`, and the answer is one
    /// ordinary reply with the same header fields every other receipt carries.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_transfer_slot_is_answered_by_the_substrate_against_the_live_db() {
        let td = tempfile::TempDir::new().unwrap();
        let conn = crate::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE notes (id TEXT PRIMARY KEY, body TEXT);
             INSERT INTO notes VALUES ('a', 'first');",
        )
        .unwrap();
        let mut db = crate::DbConn::wrap(conn, None);

        let (otx, mut orx) = tokio::sync::mpsc::channel(8);
        let sink = meclaw_core::OutputSink::new(
            otx,
            meclaw_core::Path::new("/cell"),
            meclaw_core::Uuid::now_v7(),
            meclaw_core::Uuid::now_v7(),
            64,
            meclaw_core::Headers::new(),
            Some(meclaw_core::Path::new("/caller")),
        );
        let msg = meclaw_core::MessageBuilder::new(meclaw_core::Path::new("/cell"))
            .body(meclaw_core::Body::Inline(
                json!({"transfer": {"operation": "import", "table": "notes",
                                    "schema": {"id": "text", "body": "text"},
                                    "rows": [{"id": "a", "body": "the document disagrees"},
                                             {"id": "b", "body": "new here"}]}}),
            ))
            .reply_to(meclaw_core::Path::new("/caller"))
            .build();

        assert!(
            handle_transfer_slot(
                &msg,
                &sink,
                &mut db,
                &meclaw_core::TransferBounds::default()
            )
            .await,
            "a message carrying the slot is answered here and never reaches handle()"
        );
        drop(sink);

        let em = orx.recv().await.expect("the substrate must reply");
        assert_eq!(em.content["header"]["operation"], "import");
        assert_eq!(em.content["header"]["rows_affected"], 1);
        assert!(em.content["header"].get("error_code").is_none());

        let body: String = db
            .call(|c| {
                c.query_row("SELECT body FROM notes WHERE id = 'a'", [], |r| r.get(0))
                    .unwrap()
            })
            .await;
        assert_eq!(body, "first", "the target wins, on the live path too");
    }

    // ------------------------------------------------- GH #260, the write half

    /// One live cell at `path`, one `notes` table, one transfer slot from
    /// `sender`. Returns the substrate's reply and how many rows the table
    /// holds afterwards — a refusal has to be provable by BOTH.
    async fn transfer_from(
        path: &str,
        sender: Option<&str>,
        bounds: meclaw_core::TransferBounds,
        slot: Value,
    ) -> (Value, i64) {
        let td = tempfile::TempDir::new().unwrap();
        let conn = crate::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        conn.execute_batch("CREATE TABLE notes (id TEXT PRIMARY KEY, body TEXT);")
            .unwrap();
        let mut db = crate::DbConn::wrap(conn, None);

        let (otx, mut orx) = tokio::sync::mpsc::channel(8);
        let sink = meclaw_core::OutputSink::new(
            otx,
            meclaw_core::Path::new(path),
            meclaw_core::Uuid::now_v7(),
            meclaw_core::Uuid::now_v7(),
            64,
            meclaw_core::Headers::new(),
            sender.map(meclaw_core::Path::new),
        );
        let mut b = meclaw_core::MessageBuilder::new(meclaw_core::Path::new(path))
            .body(meclaw_core::Body::Inline(json!({ "transfer": slot })));
        if let Some(s) = sender {
            b = b.reply_to(meclaw_core::Path::new(s));
        }
        let msg = b.build();

        assert!(
            handle_transfer_slot(&msg, &sink, &mut db, &bounds).await,
            "a message carrying the slot is always consumed here, refused or not"
        );
        drop(sink);
        let reply = orx.recv().await.expect("the substrate must reply").content;
        let rows: i64 = db
            .call(|c| {
                c.query_row("SELECT count(*) FROM notes", [], |r| r.get(0))
                    .unwrap()
            })
            .await;
        (reply, rows)
    }

    /// A cell that declares `contract.write_surface: "internal"` and nothing
    /// else — the GH #260 half.
    fn sealed() -> meclaw_core::TransferBounds {
        meclaw_core::TransferBounds {
            write_surface: meclaw_core::WriteSurface::Internal,
            policy: meclaw_core::TransferPolicy::All,
            base_path: None,
        }
    }

    fn one_note() -> Value {
        json!({"operation": "import", "table": "notes",
               "schema": {"id": "text", "body": "text"},
               "rows": [{"id": "a", "body": "smuggled"}]})
    }

    /// GH #260 — the gap this closes. `write_surface: "internal"` is a promise
    /// about who may write; the transfer slot is answered above every cell type
    /// and used to walk straight past it. A refusal has to be provable on the
    /// database, not just in the reply, so the row count is asserted too.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_import_from_outside_the_owning_scope_is_refused_and_writes_nothing() {
        let (reply, rows) = transfer_from(
            "/main/memory/store",
            Some("/main/intruder"),
            sealed(),
            one_note(),
        )
        .await;
        assert_eq!(reply["header"]["error_code"], "write_denied");
        assert_eq!(reply["header"]["rows_affected"], 0);
        assert_eq!(rows, 0, "a refused import must leave the table untouched");
        let text = reply["messages"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("/main/memory") && text.contains("/main/intruder"),
            "the refusal must name the scope and the sender: {text}"
        );
    }

    /// The more important half: a boundary that refuses too much is as broken as
    /// one that refuses too little. A sender inside the owning scope — the hive
    /// the cell sits in — writes exactly as before.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_import_from_inside_the_owning_scope_still_lands() {
        let (reply, rows) = transfer_from(
            "/main/memory/store",
            Some("/main/memory/curator"),
            sealed(),
            one_note(),
        )
        .await;
        assert!(
            reply["header"].get("error_code").is_none(),
            "an inside sender is not what this bounds: {reply}"
        );
        assert_eq!(reply["header"]["rows_affected"], 1);
        assert_eq!(rows, 1);
    }

    /// An export is a read and no write surface bounds it — the same sentence
    /// `store`'s own `WRITE_OPS` list says, kept true one layer up.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_export_is_a_read_and_the_write_surface_does_not_bound_it() {
        let (reply, _) = transfer_from(
            "/main/memory/store",
            Some("/main/intruder"),
            sealed(),
            json!({"operation": "export", "table": "notes"}),
        )
        .await;
        assert!(
            reply["header"].get("error_code").is_none(),
            "a read must pass a write boundary untouched: {reply}"
        );
        assert_eq!(reply["header"]["operation"], "export");
    }

    /// Fail-closed, exactly as the cell-level rule is: a message with no sender
    /// is a source message no edge inside the scope produced, so it is outside.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_import_without_a_sender_is_outside_by_definition() {
        let (reply, rows) = transfer_from("/main/memory/store", None, sealed(), one_note()).await;
        assert_eq!(reply["header"]["error_code"], "write_denied");
        assert_eq!(rows, 0);
    }

    /// The default is `Open`, and `Open` bounds nothing: a cell that declares
    /// nothing keeps the behaviour it had before this rule existed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_open_write_surface_bounds_no_import_at_all() {
        let (reply, rows) = transfer_from(
            "/main/memory/store",
            Some("/main/intruder"),
            meclaw_core::TransferBounds::default(),
            one_note(),
        )
        .await;
        assert!(reply["header"].get("error_code").is_none(), "{reply}");
        assert_eq!(rows, 1);
    }

    /// A store directly under the colony root owns `/`, which contains every
    /// cell — the declaration is inert there, and says so rather than being
    /// special-cased. Same sentence as `store`'s `owner_scope`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn at_the_colony_root_the_owning_scope_is_everything() {
        let (reply, rows) = transfer_from("/store", Some("/anywhere"), sealed(), one_note()).await;
        assert!(reply["header"].get("error_code").is_none(), "{reply}");
        assert_eq!(rows, 1);
    }

    // ------------------------------------------------ GH #314, the exempt cell

    /// The ruling on #314: a cell whose contract says `transfer: "none"` is
    /// exempt from this seam. An **export** is refused too — the reason the
    /// write half was not enough is that the vault's disclosure was a read: the
    /// plaintext `name`/`version`/`status` columns of `vault_secrets` and the
    /// whole of `vault_audit` need no passphrase to be useful.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_exempt_cell_refuses_an_export_and_gives_up_no_table() {
        let (reply, _) = transfer_from(
            "/main/access/vault",
            Some("/main/access/invoke"),
            meclaw_core::TransferBounds::exempt(),
            json!({"operation": "export"}),
        )
        .await;
        assert_eq!(reply["header"]["error_code"], "transfer_exempt");
        assert_eq!(reply["header"]["operation"], "export");
        assert_eq!(reply["header"]["rows_affected"], 0);
        assert!(
            !reply.to_string().contains("notes"),
            "a refusal that names a table is an inventory: {reply}"
        );
    }

    /// The import half — "a store that cannot leave also cannot be overwritten
    /// through the same seam" (the ruling). Provable on the database.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_exempt_cell_refuses_an_import_and_writes_nothing() {
        let (reply, rows) = transfer_from(
            "/main/access/vault",
            Some("/main/access/invoke"),
            meclaw_core::TransferBounds::exempt(),
            one_note(),
        )
        .await;
        assert_eq!(reply["header"]["error_code"], "transfer_exempt");
        assert_eq!(rows, 0, "a refused import must leave the table untouched");
    }

    /// The exemption is answered before the arguments are read, so a malformed
    /// slot gets the same sentence as a well-formed one. Nothing about this
    /// cell's database — not even whether an argument would have parsed — is
    /// worth reporting to a caller it does not answer.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_exempt_cell_answers_a_nonsense_slot_the_same_way() {
        let (reply, _) = transfer_from(
            "/main/access/vault",
            Some("/main/access/invoke"),
            meclaw_core::TransferBounds::exempt(),
            json!("not an object at all"),
        )
        .await;
        assert_eq!(reply["header"]["error_code"], "transfer_exempt");
    }

    /// The negative pin, and the more important one: the exemption is opt-in.
    /// A cell that declares nothing exports exactly as it did before #314 — the
    /// substrate infers nothing from a cell's type or its table names.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_cell_that_declares_nothing_still_exports() {
        let (reply, _) = transfer_from(
            "/main/memory/store",
            Some("/main/anywhere"),
            meclaw_core::TransferBounds::default(),
            json!({"operation": "export", "table": "notes"}),
        )
        .await;
        assert!(
            reply["header"].get("error_code").is_none(),
            "absence must keep meaning 'no change': {reply}"
        );
        assert_eq!(reply["header"]["operation"], "export");
    }

    /// A message without the slot is not this module's business.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_ordinary_message_passes_straight_through() {
        let td = tempfile::TempDir::new().unwrap();
        let conn = crate::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let mut db = crate::DbConn::wrap(conn, None);
        let (otx, _orx) = tokio::sync::mpsc::channel(8);
        let sink = meclaw_core::OutputSink::new(
            otx,
            meclaw_core::Path::new("/cell"),
            meclaw_core::Uuid::now_v7(),
            meclaw_core::Uuid::now_v7(),
            64,
            meclaw_core::Headers::new(),
            None,
        );
        let msg = meclaw_core::MessageBuilder::new(meclaw_core::Path::new("/cell"))
            .body(meclaw_core::Body::Inline(json!({"messages": []})))
            .build();
        assert!(
            !handle_transfer_slot(
                &msg,
                &sink,
                &mut db,
                &meclaw_core::TransferBounds::default()
            )
            .await
        );
    }
}
