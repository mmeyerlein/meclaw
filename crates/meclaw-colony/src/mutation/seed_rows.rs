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
//! **Who writes (#1233, T6-08..10).** Cell boundaries are hard (no exceptions,
//! the way back is the message path). A store this very mutation STAGED has
//! never been born, and the colony writes its rows straight into the staged
//! `cell.db` — birth is substrate right, the same right
//! `mutation::stage::seed_cell_db_if_present` uses. Every RUNNING store gets its
//! rows as a message on its own seed lane ([`SeedOrder`], body slot
//! [`LANE_SLOT`], sender [`DOOR`]) and writes them on its own connection with
//! [`apply_rows`] — the one implementation of the counting both writers share.
//! It answers a receipt with a digest ([`RECEIPT_SLOT`]); the door writes the
//! receipts into the mutation's `mutation_log` row and answers `committed` only
//! after the last one ([`SeedWaits`], a wait table in the colony loop — the loop
//! never blocks on it).

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
    /// The store's own `cell.message_timeout`, read from the same `config.json`
    /// as its schema: the base of the wait for its seed receipt (#1233,
    /// OR-programm-tm.H3.6).
    pub message_timeout: Option<i64>,
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

/// Table name → declared column → type, as a store's `params.schema` states it.
type DeclaredTables = BTreeMap<String, BTreeMap<String, String>>;

/// Read the `params.schema` of the store at `cell_dir`.
///
/// The colony writes these `config.json` files and is the authority over them
/// (§ Authority model), so reading one back is the same read the staging path
/// already does — not a database isolation break, which is about `cell.db`.
///
/// Returns the declared tables and the store's `cell.message_timeout` from the
/// same read (#1233: the base of the wait for its seed receipt).
fn declared_schema(cell_dir: &std::path::Path) -> Result<(DeclaredTables, Option<i64>), String> {
    let path = cell_dir.join("config.json");
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let cfg: crate::config::ParsedConfig =
        meclaw_core::serde_json::from_str(&raw).map_err(|e| format!("{}: {e}", path.display()))?;
    let message_timeout = cfg.cell.message_timeout;
    let Some(schema) = cfg.params.get("schema").and_then(|v| v.as_object()) else {
        return Ok((BTreeMap::new(), message_timeout));
    };
    let mut out = BTreeMap::new();
    for (table, cols) in schema {
        let Some(cols) = cols.as_object() else {
            continue;
        };
        let mut m = BTreeMap::new();
        for (col, ty) in cols {
            // GH #822: a column is a type string or `{type, default}`.
            let ty = ty
                .as_str()
                .or_else(|| ty.get("type").and_then(|t| t.as_str()))
                .unwrap_or("text");
            m.insert(col.clone(), ty.to_string());
        }
        out.insert(table.clone(), m);
    }
    Ok((out, message_timeout))
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
        let (schema, message_timeout) = declared_schema(&cell_dir).map_err(|e| {
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
            message_timeout,
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
///
/// `connection_setup` is the target type's own connection setup
/// ([`crate::CellFactory::connection_setup`]), run on the fresh connection
/// before anything is read or written (GH #1077): a store table with a
/// full-text index fires an insert trigger that needs the store's tokenizer,
/// and without it the first row was refused `no such tokenizer`.
pub fn apply_entries(
    resolved: &[ResolvedSeedRows],
    connection_setup: Option<crate::db_conn::ReopenSetup>,
) -> Result<Vec<SeedRowsApplied>, MutationError> {
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
        if let Some(setup) = connection_setup {
            setup(&conn).map_err(|e| {
                MutationError::SeedTargetNotAStore(format!(
                    "{KEY} could not equip the connection to {}: {e}",
                    cell_db.display()
                ))
            })?;
        }
        let (inserted, already_present) = apply_rows(&conn, &r.table, &r.columns, &r.rows)
            .map_err(|e| {
                MutationError::SeedTargetNotAStore(format!(
                    "{KEY} {} in {}: {e}",
                    r.target.as_str(),
                    cell_db.display()
                ))
            })?;
        tracing::info!(
            target_path = %r.target.as_str(),
            table = %r.table,
            inserted,
            already_present,
            "seed_rows: rows entered an unborn store through the mutation door (GH #456)"
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

/// The write itself, on a connection the caller owns: stand `table` from its
/// declared `columns` when it is not standing yet, then insert every row that
/// is not already present, column for column. Returns `(inserted,
/// already_present)`.
///
/// #1233 (T6-08): ONE implementation for both writers — the door writing into an
/// unborn store it just staged (birth is substrate right), and a running store
/// writing on its own connection when the door's rows arrive on its seed lane.
/// Every row key is checked against `columns` here because the store's caller is
/// a message, and column names are formatted into SQL.
pub fn apply_rows(
    conn: &rusqlite::Connection,
    table: &str,
    columns: &BTreeMap<String, String>,
    rows: &[Map<String, Value>],
) -> Result<(usize, usize), String> {
    if !is_plain_identifier(table) {
        return Err(format!("{table:?} is not a plain table name"));
    }
    let col_defs = columns
        .iter()
        .map(|(c, t)| format!("\"{c}\" {}", sqlite_type(t)))
        .collect::<Vec<_>>()
        .join(", ");
    conn.execute(
        &format!("CREATE TABLE IF NOT EXISTS \"{table}\" ({col_defs})"),
        [],
    )
    .map_err(|e| format!("could not stand table {table}: {e}"))?;
    let mut inserted = 0usize;
    let mut already_present = 0usize;
    for (idx, row) in rows.iter().enumerate() {
        let cols: Vec<&String> = row.keys().collect();
        if let Some(col) = cols.iter().find(|c| !columns.contains_key(c.as_str())) {
            return Err(format!(
                "{table} row {}: column {col:?} is not declared",
                idx + 1
            ));
        }
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
            .map_err(|e| format!("{table} row {}: could not be checked: {e}", idx + 1))?;
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
        .map_err(|e| format!("{table} row {}: insert failed: {e}", idx + 1))?;
        inserted += 1;
    }
    Ok((inserted, already_present))
}

/// The address the door's seed lane speaks from: a running store accepts a
/// `seed_rows` slot from this sender only, and answers its receipt back here.
pub const DOOR: &str = "/colony/mutations";

/// The body slot a store's seed lane carries (`{"seed_rows": {id, table, rows}}`).
pub const LANE_SLOT: &str = "seed_rows";

/// The body slot a store's answer on its seed lane carries.
pub const RECEIPT_SLOT: &str = "seed_receipt";

/// The `error_code` of a ledger receipt the door wrote itself because the store
/// did not answer its seed lane before the knock's deadline (#1233,
/// OR-programm-tm.H3.6).
pub const RECEIPT_TIMEOUT_CODE: &str = "seed_receipt_timeout";

/// The `error_code` of a ledger receipt the door wrote itself because the store
/// is inactive when the order would leave (#1315, OR-agent-2.N2.6): routing
/// dead-letters a message to an inactive cell (`cell_inactive`) without waking
/// it, so the store can never answer, and waiting out its backstop only delays
/// the door's answer by `cell.message_timeout` (121 s with the shipped default,
/// measured in the gh1310 member colony whose `/os` had no external edge).
pub const RECEIPT_INACTIVE_CODE: &str = "seed_target_inactive";

/// One declaration bound for a RUNNING store (#1233, T6-09). The door no longer
/// opens a second connection into a cell's `cell.db` (cell boundaries are hard,
/// an owner ruling); it sends the rows as a message and the store writes them on
/// its own connection.
#[derive(Debug, Clone)]
pub struct SeedOrder {
    /// Correlates the store's receipt with this order.
    pub order_id: String,
    /// The mutation whose ledger row the receipt is written into.
    pub mutation_id: String,
    /// The store the rows go to.
    pub target: meclaw_core::Path,
    /// The declared table.
    pub table: String,
    /// The rows, as declared.
    pub rows: Vec<Map<String, Value>>,
    /// How long the door waits for this store's receipt.
    pub wait: std::time::Duration,
    /// The store is inactive (disconnected, or failed past its restart limit)
    /// as the mutation leaves it: the order is not sent, the door books [`RECEIPT_INACTIVE_CODE`]
    /// at once instead of waiting for a receipt that cannot come (#1315).
    pub inactive: bool,
}

impl SeedOrder {
    /// Split resolved declarations: a store this very mutation staged has never
    /// been born and takes its rows straight into the staged `cell.db` (birth is
    /// substrate right); every other store is running and gets an order.
    ///
    /// The wait of an order is the store's own B backstop (`cell.message_timeout`,
    /// else `message_timeout_default_ms`, the same resolution its spawn uses):
    /// the seed write is one message's work, and past the backstop the store has
    /// either answered or the substrate cut its handle. A store that switched its
    /// backstop off still gets the shipped default, because the door must answer
    /// (OR-programm-tm.H3.6).
    ///
    /// `is_active` is the registry's `active` flag after the mutation's own
    /// activity recompute: an order to an inactive store is marked
    /// [`SeedOrder::inactive`] (#1315).
    pub fn split(
        resolved: Vec<ResolvedSeedRows>,
        born_here: &[meclaw_core::Path],
        mutation_id: &str,
        message_timeout_default_ms: u64,
        is_active: impl Fn(&meclaw_core::Path) -> bool,
    ) -> (Vec<ResolvedSeedRows>, Vec<SeedOrder>) {
        let mut direct = Vec::new();
        let mut orders = Vec::new();
        for r in resolved {
            if born_here.contains(&r.target) {
                direct.push(r);
            } else {
                let wait =
                    crate::resolve_message_timeout(r.message_timeout, message_timeout_default_ms)
                        .unwrap_or(std::time::Duration::from_millis(
                            crate::DEFAULT_MESSAGE_TIMEOUT_MS,
                        ));
                let inactive = !is_active(&r.target);
                orders.push(SeedOrder {
                    order_id: meclaw_core::Uuid::now_v7().to_string(),
                    mutation_id: mutation_id.to_string(),
                    target: r.target,
                    table: r.table,
                    rows: r.rows,
                    wait,
                    inactive,
                });
            }
        }
        (direct, orders)
    }

    fn message(&self, trace_id: meclaw_core::Uuid) -> meclaw_core::Message {
        let rows: Vec<Value> = self.rows.iter().cloned().map(Value::Object).collect();
        let mut slot = Map::new();
        slot.insert("id".into(), Value::String(self.order_id.clone()));
        slot.insert("table".into(), Value::String(self.table.clone()));
        slot.insert("rows".into(), Value::Array(rows));
        let mut body = Map::new();
        // A universal body (`messages`) with the lane as a top-level slot, the
        // shape the `params` slot of a store already has.
        body.insert("messages".into(), Value::Array(Vec::new()));
        body.insert(LANE_SLOT.into(), Value::Object(slot));
        meclaw_core::MessageBuilder::new(self.target.clone())
            .reply_to(meclaw_core::Path::new(DOOR))
            .trace_id(trace_id)
            .body(meclaw_core::Body::Inline(Value::Object(body)))
            .build()
    }
}

/// Who is waiting for the receipts of one door knock (#1233, T6-10).
pub enum SeedWaiter {
    /// `ColonyMsg::Mutation`.
    Mutation(
        tokio::sync::oneshot::Sender<crate::mutation::MutationOutcome>,
        crate::mutation::MutationOutcome,
    ),
    /// `ColonyMsg::MutationDoor` (HTTP, `meclaw --apply`).
    Door(
        tokio::sync::oneshot::Sender<crate::mutation::MutationDoorOutcome>,
        crate::mutation::MutationDoorOutcome,
    ),
    /// The EDA door: the reply to emit.
    Reply(Option<meclaw_core::Path>, Value),
}

struct Pending {
    open: usize,
    receipts: BTreeMap<String, Vec<Value>>,
    waiter: SeedWaiter,
    /// The orders of this knock, so a deadline can name the ones still open.
    orders: Vec<String>,
    /// Park time plus the longest wait of its orders; `None` only when that sum
    /// does not fit an `Instant` (a backstop of centuries), which never expires.
    deadline: Option<tokio::time::Instant>,
}

/// The wait table in the colony loop (#1233, T6-10): the door answers
/// `committed` only once every running store has answered its seed lane, so a
/// caller that reads right after `committed` (provisioning, a hardening script)
/// sees the rows. The loop never blocks on it — a knock parks here, the loop
/// keeps routing, and the last receipt releases the answer.
#[derive(Default)]
pub struct SeedWaits {
    orders: std::collections::HashMap<String, (u64, meclaw_core::Path, String, String)>,
    pending: std::collections::HashMap<u64, Pending>,
    next: u64,
}

impl SeedWaits {
    /// Send every order on its store's lane and park `waiter` until every receipt
    /// is in. Returns the reply to emit when the knock completed at once (no
    /// order could be enqueued).
    pub async fn park(
        &mut self,
        orders: Vec<SeedOrder>,
        waiter: SeedWaiter,
        inbox_self_tx: &tokio::sync::mpsc::Sender<crate::colony::ColonyMsg>,
        writer_tx: &tokio::sync::mpsc::Sender<crate::persist::writer::ColonyWriteOp>,
        trace_id: meclaw_core::Uuid,
    ) -> Option<(Option<meclaw_core::Path>, Value)> {
        let key = self.next;
        self.next += 1;
        let mut pending = Pending {
            open: 0,
            receipts: BTreeMap::new(),
            waiter,
            orders: Vec::new(),
            deadline: None,
        };
        let mut wait = std::time::Duration::ZERO;
        for o in orders {
            if o.inactive {
                // #1315: routing would dead-letter this order as `cell_inactive`
                // without waking the store, so no receipt can come. The door
                // books the refusal now instead of arming a deadline for it.
                let mut r = Map::new();
                r.insert("id".into(), Value::String(o.order_id.clone()));
                r.insert("target".into(), Value::String(o.target.as_str().into()));
                r.insert("table".into(), Value::String(o.table.clone()));
                r.insert(
                    "error_code".into(),
                    Value::String(RECEIPT_INACTIVE_CODE.into()),
                );
                r.insert(
                    "error".into(),
                    Value::String(format!(
                        "{} is inactive, so its seed lane was not sent",
                        o.target.as_str()
                    )),
                );
                pending
                    .receipts
                    .entry(o.mutation_id)
                    .or_default()
                    .push(Value::Object(r));
                continue;
            }
            let msg = o.message(trace_id);
            match inbox_self_tx.try_send(crate::colony::ColonyMsg::Route {
                sender_path: meclaw_core::Path::new(DOOR),
                msg,
            }) {
                Ok(()) => {
                    self.orders.insert(
                        o.order_id.clone(),
                        (
                            key,
                            o.target.clone(),
                            o.mutation_id.clone(),
                            o.table.clone(),
                        ),
                    );
                    pending.open += 1;
                    pending.orders.push(o.order_id.clone());
                    wait = wait.max(o.wait);
                }
                Err(e) => {
                    let mut r = Map::new();
                    r.insert("target".into(), Value::String(o.target.as_str().into()));
                    r.insert("table".into(), Value::String(o.table.clone()));
                    r.insert(
                        "error".into(),
                        Value::String(format!("the seed lane could not be enqueued: {e}")),
                    );
                    pending
                        .receipts
                        .entry(o.mutation_id)
                        .or_default()
                        .push(Value::Object(r));
                }
            }
        }
        if pending.open == 0 {
            return finish(pending, writer_tx).await;
        }
        pending.deadline = tokio::time::Instant::now().checked_add(wait);
        self.pending.insert(key, pending);
        None
    }

    /// The earliest deadline of a parked knock — what the colony loop's one
    /// timer is armed on (#1233, OR-programm-tm.H3.6). `None` while nothing waits.
    pub fn next_deadline(&self) -> Option<tokio::time::Instant> {
        self.pending.values().filter_map(|p| p.deadline).min()
    }

    /// The colony loop's timer arm: release every knock whose deadline has
    /// passed (#1233, OR-programm-tm.H3.6). A store that never answers its lane
    /// (dead, failed, its order in the DLQ) is normal operation, and without
    /// this the knock would never be answered. The topology is committed and
    /// durable, so the answer stays `committed` (OR-programm-tm.H3.3); each
    /// missing receipt enters the ledger as an error with
    /// [`RECEIPT_TIMEOUT_CODE`]. A receipt that arrives later finds no open
    /// order and is dropped by [`Self::receive`].
    pub async fn expire(
        &mut self,
        inbox_self_tx: &tokio::sync::mpsc::Sender<crate::colony::ColonyMsg>,
        writer_tx: &tokio::sync::mpsc::Sender<crate::persist::writer::ColonyWriteOp>,
    ) {
        let now = tokio::time::Instant::now();
        let due: Vec<u64> = self
            .pending
            .iter()
            .filter(|(_, p)| p.deadline.is_some_and(|d| d <= now))
            .map(|(k, _)| *k)
            .collect();
        for key in due {
            let Some(mut pending) = self.pending.remove(&key) else {
                continue;
            };
            for order_id in std::mem::take(&mut pending.orders) {
                // An order whose receipt came in has already left the table.
                let Some((_, target, mutation_id, table)) = self.orders.remove(&order_id) else {
                    continue;
                };
                let mut r = Map::new();
                r.insert("id".into(), Value::String(order_id));
                r.insert("target".into(), Value::String(target.as_str().into()));
                r.insert("table".into(), Value::String(table));
                r.insert(
                    "error_code".into(),
                    Value::String(RECEIPT_TIMEOUT_CODE.into()),
                );
                r.insert(
                    "error".into(),
                    Value::String(format!(
                        "{} sent no seed receipt before the deadline (its message_timeout)",
                        target.as_str()
                    )),
                );
                pending
                    .receipts
                    .entry(mutation_id)
                    .or_default()
                    .push(Value::Object(r));
            }
            if let Some((Some(to), body)) = finish(pending, writer_tx).await {
                // The EDA door's reply, the shape `emit_reply_or_done` builds,
                // handed back to the loop as its own work item (`try_send`: the
                // loop is the only consumer of its inbox).
                let msg = meclaw_core::MessageBuilder::new(to)
                    .body(meclaw_core::Body::Inline(body))
                    .build();
                if let Err(e) = inbox_self_tx.try_send(crate::colony::ColonyMsg::Route {
                    sender_path: meclaw_core::Path::new("/colony"),
                    msg,
                }) {
                    tracing::warn!(error = %e, "seed wait reply could not be enqueued");
                }
            }
        }
    }

    /// A body arriving at the door that carries a [`RECEIPT_SLOT`]. Accepted only
    /// from the store the order went to; the last receipt of a knock writes the
    /// ledger and releases the answer (returned when it is a reply to emit).
    pub async fn receive(
        &mut self,
        sender: &meclaw_core::Path,
        body: &Value,
        writer_tx: &tokio::sync::mpsc::Sender<crate::persist::writer::ColonyWriteOp>,
    ) -> Option<(Option<meclaw_core::Path>, Value)> {
        let receipt = body.get(RECEIPT_SLOT)?.as_object()?;
        let order_id = receipt.get("id").and_then(|v| v.as_str())?;
        let Some((key, target, mutation_id, table)) = self.orders.get(order_id).cloned() else {
            tracing::warn!(order_id, sender = %sender.as_str(), "seed receipt for no open order");
            return None;
        };
        if &target != sender {
            tracing::warn!(
                order_id,
                sender = %sender.as_str(),
                target = %target.as_str(),
                "seed receipt from a cell the order did not go to — ignored"
            );
            return None;
        }
        self.orders.remove(order_id);
        let pending = self.pending.get_mut(&key)?;
        let mut r = receipt.clone();
        r.insert("target".into(), Value::String(target.as_str().into()));
        r.entry("table").or_insert(Value::String(table));
        pending
            .receipts
            .entry(mutation_id)
            .or_default()
            .push(Value::Object(r));
        pending.open = pending.open.saturating_sub(1);
        if pending.open > 0 {
            return None;
        }
        let done = self.pending.remove(&key)?;
        finish(done, writer_tx).await
    }
}

/// The colony loop's seed timer: sleeps until `deadline`, or forever when no
/// knock waits (the arm is disarmed then anyway; `select!` still builds the
/// future of a disarmed arm).
pub async fn until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending::<()>().await,
    }
}

/// Write each mutation's receipts into its ledger row, durably, THEN answer.
async fn finish(
    pending: Pending,
    writer_tx: &tokio::sync::mpsc::Sender<crate::persist::writer::ColonyWriteOp>,
) -> Option<(Option<meclaw_core::Path>, Value)> {
    for (id, receipts) in pending.receipts {
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        let op = crate::persist::writer::ColonyWriteOp::MutationLogSeedReceipts {
            id,
            receipts_json: Value::Array(receipts).to_string(),
            ack: Some(ack_tx),
        };
        if writer_tx.send(op).await.is_ok() {
            let _ = ack_rx.await;
        }
    }
    match pending.waiter {
        SeedWaiter::Mutation(ack, outcome) => {
            let _ = ack.send(outcome);
            None
        }
        SeedWaiter::Door(ack, outcome) => {
            let _ = ack.send(outcome);
            None
        }
        SeedWaiter::Reply(to, body) => Some((to, body)),
    }
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
