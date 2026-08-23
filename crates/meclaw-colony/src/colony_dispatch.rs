//! Phase-13.5-A6: cell→/colony endpoint dispatcher + extracted inbox-arm handlers.
//!
//! This module encapsulates everything called by both the inbox arm (HTTP API via
//! ColonyMsg) and the outputs arm (cell emission via route() →
//! RouteAction::ColonyDispatch). Before A6 all handlers lived inline in the inbox
//! arm of `colony_task` — now they are shared call points with identical semantics.
//!
//! Spec: `docs/meclaw-overview.md` § `/colony` as a virtual endpoint (Z.393-417).
//! Symmetry internal API ↔ external API (Z.397).
//!
//! **T1 scope**: 8 handlers extracted verbatim from the `colony_task` inbox arm. A
//! pure move — no logic change. The second caller (`dispatch_colony_endpoint` in the
//! outputs arm) follows in T3.

use crate::colony::CellStatus;
use crate::colony::RegistryEntry;
use crate::dead_letter::DeadLetter;
use crate::edge_table::EdgeTable;
use crate::persist::colony_db::ColonyDb;
use meclaw_core::{Path, Uuid};
use std::collections::HashMap;

/// Phase 12-B step-7.1: read-only snapshot from in-memory registry.
/// Filters: path (exact), path_prefix (string prefix), cell_type (exact).
/// Hard cap 1000 — protects HTTP-API callers from accidental fan-out.
///
/// Phase 13.5-A6: extracted verbatim from `colony_task` inbox-arm — same
/// semantics, two callers expected (inbox-arm now, outputs-arm in T3).
/// Spec: `docs/meclaw-overview.md` § /colony als virtueller Endpunkt (Symmetrie).
///
/// Phase 13.5-lifecycle-3b T8 (F7): `active` filter — `Some(true)` keeps only
/// `entry.active == true`, `Some(false)` only inactive, `None` keeps all.
/// Symmetric across the HTTP-Inbox path (`ColonyMsg::ReadRegistry` arm) and the
/// cell→/colony read path (`dispatch_colony_endpoint`): both call this function.
pub fn handle_read_registry(
    registry: &HashMap<Path, RegistryEntry>,
    path: Option<Path>,
    path_prefix: Option<Path>,
    cell_type: Option<String>,
    active: Option<bool>,
    limit: usize,
) -> crate::api_dto::ReadRegistryReply {
    let cap = limit.clamp(1, 1000);
    let entries: Vec<crate::api_dto::RegistryEntryDto> = registry
        .iter()
        .filter(|(p, _)| match &path {
            Some(want) => *p == want,
            None => true,
        })
        .filter(|(p, _)| match &path_prefix {
            Some(pref) => p.as_str().starts_with(pref.as_str()),
            None => true,
        })
        .filter(|(_, e)| match &cell_type {
            Some(ct) => e.cell_type == *ct,
            None => true,
        })
        .filter(|(_, e)| match active {
            Some(want) => e.active == want,
            None => true,
        })
        .take(cap)
        .map(|(p, e)| {
            let lifecycle_status = match &e.status {
                CellStatus::Awake => "Awake",
                CellStatus::Asleep { .. } => "Asleep",
                CellStatus::NotYetSpawned { .. } => "NotYetSpawned",
            }
            .to_string();
            crate::api_dto::RegistryEntryDto {
                path: p.as_str().to_string(),
                cell_id: e.cell_id.to_string(),
                cell_type: e.cell_type.clone(),
                lifecycle_status,
                active: e.active,
                failed: e.failed,
            }
        })
        .collect();
    crate::api_dto::ReadRegistryReply { entries }
}

/// Phase-16 W6d (A6): pure-Read of the persistent `dead_letters` table — the DLQ
/// is now durable in `colony.db` (the single source of truth), no longer an
/// in-memory `VecDeque`. `since`/`error_code`/`limit` filter at the SQL layer
/// (`?since=` on `created_at`, `?error_code=` exact, `?limit=` clamped 1..=1000).
/// The reply is the 6-field `DeadLetterDto` projection (the `message_json`
/// envelope column is for the drain's reconstruction, not the HTTP read).
///
/// **SYNC** (no `.await`): `ColonyDb` is `!Sync` (rusqlite `Connection`), so it
/// must never be borrowed across an await in the `colony_task` future. The caller
/// fences FIRST (await on `&writer_tx`, which IS Send) for read-after-write, THEN
/// calls this synchronous read+project. See `colony_task`'s `ReadDeadLetters` arm.
pub fn handle_read_dead_letters(
    colony_db: &ColonyDb,
    since: Option<i64>,
    error_code: Option<String>,
    limit: usize,
) -> crate::api_dto::ReadDeadLettersReply {
    let cap = limit.clamp(1, 1000);
    let rows = colony_db
        .read_dead_letters(since, error_code, cap)
        .unwrap_or_default();
    let entries: Vec<crate::api_dto::DeadLetterDto> = rows
        .into_iter()
        .map(|r| crate::api_dto::DeadLetterDto {
            sender_path: r.sender_path,
            original_target: r.original_target,
            resolved_target: r.resolved_target,
            error_code: r.error_code,
            trace_id: r.trace_id,
            created_at: r.created_at,
            // P1: best-effort projection out of the persisted envelope. A row
            // without a parseable `id` is not an error — the consumer degrades
            // to the trace-level link.
            message_id: serde_json::from_str::<serde_json::Value>(&r.message_json)
                .ok()
                .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(|s| s.to_string())),
        })
        .collect();
    crate::api_dto::ReadDeadLettersReply { entries }
}

/// Phase-16 W6d (A6): snapshot + reconstruct the FULL DLQ from the DB — every row
/// becomes a full `DeadLetter` (envelope from `message_json`), so the drain hook /
/// HTTP DELETE keep returning `Vec<DeadLetter>` with body/correlation_id intact.
///
/// **SYNC** (no `.await`): `!Sync` `ColonyDb` borrow. The caller fences first, then
/// calls this read, then enqueues the `DeleteAllDeadLetters` clear on `&writer_tx`
/// — the single-owner `colony_task` runs no other message between, so no row is
/// inserted in the read→delete gap (no TOCTOU). See the `DrainDeadLetters` arm.
pub fn handle_drain_dead_letters(colony_db: &ColonyDb) -> Vec<DeadLetter> {
    colony_db
        .read_all_dead_letters()
        .unwrap_or_default()
        .into_iter()
        .map(crate::colony::dead_letter_from_row)
        .collect()
}

/// Phase 12-B step-7.3: sync read via colony_db (no .await, so the !Sync borrow
/// is fine).
///
/// Phase 13.5-A6: extracted verbatim from `colony_task` inbox-arm — same
/// semantics, two callers expected (inbox-arm now, outputs-arm in T3).
/// Spec: `docs/meclaw-overview.md` § /colony als virtueller Endpunkt (Symmetrie).
///
/// W13 hardening: the `cell_type` filter is wired (see [`template_cell_type`]).
pub fn handle_read_templates(
    colony_db: &ColonyDb,
    cell_type: Option<String>,
    name: Option<String>,
    limit: usize,
) -> crate::api_dto::ReadTemplatesReply {
    handle_read_templates_from_rows(
        colony_db.read_templates().unwrap_or_default(),
        cell_type,
        name,
        limit,
    )
}

/// The cell type a scanned template instantiates, read off its `config.json`.
///
/// The `templates` table does not carry it: a scan records the `template.json`
/// identity (name, version, author, path), while the type lives one file over,
/// in the `config.json` the template ships. The `TemplateEntryDto` doc named the
/// two ways out — cache it at scan time, or read it on the fly — and this is the
/// second one, chosen because it changes no schema and no scan path. The read is
/// a handful of small files on an explicitly-filtered request; the unfiltered
/// listing (the common one) never calls it.
///
/// An unreadable or type-less `config.json` yields `None`, which is a non-match
/// rather than an error: a template directory the filter cannot classify is
/// simply not of the requested type.
fn template_cell_type(filesystem_path: &str) -> Option<String> {
    let raw =
        std::fs::read_to_string(std::path::Path::new(filesystem_path).join("config.json")).ok()?;
    let parsed: meclaw_core::serde_json::Value = meclaw_core::serde_json::from_str(&raw).ok()?;
    parsed
        .get("cell")
        .and_then(|c| c.get("type"))
        .and_then(|t| t.as_str())
        .map(String::from)
}

/// Phase 12-B step-7.4: sync read via colony_db (no .await).
///
/// Phase 13.5-A6: extracted verbatim from `colony_task` inbox-arm — same
/// semantics, two callers expected (inbox-arm now, outputs-arm in T3).
/// Spec: `docs/meclaw-overview.md` § /colony als virtueller Endpunkt (Symmetrie).
pub fn handle_read_mutations_audit(
    colony_db: &ColonyDb,
    since: Option<i64>,
    limit: usize,
) -> crate::api_dto::ReadMutationsAuditReply {
    let cap = limit.clamp(1, 1000);
    let rows = colony_db.read_mutation_log(since, cap).unwrap_or_default();
    let entries: Vec<crate::api_dto::MutationLogDto> = rows
        .into_iter()
        .map(|r| crate::api_dto::MutationLogDto {
            id: r.id,
            scope: r.scope,
            payload_json: r.payload_json,
            status: r.status,
            failure_reason: r.failure_reason,
            created_at: r.created_at,
            committed_at: r.committed_at,
            error_code: r.error_code,
            trace_id: r.trace_id,
        })
        .collect();
    crate::api_dto::ReadMutationsAuditReply { entries }
}

/// P1 (message browser): paginated, filtered read over `colony.db::message_log`.
///
/// Same shape as [`handle_read_trace`] — `spawn_blocking` plus a fresh
/// `SQLITE_OPEN_READ_ONLY` connection, so the WAL reader never touches the
/// writer thread. **Honest warning**: like every `Read*` arm it stalls the
/// Colony-Inbox loop for the query duration; the stall is bounded by
/// `scan_budget` (≤ 50_000 rows), not by `limit` alone.
///
/// Two-stage query (plan § F3): the inner select narrows through an existing
/// index (`created_at` for ordering + range, `trace_id`, `parent_message_id`,
/// `to_path`) and is capped at `scan_budget` rows; the outer select applies the
/// predicates `message_log` has no index for (`correlation_id`, `from_path`,
/// `body_kind`). When the inner select hits its cap, `scan_truncated` says so —
/// the residual predicates then only saw that window.
///
/// Never logs filter values or payloads (secret hygiene) — errors report the
/// failure class only.
pub async fn handle_read_messages(
    db_path: &std::path::Path,
    filter: crate::api_dto::MessageLogFilter,
) -> crate::api_dto::ReadMessagesReply {
    let limit = filter.limit.clamp(1, 1000);
    let scan_budget = filter.scan_budget.clamp(1, 50_000);
    let db_path = db_path.to_path_buf();

    let join = tokio::task::spawn_blocking(
        move || -> rusqlite::Result<(Vec<crate::api_dto::MessageLogDto>, usize)> {
            let conn = rusqlite::Connection::open_with_flags(
                &db_path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
            )?;
            // GH #98: read-only opens never run the setup functions — install
            // the busy budget directly.
            crate::persist::apply_busy_timeout(&conn)?;
            const COLUMNS: &str = "id, trace_id, parent_message_id, correlation_id, ttl,
                 from_path, to_path, reply_to, headers, body_kind, body_payload, created_at";
            let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

            // Single-row lookup by PRIMARY KEY short-circuits every other filter.
            if let Some(id) = filter.id.as_ref() {
                let sql = format!("SELECT {COLUMNS} FROM message_log WHERE id = ? LIMIT 1");
                params.push(Box::new(id.clone()));
                let rows = query_message_log(&conn, &sql, &params)?;
                let scanned = rows.len();
                return Ok((rows, scanned));
            }

            // --- inner select: indexed predicates only, capped at scan_budget ---
            let mut inner = format!("SELECT {COLUMNS} FROM message_log WHERE 1=1");
            if let Some(t) = filter.trace_id.as_ref() {
                inner.push_str(" AND trace_id = ?");
                params.push(Box::new(t.clone()));
            }
            if let Some(p) = filter.parent_message_id.as_ref() {
                inner.push_str(" AND parent_message_id = ?");
                params.push(Box::new(p.clone()));
            }
            if let Some(prefix) = filter.to_path_prefix.as_ref() {
                let (lo, hi) = path_prefix_range(prefix);
                inner.push_str(" AND to_path >= ?");
                params.push(Box::new(lo));
                if let Some(hi) = hi {
                    inner.push_str(" AND to_path < ?");
                    params.push(Box::new(hi));
                }
            }
            if let Some(s) = filter.since {
                inner.push_str(" AND created_at >= ?");
                params.push(Box::new(s));
            }
            if let Some(u) = filter.until {
                inner.push_str(" AND created_at <= ?");
                params.push(Box::new(u));
            }
            if let Some(cursor) = filter.before.as_ref() {
                inner.push_str(" AND (created_at < ? OR (created_at = ? AND id < ?))");
                params.push(Box::new(cursor.created_at));
                params.push(Box::new(cursor.created_at));
                params.push(Box::new(cursor.id.clone()));
            }
            inner.push_str(" ORDER BY created_at DESC, id DESC LIMIT ?");
            params.push(Box::new(scan_budget as i64));
            // Everything bound so far belongs to the inner select — the COUNT
            // probe below re-binds exactly this prefix.
            let inner_param_count = params.len();

            // --- outer select: residual predicates inside the scanned window ---
            let mut outer = format!("SELECT {COLUMNS} FROM ({inner}) WHERE 1=1");
            if let Some(c) = filter.correlation_id.as_ref() {
                outer.push_str(" AND correlation_id = ?");
                params.push(Box::new(c.clone()));
            }
            if let Some(prefix) = filter.from_path_prefix.as_ref() {
                let (lo, hi) = path_prefix_range(prefix);
                outer.push_str(" AND from_path >= ?");
                params.push(Box::new(lo));
                if let Some(hi) = hi {
                    outer.push_str(" AND from_path < ?");
                    params.push(Box::new(hi));
                }
            }
            if let Some(k) = filter.body_kind.as_ref() {
                outer.push_str(" AND body_kind = ?");
                params.push(Box::new(k.clone()));
            }
            outer.push_str(" ORDER BY created_at DESC, id DESC LIMIT ?");
            params.push(Box::new(limit as i64));

            let rows = query_message_log(&conn, &outer, &params)?;

            // How many rows did the inner select actually read? The outer LIMIT
            // hides that, so probe it separately — the inner select's params are
            // the leading `inner_param_count` entries of `params`.
            let count_sql = format!("SELECT COUNT(*) FROM ({inner})");
            let mut count_stmt = conn.prepare(&count_sql)?;
            let inner_params: Vec<&dyn rusqlite::ToSql> = params
                .iter()
                .take(inner_param_count)
                .map(|b| b.as_ref())
                .collect();
            let scanned: i64 = count_stmt
                .query_row(rusqlite::params_from_iter(inner_params.iter()), |r| {
                    r.get(0)
                })?;
            Ok((rows, scanned as usize))
        },
    )
    .await;

    let (entries, scanned) = match join {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            tracing::error!(error = ?e, "ReadMessages SQL failed");
            (Vec::new(), 0)
        }
        Err(e) => {
            tracing::error!(error = ?e, "ReadMessages spawn_blocking failed");
            (Vec::new(), 0)
        }
    };
    let next = if entries.len() == limit {
        entries.last().map(|e| crate::api_dto::MessageLogCursor {
            created_at: e.created_at,
            id: e.id.clone(),
        })
    } else {
        None
    };
    crate::api_dto::ReadMessagesReply {
        entries,
        next,
        scan_budget,
        scan_truncated: scanned >= scan_budget,
    }
}

/// Run a prepared `message_log` select and map every row to [`crate::api_dto::MessageLogDto`].
/// Column order must match the `COLUMNS` constant in [`handle_read_messages`].
fn query_message_log(
    conn: &rusqlite::Connection,
    sql: &str,
    params: &[Box<dyn rusqlite::ToSql>],
) -> rusqlite::Result<Vec<crate::api_dto::MessageLogDto>> {
    let mut stmt = conn.prepare(sql)?;
    let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
    let rows = stmt.query_map(rusqlite::params_from_iter(refs.iter()), |r| {
        Ok(crate::api_dto::MessageLogDto {
            id: r.get(0)?,
            trace_id: r.get(1)?,
            parent_message_id: r.get(2)?,
            correlation_id: r.get(3)?,
            ttl: r.get(4)?,
            from_path: r.get(5)?,
            to_path: r.get(6)?,
            reply_to: r.get(7)?,
            headers_json: r.get(8)?,
            body_kind: r.get(9)?,
            body_payload: r.get(10)?,
            created_at: r.get(11)?,
        })
    })?;
    rows.collect()
}

/// Encode a string prefix as a half-open range `[lo, hi)`.
///
/// P1 (message browser): `LIKE 'p%'` is only index-optimized when
/// `case_sensitive_like=ON`, which this workspace does not set — a range
/// comparison drives the B-Tree index unconditionally. `hi` is `None` when no
/// successor exists (empty prefix, or a prefix consisting only of `0xFF` bytes);
/// the caller then omits the upper bound and the prefix matches everything from
/// `lo` onwards.
pub(crate) fn path_prefix_range(prefix: &str) -> (String, Option<String>) {
    let lo = prefix.to_string();
    let mut bytes = prefix.as_bytes().to_vec();
    while let Some(last) = bytes.pop() {
        if last < 0xff {
            bytes.push(last + 1);
            return (lo, String::from_utf8(bytes).ok());
        }
    }
    (lo, None)
}

/// Phase 12-B step-7.6: spawn_blocking + WAL Read-Only Connection.
/// STALLS the Colony-Inbox loop until the JoinHandle resolves —
/// bounded by limit ≤ 1000.
///
/// Phase 13.5-A6: extracted verbatim from `colony_task` inbox-arm — same
/// semantics, two callers expected (inbox-arm now, outputs-arm in T3).
/// `db_path: &std::path::Path` per the spec owner' MOD #4: helper opens its own
/// read-only Connection via `spawn_blocking`, no `&ColonyDb` borrow over
/// `.await` (keeps the future Send + reusable from T3's outputs-arm-dispatch).
///
/// Spec: `docs/meclaw-overview.md` § /colony als virtueller Endpunkt (Symmetrie).
pub async fn handle_read_trace(
    db_path: &std::path::Path,
    trace_id: Option<Uuid>,
    path_prefix: Option<Path>,
    correlation_id: Option<Uuid>,
    only_error: bool,
    since: Option<i64>,
    limit: usize,
) -> crate::api_dto::ReadTraceReply {
    let cap = limit.clamp(1, 1000);
    let db_path = db_path.to_path_buf();
    let trace_id_str = trace_id.map(|u| u.to_string());
    let correlation_id_str = correlation_id.map(|u| u.to_string());
    let path_prefix_str = path_prefix.map(|p| p.as_str().to_string());
    let join = tokio::task::spawn_blocking(
        move || -> rusqlite::Result<Vec<crate::api_dto::MessageLogDto>> {
            let conn = rusqlite::Connection::open_with_flags(
                &db_path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
            )?;
            // GH #98: read-only opens never run the setup functions — install
            // the busy budget directly.
            crate::persist::apply_busy_timeout(&conn)?;
            // Build WHERE clauses incrementally.
            let mut sql = String::from(
                "SELECT id, trace_id, parent_message_id, correlation_id, ttl,
                    from_path, to_path, reply_to, headers, body_kind,
                    body_payload, created_at
             FROM message_log WHERE 1=1",
            );
            let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            if let Some(ref t) = trace_id_str {
                sql.push_str(" AND trace_id = ?");
                params.push(Box::new(t.clone()));
            }
            if let Some(ref c) = correlation_id_str {
                sql.push_str(" AND correlation_id = ?");
                params.push(Box::new(c.clone()));
            }
            if let Some(ref pp) = path_prefix_str {
                sql.push_str(" AND to_path LIKE ?");
                params.push(Box::new(format!("{pp}%")));
            }
            if only_error {
                // Heuristic: error_code lives in the headers JSON; the column
                // itself isn't extracted. LIKE-match is the pragmatic filter
                // until message_log gains a dedicated column (Phase-14).
                sql.push_str(" AND headers LIKE '%\"error_code\"%'");
            }
            if let Some(s) = since {
                sql.push_str(" AND created_at >= ?");
                params.push(Box::new(s));
            }
            sql.push_str(" ORDER BY created_at ASC LIMIT ?");
            params.push(Box::new(cap as i64));
            let mut stmt = conn.prepare(&sql)?;
            let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
            let rows = stmt.query_map(rusqlite::params_from_iter(param_refs.iter()), |r| {
                Ok(crate::api_dto::MessageLogDto {
                    id: r.get(0)?,
                    trace_id: r.get(1)?,
                    parent_message_id: r.get(2)?,
                    correlation_id: r.get(3)?,
                    ttl: r.get(4)?,
                    from_path: r.get(5)?,
                    to_path: r.get(6)?,
                    reply_to: r.get(7)?,
                    headers_json: r.get(8)?,
                    body_kind: r.get(9)?,
                    body_payload: r.get(10)?,
                    created_at: r.get(11)?,
                })
            })?;
            rows.collect()
        },
    )
    .await;
    let entries = match join {
        Ok(Ok(rows)) => rows,
        Ok(Err(e)) => {
            tracing::error!(error = ?e, "ReadTrace SQL failed");
            Vec::new()
        }
        Err(e) => {
            tracing::error!(error = ?e, "ReadTrace spawn_blocking failed");
            Vec::new()
        }
    };
    crate::api_dto::ReadTraceReply { entries }
}

/// Phase 12-B step-7.5: scope-prefix-filtered Nodes + Edges.
/// Root-scope "/" matches everything; sub-scope "/a" matches exactly "/a"
/// and "/a/..." but NOT "/abc".
///
/// Phase 13.5-A6: extracted verbatim from `colony_task` inbox-arm — same
/// semantics, two callers expected (inbox-arm now, outputs-arm in T3).
/// Spec: `docs/meclaw-overview.md` § /colony als virtueller Endpunkt (Symmetrie).
pub fn handle_read_graph(
    registry: &HashMap<Path, RegistryEntry>,
    edges: &EdgeTable,
    scope: Path,
) -> crate::api_dto::ReadGraphReply {
    let scope_str = scope.as_str().to_string();
    let in_scope = |p: &str| -> bool {
        if scope_str == "/" {
            return true;
        }
        if p == scope_str {
            return true;
        }
        p.starts_with(&scope_str) && p.as_bytes().get(scope_str.len()) == Some(&b'/')
    };
    let nodes: Vec<crate::api_dto::GraphNodeDto> = registry
        .iter()
        .filter(|(p, _)| in_scope(p.as_str()))
        .map(|(p, e)| crate::api_dto::GraphNodeDto {
            path: p.as_str().to_string(),
            cell_type: e.cell_type.clone(),
        })
        .collect();
    let scope_edges: Vec<crate::api_dto::GraphEdgeDto> = edges
        .iter()
        .filter(|e| in_scope(e.from.as_str()) && in_scope(e.to.as_str()))
        .map(|e| crate::api_dto::GraphEdgeDto {
            id: e.id.to_string(),
            from: e.from.as_str().to_string(),
            to: e.to.as_str().to_string(),
            // Phase 13.5-A1 F6: expose source strings for
            // match-pattern string-equality.
            condition: e.condition.as_ref().map(|c| c.source.clone()),
            modifier: e
                .modifier
                .as_ref()
                .and_then(|m| meclaw_core::serde_json::to_value(&m.source).ok()),
            // GH #367: the routing phase (GH #283) is part of what an edge IS,
            // so a reader of the graph — a boot probe, a builder, the UI — has
            // to be able to see it.
            is_default: e.is_default,
        })
        .collect();
    crate::api_dto::ReadGraphReply {
        scope: scope_str,
        graph_version: 0, // Phase-14: real version bump
        nodes,
        edges: scope_edges,
    }
}

/// Phase 11 slice 11-E: internally triggers the same path as the boot scan.
/// The CLI flag `--rescan-templates` and (phase 12) an HTTP POST send this operation.
///
/// Phase 13.5-A6: extracted verbatim from `colony_task` inbox-arm — same
/// semantics, two callers expected (inbox-arm now, outputs-arm in T3).
/// `&ColonyDb` is allowed: `apply_scan_result` is `fn -> impl Future + Send`
/// (synchronous-prologue pattern), the DB borrow does NOT live in the future — see
/// the `templates/mod.rs` doc. The result is returned so that both callers
/// (regular + shutdown drain) can emit their respective log message
/// (verbatim preservation of the drain variant).
/// Spec: `docs/meclaw-overview.md` § `/colony` as a virtual endpoint (symmetry).
pub fn handle_rescan_templates<'a>(
    colony_db: &ColonyDb,
    templates_root: &'a std::path::Path,
) -> impl std::future::Future<Output = Result<(), crate::templates::scanner::ScannerError>> + Send + 'a
{
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // Delegate to `apply_scan_result`, which uses the same synchronous-prologue
    // pattern: the `&ColonyDb` borrow lives only in the sync prelude, the
    // returned future captures only Send-types (writer-tx clone +
    // queue-depth Arc-clone + owned data). Keeps `colony_task` Send.
    crate::templates::apply_scan_result(templates_root, colony_db, now)
}

// ============================================================================
// Phase-13.5-A6-T3: dispatch_colony_endpoint + reply-builders + parser-helpers
// ============================================================================

/// Build a reply-Cascade if `reply_to` is set, else `Done`.
///
/// Reply routing per the A6 plan: NO `outputs_tx` send (that would fill the outputs
/// channel). Instead `RouteAction::Cascade { sender: "/colony", msg }` —
/// `route_with_log` takes the cascade arm and routes the reply directly to
/// `reply_to`. `sender = "/colony"` is consistent with `send_eda_reject`
/// (see `colony.rs::send_eda_reject` — the same virtual origin path).
fn emit_reply_or_done(
    reply_to: Option<meclaw_core::Path>,
    reply_body: meclaw_core::serde_json::Value,
) -> crate::colony::RouteAction {
    match reply_to {
        Some(rt) => {
            let reply_msg = meclaw_core::MessageBuilder::new(rt)
                .body(meclaw_core::Body::Inline(reply_body))
                .build();
            crate::colony::RouteAction::Cascade {
                sender: meclaw_core::Path::new("/colony"),
                msg: reply_msg,
            }
        }
        None => crate::colony::RouteAction::Done,
    }
}

/// A `/colony` read filter that arrived but could not be read (GH #341, #359).
///
/// Never silently dropped: an ignored filter and an empty filter must not look
/// alike from the outside, so this becomes an error reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadQueryError {
    /// The key whose value could not be read, e.g. `query` or `query.scope`.
    pub key: String,
    /// Human-readable reason, naming the key and the JSON type found.
    pub details: String,
}

/// The `limit` every `/colony` read shares: default 100, floored at 1, capped at
/// 1000 (Spec Z.414). Out of RANGE stays clamped — clamped is not dropped; only
/// out of TYPE is refused.
const READ_LIMIT_DEFAULT: usize = 100;

/// Read `body.query` as an optional object.
///
/// A missing `query` or a `query` of `null` is the documented default ("if
/// `query` or a single field is missing, the defaults apply"). A `query` that is
/// present but not an object is a refused filter, never an unfiltered answer.
fn read_query_object(
    body: &meclaw_core::serde_json::Value,
) -> Result<
    Option<&meclaw_core::serde_json::Map<String, meclaw_core::serde_json::Value>>,
    ReadQueryError,
> {
    use meclaw_core::serde_json::Value;
    match body.get("query") {
        Some(Value::Object(q)) => Ok(Some(q)),
        Some(other) if !other.is_null() => Err(ReadQueryError {
            key: "query".into(),
            details: format!(
                "`query` must be an object, found {} — the documented read \
                 envelope is {{\"query\": {{…}}}}",
                json_type_name(other)
            ),
        }),
        _ => Ok(None),
    }
}

/// A present-but-wrong-typed field of the read envelope.
fn wrong_type(field: &str, want: &str, found: &meclaw_core::serde_json::Value) -> ReadQueryError {
    ReadQueryError {
        key: format!("query.{field}"),
        details: format!(
            "`query.{field}` must be {want}, found {}",
            json_type_name(found)
        ),
    }
}

/// Type alias for the borrowed `query` object the field readers work on.
type QueryObject<'a> =
    Option<&'a meclaw_core::serde_json::Map<String, meclaw_core::serde_json::Value>>;

/// Read an optional string field. Absent or `null` is the documented default.
fn read_opt_str<'a>(q: QueryObject<'a>, field: &str) -> Result<Option<&'a str>, ReadQueryError> {
    use meclaw_core::serde_json::Value;
    match q.and_then(|q| q.get(field)) {
        Some(Value::String(s)) => Ok(Some(s.as_str())),
        Some(other) if !other.is_null() => Err(wrong_type(field, "a string", other)),
        _ => Ok(None),
    }
}

/// Read an optional bool field.
fn read_opt_bool(q: QueryObject<'_>, field: &str) -> Result<Option<bool>, ReadQueryError> {
    use meclaw_core::serde_json::Value;
    match q.and_then(|q| q.get(field)) {
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(other) if !other.is_null() => Err(wrong_type(field, "a boolean", other)),
        _ => Ok(None),
    }
}

/// Read an optional signed-integer field.
fn read_opt_i64(q: QueryObject<'_>, field: &str) -> Result<Option<i64>, ReadQueryError> {
    use meclaw_core::serde_json::Value;
    match q.and_then(|q| q.get(field)) {
        Some(Value::Number(n)) => n.as_i64().map(Some).ok_or_else(|| ReadQueryError {
            key: format!("query.{field}"),
            details: format!("`query.{field}` must be an integer, found {n}"),
        }),
        Some(other) if !other.is_null() => Err(wrong_type(field, "an integer", other)),
        _ => Ok(None),
    }
}

/// Read the shared `limit` field. Out of type is refused, out of range clamped.
fn read_limit(q: QueryObject<'_>) -> Result<usize, ReadQueryError> {
    use meclaw_core::serde_json::Value;
    let raw = match q.and_then(|q| q.get("limit")) {
        Some(Value::Number(n)) => n.as_u64().ok_or_else(|| ReadQueryError {
            key: "query.limit".into(),
            details: format!("`query.limit` must be a non-negative integer, found {n}"),
        })?,
        Some(other) if !other.is_null() => {
            return Err(wrong_type("limit", "a number", other));
        }
        _ => READ_LIMIT_DEFAULT as u64,
    };
    Ok((raw as usize).clamp(1, 1000))
}

/// Read an optional UUID field. A string of the wrong SHAPE is refused just like
/// a value of the wrong TYPE — `Uuid::parse_str(s).ok()` used to turn a broken
/// UUID into "no filter", so a caller asking for one trace got every trace.
fn read_opt_uuid(q: QueryObject<'_>, field: &str) -> Result<Option<Uuid>, ReadQueryError> {
    match read_opt_str(q, field)? {
        Some(s) => Uuid::parse_str(s).map(Some).map_err(|e| ReadQueryError {
            key: format!("query.{field}"),
            details: format!("`query.{field}` must be a valid UUID: {e}"),
        }),
        None => Ok(None),
    }
}

/// The filters a `/colony/registry` read carries (GH #359).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryReadFilters {
    /// Exact path match.
    pub path: Option<meclaw_core::Path>,
    /// Prefix match on the path string.
    pub path_prefix: Option<meclaw_core::Path>,
    /// Cell-type filter.
    pub cell_type: Option<String>,
    /// Active filter (F7): `Some(true)` active only, `Some(false)` inactive
    /// only, `None` keeps all.
    pub active: Option<bool>,
    /// Hard cap on returned entries.
    pub limit: usize,
}

/// The filters a `/colony/templates` read carries (GH #359).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplatesReadFilters {
    /// Exact match on the cell type declared in the template's `config.json`.
    pub cell_type: Option<String>,
    /// Exact match on `template.json::name`.
    pub name: Option<String>,
    /// Hard cap on returned entries.
    pub limit: usize,
}

/// The filters a `/colony/trace` read carries (GH #359).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceReadFilters {
    /// Filter on `trace_id`.
    pub trace_id: Option<Uuid>,
    /// Prefix match on `to_path`.
    pub path_prefix: Option<Path>,
    /// Filter on `correlation_id`.
    pub correlation_id: Option<Uuid>,
    /// Only rows carrying an `error_code`.
    pub only_error: bool,
    /// Only rows with `created_at >= since` (Unix seconds).
    pub since: Option<i64>,
    /// Hard cap on returned entries.
    pub limit: usize,
}

/// Parse `body.query.{path,path_prefix,cell_type,active,limit}` for the
/// `/colony/registry` endpoint. `active` is an optional JSON bool (F7).
///
/// GH #359: a field that is present but wrong-typed is refused, never dropped —
/// an ignored filter and an empty filter must not look alike from the outside.
/// Absent stays absent, and `limit` stays clamped to 1…1000.
pub fn parse_read_query_path_filters(
    body: &meclaw_core::serde_json::Value,
) -> Result<RegistryReadFilters, ReadQueryError> {
    let q = read_query_object(body)?;
    Ok(RegistryReadFilters {
        path: read_opt_str(q, "path")?.map(meclaw_core::Path::new),
        path_prefix: read_opt_str(q, "path_prefix")?.map(meclaw_core::Path::new),
        cell_type: read_opt_str(q, "cell_type")?.map(String::from),
        active: read_opt_bool(q, "active")?,
        limit: read_limit(q)?,
    })
}

/// Parse `body.query.{cell_type,name,limit}` for `/colony/templates` reads.
///
/// GH #359: same discipline as [`parse_read_query_path_filters`].
pub fn parse_read_query_templates_filters(
    body: &meclaw_core::serde_json::Value,
) -> Result<TemplatesReadFilters, ReadQueryError> {
    let q = read_query_object(body)?;
    Ok(TemplatesReadFilters {
        cell_type: read_opt_str(q, "cell_type")?.map(String::from),
        name: read_opt_str(q, "name")?.map(String::from),
        limit: read_limit(q)?,
    })
}

/// Parse `body.query.{trace_id,path_prefix,correlation_id,only_error,since,limit}`
/// for `/colony/trace` reads.
///
/// GH #359: the sharp case lives here — `Uuid::parse_str(s).ok()` turned a
/// syntactically broken UUID into "no filter", so a caller asking for ONE trace
/// got the newest 100 entries of EVERY trace. A broken UUID is now refused.
pub fn parse_read_query_trace_filters(
    body: &meclaw_core::serde_json::Value,
) -> Result<TraceReadFilters, ReadQueryError> {
    let q = read_query_object(body)?;
    Ok(TraceReadFilters {
        trace_id: read_opt_uuid(q, "trace_id")?,
        path_prefix: read_opt_str(q, "path_prefix")?.map(Path::new),
        correlation_id: read_opt_uuid(q, "correlation_id")?,
        only_error: read_opt_bool(q, "only_error")?.unwrap_or(false),
        since: read_opt_i64(q, "since")?,
        limit: read_limit(q)?,
    })
}

/// Name the JSON type of a value for an error message.
fn json_type_name(v: &meclaw_core::serde_json::Value) -> &'static str {
    use meclaw_core::serde_json::Value;
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Read the scope filter of a `/colony/graph` read out of the request body.
///
/// The documented shape is `body.query.scope` — the request envelope every
/// `/colony/*` read shares (`docs/meclaw-overview.md` § `/colony` als virtueller
/// Endpunkt), and the shape the shipped `templates/canvy` probe sends.
///
/// **Deprecated alias (GH #341, ruling K-1):** a top-level `body.scope` is still
/// accepted for exactly one release and then goes. It is only consulted when the
/// documented shape carries no scope, so the alias can never override it.
///
/// A filter that is present but unreadable (wrong JSON type) is an error, never
/// a silent fall-back to the root scope. Absent means absent: no `query`, no
/// `scope`, or either of them `null`, is the documented root default.
pub fn parse_graph_scope(body: &meclaw_core::serde_json::Value) -> Result<Path, ReadQueryError> {
    use meclaw_core::serde_json::Value;
    match body.get("query") {
        Some(Value::Object(q)) => match q.get("scope") {
            Some(Value::String(s)) => return Ok(Path::new(s)),
            Some(other) if !other.is_null() => {
                return Err(ReadQueryError {
                    key: "query.scope".into(),
                    details: format!(
                        "`query.scope` must be a path string, found {}",
                        json_type_name(other)
                    ),
                });
            }
            // No `scope` in the query object: the documented per-field default
            // applies — fall through to the deprecated alias, then to root.
            _ => {}
        },
        Some(other) if !other.is_null() => {
            return Err(ReadQueryError {
                key: "query".into(),
                details: format!(
                    "`query` must be an object, found {} — the documented read \
                     envelope is {{\"query\": {{\"scope\": \"<path>\"}}}}",
                    json_type_name(other)
                ),
            });
        }
        _ => {}
    }
    match body.get("scope") {
        Some(Value::String(s)) => {
            tracing::warn!(
                scope = %s,
                "/colony/graph: top-level `scope` is deprecated (GH #341) — \
                 send the documented query envelope instead; the alias goes in \
                 the next release"
            );
            Ok(Path::new(s))
        }
        Some(other) if !other.is_null() => Err(ReadQueryError {
            key: "scope".into(),
            details: format!(
                "deprecated top-level `scope` must be a path string, found {}",
                json_type_name(other)
            ),
        }),
        _ => Ok(Path::new("/")),
    }
}

/// Answer a `/colony/graph` read: parse the filter out of the request body, then
/// project the topology through it. Returns the reply body exactly as the
/// dispatcher puts it on the wire, so the request shape and the answer can be
/// pinned together.
pub fn build_graph_read_reply(
    registry: &HashMap<Path, RegistryEntry>,
    edges: &EdgeTable,
    body: &meclaw_core::serde_json::Value,
) -> meclaw_core::serde_json::Value {
    match parse_graph_scope(body) {
        Ok(scope) => build_graph_reply(&handle_read_graph(registry, edges, scope)),
        Err(e) => refuse_read("/colony/graph", "graph", &e),
    }
}

/// Answer a `/colony/registry` read: parse the filters out of the request body,
/// then project the registry through them. Returns the reply body exactly as the
/// dispatcher puts it on the wire, so the request shape and the answer can be
/// pinned together (GH #359, the pattern of [`build_graph_read_reply`]).
pub fn build_registry_read_reply(
    registry: &HashMap<Path, RegistryEntry>,
    body: &meclaw_core::serde_json::Value,
) -> meclaw_core::serde_json::Value {
    match parse_read_query_path_filters(body) {
        Ok(f) => build_registry_reply(&handle_read_registry(
            registry,
            f.path,
            f.path_prefix,
            f.cell_type,
            f.active,
            f.limit,
        )),
        Err(e) => refuse_read("/colony/registry", "registry", &e),
    }
}

/// Answer a `/colony/templates` read off the rows the caller already extracted.
pub fn build_templates_read_reply(
    rows: Vec<crate::persist::colony_db::TemplateRow>,
    body: &meclaw_core::serde_json::Value,
) -> meclaw_core::serde_json::Value {
    match parse_read_query_templates_filters(body) {
        Ok(f) => build_templates_reply(&handle_read_templates_from_rows(
            rows,
            f.cell_type,
            f.name,
            f.limit,
        )),
        Err(e) => refuse_read("/colony/templates", "templates", &e),
    }
}

/// Log the refused filter once and build the endpoint's error reply (GH #359).
fn refuse_read(endpoint: &str, slot: &str, err: &ReadQueryError) -> meclaw_core::serde_json::Value {
    tracing::warn!(
        endpoint = endpoint,
        key = %err.key,
        details = %err.details,
        "unreadable filter — answering an error, not the unfiltered holdings"
    );
    build_read_error_reply(slot, err)
}

/// Extract the inline JSON body or return `Null` for blob bodies.
fn body_value(msg: &meclaw_core::Message) -> meclaw_core::serde_json::Value {
    match &msg.body {
        meclaw_core::Body::Inline(v) => v.clone(),
        meclaw_core::Body::Blob(_) => meclaw_core::serde_json::Value::Null,
    }
}

// ---------------------------- Reply-builders ----------------------------

fn build_mutation_reply(
    outcome: &crate::mutation::MutationOutcome,
) -> meclaw_core::serde_json::Value {
    use crate::mutation::MutationOutcome;
    let body = match outcome {
        MutationOutcome::Committed { id } => meclaw_core::serde_json::json!({
            "outcome": "committed",
            "id": id,
        }),
        // GH #293 — `violations` is deliberately NOT on this wire yet. The EDA
        // reply body is a public contract surface (README § Stability), and the
        // rendered `details` already carries every violation, one per line.
        MutationOutcome::Rejected {
            id,
            error_code,
            details,
            violations: _,
        } => meclaw_core::serde_json::json!({
            "outcome": "rejected",
            "id": id,
            "error_code": error_code,
            "details": details,
        }),
    };
    meclaw_core::serde_json::json!({ "mutation": body })
}

fn build_registry_reply(
    reply: &crate::api_dto::ReadRegistryReply,
) -> meclaw_core::serde_json::Value {
    meclaw_core::serde_json::json!({
        "registry": meclaw_core::serde_json::to_value(&reply.entries).unwrap_or_default(),
    })
}

fn build_templates_reply(
    reply: &crate::api_dto::ReadTemplatesReply,
) -> meclaw_core::serde_json::Value {
    meclaw_core::serde_json::json!({
        "templates": meclaw_core::serde_json::to_value(&reply.entries).unwrap_or_default(),
    })
}

fn build_rescan_reply(
    outcome: &Result<(), crate::templates::scanner::ScannerError>,
) -> meclaw_core::serde_json::Value {
    match outcome {
        Ok(()) => meclaw_core::serde_json::json!({ "rescan": { "status": "ok" } }),
        Err(e) => meclaw_core::serde_json::json!({
            "rescan": { "status": "error", "error": format!("{e:?}") },
        }),
    }
}

fn build_graph_reply(reply: &crate::api_dto::ReadGraphReply) -> meclaw_core::serde_json::Value {
    meclaw_core::serde_json::json!({
        "graph": {
            "scope": reply.scope,
            "graph_version": reply.graph_version,
            "nodes": meclaw_core::serde_json::to_value(&reply.nodes).unwrap_or_default(),
            "edges": meclaw_core::serde_json::to_value(&reply.edges).unwrap_or_default(),
        },
    })
}

/// GH #341/#359: a `/colony` read whose filter could not be read answers in the
/// endpoint's own top-level slot, discriminated by `status` — the shape
/// `build_rescan_reply` and `build_mutation_reply` already use. It deliberately
/// carries no result list: a reader must not be able to mistake a refused filter
/// for an answer, and `reply["<slot>"].as_array()` is `None` on this shape.
///
/// `slot` is the endpoint's own name: `graph`, `registry`, `templates`, `trace`.
pub fn build_read_error_reply(slot: &str, err: &ReadQueryError) -> meclaw_core::serde_json::Value {
    meclaw_core::serde_json::json!({
        slot: {
            "status": "error",
            "error_code": "invalid_query",
            "details": err.details,
        },
    })
}

fn build_trace_reply(reply: &crate::api_dto::ReadTraceReply) -> meclaw_core::serde_json::Value {
    meclaw_core::serde_json::json!({
        "trace": meclaw_core::serde_json::to_value(&reply.entries).unwrap_or_default(),
    })
}

// ---------------------------- Main dispatcher ----------------------------

/// Phase-13.5-A6-T3: routes `/colony/<endpoint>` to the T1 helpers resp.
/// `handle_mutation`, builds the reply message as a UBF top-level slot and returns
/// a `RouteAction`.
///
/// **NO self-send** to `inbox_self_tx` — direct call of the helpers.
/// **NO outputs_tx send** for the reply — the reply goes back to `msg.reply_to`
/// via `RouteAction::Cascade` through `route_with_log`.
///
/// **Send constraint**: `&ColonyDb` is `!Sync` (RefCell<Connection>) — `&ColonyDb`
/// as a parameter would make the surrounding `colony_task` future `!Send` (every
/// `.await` in the async fn body captures the borrow in the state machine, even if
/// it is only used in one branch). Solution: `&ColonyDb` is split into its sub-refs
/// in the **caller** (colony.rs, before the `.await`); dispatch_colony_endpoint
/// only receives Send sub-refs:
/// - `writer_tx: &Sender<ColonyWriteOp>` (Send+Sync)
/// - `db_path: &Path` (Send+Sync)
/// - `templates_rows`/`mutation_audit_rows`: pre-extracted synchronously in the
///   caller, passed through owned as parameters
///
/// **`/colony/events`** is deferred (U4) and falls into the `_ =>` arm
/// → `ColonyEndpointUnimplemented` DLQ push with `sender` pass-through
/// (must-fix #2).
///
/// Spec: `docs/meclaw-overview.md` § `/colony` as a virtual endpoint (Z.393-417).
///
/// **Send-pre-extraction params**:
/// - `templates_rows`: pre-extracted template rows for `/colony/templates`. The caller
///   reads `colony_db.read_templates()` synchronously before the `.await` and passes
///   them through owned.
/// - `rescan_future`: pre-extracted rescan future for `/colony/templates/rescan`.
///   The caller builds `handle_rescan_templates(&colony_db, &root)` synchronously (the
///   synchronous prologue consumes the `&ColonyDb` borrow, the returned future captures
///   only Send-owned data) and passes it through boxed. Lifetime `'fut` is bound to
///   `&root` in the caller (it lives in the `colony_task` scope; the future is awaited
///   in the same scope and not spawned).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn dispatch_colony_endpoint<'fut>(
    registry: &mut std::collections::HashMap<meclaw_core::Path, crate::RegistryEntry>,
    hive_scopes: &mut crate::hive_scope::HiveScopeTable,
    edges: &mut crate::edge_table::EdgeTable,
    node_contracts: &mut std::collections::HashMap<meclaw_core::Path, crate::NodeContract>, // Hardening Slice 1 (Task 1.4) — forwarded to handle_mutation
    dead_letters: &mut std::collections::VecDeque<crate::dead_letter::DeadLetter>,
    writer_tx: &tokio::sync::mpsc::Sender<crate::persist::writer::ColonyWriteOp>,
    db_path: &std::path::Path,
    templates_snapshot: crate::templates::TemplatesRegistry,
    templates_rows: Vec<crate::persist::colony_db::TemplateRow>,
    rescan_future: std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<(), crate::templates::scanner::ScannerError>>
                + Send
                + 'fut,
        >,
    >,
    factories: &crate::CellFactoryRegistry,
    root: &std::path::Path,
    inbox_self_tx: &tokio::sync::mpsc::Sender<crate::colony::ColonyMsg>,
    outputs_tx: &tokio::sync::mpsc::Sender<meclaw_core::CellEmission>,
    endpoint: meclaw_core::Path,
    msg: meclaw_core::Message,
    sender: meclaw_core::Path,
    idle_default_ms: u64, // Phase-13.5 A7 — colony.json idle-default forwarded to handle_mutation
    message_timeout_default_ms: u64, // P3-B-plumb-2 — colony.json B-backstop default forwarded to handle_mutation
    mailbox_default_capacity: usize, // Paket-1 T20 — colony.json mailbox-default forwarded to handle_mutation
    strict_validation: bool, // paket-7 B5 — colony.json strict_validation forwarded to handle_mutation
    blob_store: Option<std::sync::Arc<crate::DiskBlobStore>>, // Phase-13.5 A8 — forwarded to handle_mutation
    blob_inline_max_bytes: usize, // Phase-13.5 A8 (F2) — offload threshold forwarded to handle_mutation
    env_source: Option<&std::path::Path>, // U8 (RULED A8) — env source from startup, forwarded to handle_mutation
) -> crate::colony::RouteAction {
    let body = body_value(&msg);
    let reply_to = msg.reply_to.clone();
    let trace_id = msg.trace_id;
    let parent_message_id = msg.id;

    match endpoint.as_str() {
        "/colony/mutations" => {
            // F4-PIN: body is passed verbatim as payload — `handle_mutation`
            // extracts `diff`/`ctx`/`scope` from its own schema.
            let outcome = crate::colony::handle_mutation(
                registry,
                hive_scopes,
                edges,
                node_contracts,
                dead_letters,
                writer_tx,
                templates_snapshot,
                factories,
                root,
                inbox_self_tx,
                outputs_tx,
                body,
                reply_to.clone(),
                trace_id,
                parent_message_id,
                idle_default_ms,
                message_timeout_default_ms,
                mailbox_default_capacity,
                strict_validation,
                blob_store.clone(),
                blob_inline_max_bytes,
                env_source,
                None, // /colony/mutations dispatch path: no test sync hook
            )
            .await;
            let reply_body = build_mutation_reply(&outcome);
            emit_reply_or_done(reply_to, reply_body)
        }
        "/colony/registry" => {
            let reply_body = build_registry_read_reply(registry, &body);
            emit_reply_or_done(reply_to, reply_body)
        }
        "/colony/dead_letters" => {
            // W2d (Substrat, ruling 2026-06-12): `/colony/dead_letters` is
            // a READ-ONLY diagnostic sink, served EXCLUSIVELY via the dedicated
            // API inbox variants `ColonyMsg::ReadDeadLetters`/`DrainDeadLetters`
            // — NEVER via EDA dispatch. A dispatch that lands here is therefore an
            // illegitimate WRITE to a read endpoint (the pre-W2d hard-coded cell
            // fallback target). Serving it as a read replied the DLQ listing back
            // to the emitting cell (`reply_to`), which re-emitted, ~13 000× — a
            // self-sustaining source loop. It is HARD-REJECTED instead: one
            // `colony_endpoint_unimplemented` DLQ entry (sender pass-through),
            // `Done` — no reply, no re-injection. Only `/colony/mutations` is a
            // writable EDA endpoint (13.5-A6); the `/colony/reads` class is not.
            tracing::warn!(
                endpoint = %endpoint.as_str(),
                sender = %sender.as_str(),
                "write to read-only /colony/dead_letters — hard-reject (no re-injection)"
            );
            let resolved = endpoint.clone();
            crate::colony::push_dead_letter(
                dead_letters,
                crate::dead_letter::DeadLetter {
                    sender_path: sender,
                    original_target: endpoint,
                    resolved_target: resolved,
                    message: msg,
                    reason: crate::dead_letter::DeadLetterReason::ColonyEndpointUnimplemented,
                },
            );
            crate::colony::RouteAction::Done
        }
        "/colony/templates" => {
            let reply_body = build_templates_read_reply(templates_rows, &body);
            emit_reply_or_done(reply_to, reply_body)
        }
        "/colony/templates/rescan" => {
            // The rescan future comes owned from the caller (the synchronous-prologue
            // pattern consumed the `&ColonyDb` borrow; the future is Send + 'static).
            let outcome = rescan_future.await;
            let reply_body = build_rescan_reply(&outcome);
            emit_reply_or_done(reply_to, reply_body)
        }
        "/colony/graph" => {
            let reply_body = build_graph_read_reply(registry, edges, &body);
            emit_reply_or_done(reply_to, reply_body)
        }
        "/colony/trace" => {
            // Not a `build_*_read_reply` helper like its three siblings: the
            // read itself is async and needs the colony DB, so the refusal is
            // taken here, before the query runs.
            let reply_body = match parse_read_query_trace_filters(&body) {
                Ok(f) => {
                    let reply = handle_read_trace(
                        db_path,
                        f.trace_id,
                        f.path_prefix,
                        f.correlation_id,
                        f.only_error,
                        f.since,
                        f.limit,
                    )
                    .await;
                    build_trace_reply(&reply)
                }
                Err(e) => refuse_read("/colony/trace", "trace", &e),
            };
            emit_reply_or_done(reply_to, reply_body)
        }
        // `/colony/events` (U4-deferred) + any unknown `/colony/<x>` →
        // ColonyEndpointUnimplemented DLQ with `sender` pass-through
        // (must-fix #2: sender from RouteAction::ColonyDispatch,
        // NOT msg.reply_to).
        _ => {
            tracing::warn!(
                endpoint = %endpoint.as_str(),
                sender = %sender.as_str(),
                "unknown /colony/<endpoint> — dead-letter ColonyEndpointUnimplemented"
            );
            let resolved = endpoint.clone();
            crate::colony::push_dead_letter(
                dead_letters,
                crate::dead_letter::DeadLetter {
                    sender_path: sender,
                    original_target: endpoint,
                    resolved_target: resolved,
                    message: msg,
                    reason: crate::dead_letter::DeadLetterReason::ColonyEndpointUnimplemented,
                },
            );
            crate::colony::RouteAction::Done
        }
    }
}

/// Variant of `handle_read_templates` without the `&ColonyDb` borrow. Filters
/// pre-extracted rows analogously to the DB version — and it IS the DB version's
/// body, so the HTTP door and the `/colony` message door filter identically.
fn handle_read_templates_from_rows(
    rows: Vec<crate::persist::colony_db::TemplateRow>,
    cell_type: Option<String>,
    name: Option<String>,
    limit: usize,
) -> crate::api_dto::ReadTemplatesReply {
    let cap = limit.clamp(1, 1000);
    let entries: Vec<crate::api_dto::TemplateEntryDto> = rows
        .into_iter()
        .filter(|r| match &name {
            Some(n) => r.name == *n,
            None => true,
        })
        // Equality on the declared type. An unknown value matches nothing and is
        // an empty list, not an error: "no template of that type" is a true and
        // useful answer, and a 4xx would make the caller guess the closed set.
        .filter(|r| match &cell_type {
            Some(t) => template_cell_type(&r.filesystem_path).as_deref() == Some(t.as_str()),
            None => true,
        })
        .take(cap)
        .map(|r| crate::api_dto::TemplateEntryDto {
            template_id: r.template_id,
            name: r.name,
            version: r.version,
            filesystem_path: r.filesystem_path,
            author: r.author,
        })
        .collect();
    crate::api_dto::ReadTemplatesReply { entries }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P1 Task 8: the DLQ read projection carries the dead-lettered message's
    /// own id, so the dead-letter view can link to the exact origin message
    /// instead of only its trace.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dead_letter_dto_carries_message_id_from_envelope() {
        use crate::persist::writer::ColonyWriteOp;

        let trace = "019ebb7e-0000-7000-8000-000000000abc";
        let msg_id = "019ebb7e-0000-7000-8000-000000000d1e";
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("c.db");
        let db = crate::ColonyDb::open(&db_path).unwrap();
        db.send_op(ColonyWriteOp::InsertDeadLetter {
            sender_path: "/sender".into(),
            original_target: "/target".into(),
            resolved_target: "/target".into(),
            error_code: "no_route".into(),
            trace_id: trace.into(),
            created_at: 100,
            message_json: format!(r#"{{"id":"{msg_id}","trace_id":"{trace}"}}"#),
        })
        .await;
        db.shutdown_async().await;

        let db2 = crate::ColonyDb::open(&db_path).unwrap();
        let reply = handle_read_dead_letters(&db2, None, None, 10);
        assert_eq!(reply.entries[0].message_id.as_deref(), Some(msg_id));
        db2.shutdown_async().await;
    }

    /// Old rows whose envelope has no `id` (or is not JSON at all) must NOT
    /// fail the read — they yield `None` and the view falls back to the
    /// trace-level link.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dead_letter_dto_tolerates_rows_without_message_id() {
        use crate::persist::writer::ColonyWriteOp;

        let trace = "019ebb7e-0000-7000-8000-000000000abc";
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("c.db");
        let db = crate::ColonyDb::open(&db_path).unwrap();
        for (ts, envelope) in [
            (100i64, r#"{"target":"/b"}"#.to_string()),
            (200, "not json at all".to_string()),
        ] {
            db.send_op(ColonyWriteOp::InsertDeadLetter {
                sender_path: "/sender".into(),
                original_target: "/target".into(),
                resolved_target: "/target".into(),
                error_code: "no_route".into(),
                trace_id: trace.into(),
                created_at: ts,
                message_json: envelope,
            })
            .await;
        }
        db.shutdown_async().await;

        let db2 = crate::ColonyDb::open(&db_path).unwrap();
        let reply = handle_read_dead_letters(&db2, None, None, 10);
        assert_eq!(reply.entries.len(), 2, "both rows survive the read");
        assert!(
            reply.entries.iter().all(|e| e.message_id.is_none()),
            "no id in the envelope yields None, never an error"
        );
        db2.shutdown_async().await;
    }

    /// P1 test fixture: write one `message_log` row through the real writer op
    /// (no hand-rolled SQL — the row shape stays honest against the DDL).
    #[allow(clippy::too_many_arguments)]
    async fn insert_log_row(
        db: &crate::ColonyDb,
        id: &str,
        created_at: i64,
        from: &str,
        to: &str,
        trace_id: &str,
        parent: Option<&str>,
        correlation: Option<&str>,
        body_kind: &str,
        body_payload: Option<&str>,
    ) {
        db.send_op(crate::persist::writer::ColonyWriteOp::InsertMessageLog(
            crate::persist::writer::MessageLogRow {
                id: id.into(),
                trace_id: trace_id.into(),
                parent_message_id: parent.map(|s| s.into()),
                correlation_id: correlation.map(|s| s.into()),
                ttl: 32,
                from_path: from.into(),
                to_path: to.into(),
                reply_to: None,
                headers_json: "{}".into(),
                body_kind: body_kind.into(),
                body_payload: body_payload.map(|s| s.into()),
                created_at,
            },
        ))
        .await;
    }

    /// Shorthand for the common case: inline body, no parent/correlation.
    async fn insert_simple_row(
        db: &crate::ColonyDb,
        id: &str,
        created_at: i64,
        from: &str,
        to: &str,
    ) {
        insert_log_row(
            db,
            id,
            created_at,
            from,
            to,
            "019ebb7e-0000-7000-8000-0000000000ff",
            None,
            None,
            "inline",
            Some(r#"{"messages":[]}"#),
        )
        .await;
    }

    /// P1 Task 3a: newest-first ordering, limit, and the keyset cursor a full
    /// page hands out.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn read_messages_returns_newest_first_and_respects_limit() {
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("c.db");
        let db = crate::ColonyDb::open(&db_path).unwrap();
        for (id, ts) in [("m1", 100i64), ("m2", 200), ("m3", 300)] {
            insert_simple_row(&db, id, ts, "/a", "/b").await;
        }
        db.shutdown_async().await;

        let filter = crate::api_dto::MessageLogFilter {
            limit: 2,
            scan_budget: 5000,
            ..Default::default()
        };
        let reply = handle_read_messages(&db_path, filter).await;

        assert_eq!(reply.entries.len(), 2, "limit honoured");
        assert_eq!(reply.entries[0].id, "m3", "newest first");
        assert_eq!(reply.entries[1].id, "m2");
        let cursor = reply.next.expect("full page yields a cursor");
        assert_eq!(cursor.id, "m2");
        assert_eq!(cursor.created_at, 200);
        assert!(!reply.scan_truncated);
        assert_eq!(reply.scan_budget, 5000);
    }

    /// P1 Task 3b: the keyset cursor excludes its own row and breaks
    /// `created_at` ties by `id`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn read_messages_cursor_returns_strictly_older_rows() {
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("c.db");
        let db = crate::ColonyDb::open(&db_path).unwrap();
        insert_simple_row(&db, "m1", 100, "/a", "/b").await;
        insert_simple_row(&db, "m2", 200, "/a", "/b").await;
        insert_simple_row(&db, "m3", 200, "/a", "/b").await; // shares created_at with m2
        db.shutdown_async().await;

        let filter = crate::api_dto::MessageLogFilter {
            limit: 10,
            scan_budget: 5000,
            before: Some(crate::api_dto::MessageLogCursor {
                created_at: 200,
                id: "m3".into(),
            }),
            ..Default::default()
        };
        let reply = handle_read_messages(&db_path, filter).await;
        let ids: Vec<&str> = reply.entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["m2", "m1"],
            "cursor row itself excluded, tie broken by id"
        );
        assert!(reply.next.is_none(), "partial page yields no cursor");
    }

    /// P1 Task 3c: indexed predicate (`to_path` prefix) and residual predicate
    /// (`from_path` prefix) both filter.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn read_messages_filters_by_indexed_and_residual_predicates() {
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("c.db");
        let db = crate::ColonyDb::open(&db_path).unwrap();
        insert_simple_row(&db, "m1", 100, "/src/one", "/mem/a").await;
        insert_simple_row(&db, "m2", 200, "/other", "/mem/b").await;
        insert_simple_row(&db, "m3", 300, "/src/two", "/elsewhere").await;
        db.shutdown_async().await;

        let by_to = handle_read_messages(
            &db_path,
            crate::api_dto::MessageLogFilter {
                to_path_prefix: Some("/mem".into()),
                limit: 10,
                scan_budget: 5000,
                ..Default::default()
            },
        )
        .await;
        let ids: Vec<&str> = by_to.entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["m2", "m1"], "indexed to_path prefix");

        let by_from = handle_read_messages(
            &db_path,
            crate::api_dto::MessageLogFilter {
                from_path_prefix: Some("/src".into()),
                limit: 10,
                scan_budget: 5000,
                ..Default::default()
            },
        )
        .await;
        let ids: Vec<&str> = by_from.entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["m3", "m1"], "residual from_path prefix");
    }

    /// P1 Task 3c: `body_kind` + `correlation_id` are residual predicates too.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn read_messages_filters_by_body_kind_and_correlation() {
        let corr = "019ebb7e-0000-7000-8000-000000000c07";
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("c.db");
        let db = crate::ColonyDb::open(&db_path).unwrap();
        insert_log_row(
            &db,
            "inline1",
            100,
            "/a",
            "/b",
            "019ebb7e-0000-7000-8000-0000000000ff",
            None,
            Some(corr),
            "inline",
            Some("{}"),
        )
        .await;
        insert_log_row(
            &db,
            "blob1",
            200,
            "/a",
            "/b",
            "019ebb7e-0000-7000-8000-0000000000ff",
            None,
            None,
            "blob",
            Some("019ebb7e-0000-7000-8000-00000000b10b"),
        )
        .await;
        db.shutdown_async().await;

        let blobs = handle_read_messages(
            &db_path,
            crate::api_dto::MessageLogFilter {
                body_kind: Some("blob".into()),
                limit: 10,
                scan_budget: 5000,
                ..Default::default()
            },
        )
        .await;
        let ids: Vec<&str> = blobs.entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["blob1"]);

        let correlated = handle_read_messages(
            &db_path,
            crate::api_dto::MessageLogFilter {
                correlation_id: Some(corr.into()),
                limit: 10,
                scan_budget: 5000,
                ..Default::default()
            },
        )
        .await;
        let ids: Vec<&str> = correlated.entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["inline1"]);
    }

    /// P1 Task 3c: an exhausted scan budget is reported, never silently
    /// swallowed — the residual filter only saw the scanned window.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn read_messages_flags_truncation_when_scan_budget_exhausted() {
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("c.db");
        let db = crate::ColonyDb::open(&db_path).unwrap();
        for i in 0..5 {
            insert_simple_row(&db, &format!("m{i}"), 100 + i, "/noise", "/b").await;
        }
        insert_simple_row(&db, "target", 1, "/wanted", "/b").await; // oldest row
        db.shutdown_async().await;

        let reply = handle_read_messages(
            &db_path,
            crate::api_dto::MessageLogFilter {
                from_path_prefix: Some("/wanted".into()),
                limit: 10,
                scan_budget: 3, // window does not reach "target"
                ..Default::default()
            },
        )
        .await;
        assert!(
            reply.entries.is_empty(),
            "residual filter only sees the scanned window"
        );
        assert!(
            reply.scan_truncated,
            "an exhausted budget must be reported to the caller"
        );
        assert_eq!(reply.scan_budget, 3);
    }

    /// P1 Task 3d: `id` is a PRIMARY-KEY lookup that overrides every other filter.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn read_messages_by_id_returns_exactly_one_row() {
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("c.db");
        let db = crate::ColonyDb::open(&db_path).unwrap();
        insert_simple_row(&db, "m1", 100, "/a", "/b").await;
        insert_simple_row(&db, "m2", 200, "/a", "/b").await;
        db.shutdown_async().await;

        let reply = handle_read_messages(
            &db_path,
            crate::api_dto::MessageLogFilter {
                id: Some("m1".into()),
                to_path_prefix: Some("/zzz".into()), // competing filter is ignored
                limit: 10,
                scan_budget: 5000,
                ..Default::default()
            },
        )
        .await;
        assert_eq!(reply.entries.len(), 1);
        assert_eq!(reply.entries[0].id, "m1");
        assert!(reply.next.is_none());
        assert!(!reply.scan_truncated);
    }

    /// P1 Task 2: prefix filters are encoded as half-open ranges so SQLite can
    /// drive them off a B-Tree index.
    #[test]
    fn prefix_range_bounds_ordinary_path() {
        let (lo, hi) = path_prefix_range("/mem");
        assert_eq!(lo, "/mem");
        assert_eq!(hi.as_deref(), Some("/men"), "last byte incremented");
    }

    #[test]
    fn prefix_range_has_no_upper_bound_for_empty_prefix() {
        let (lo, hi) = path_prefix_range("");
        assert_eq!(lo, "");
        assert!(hi.is_none(), "empty prefix matches everything");
    }

    #[test]
    fn prefix_range_excludes_sibling_outside_the_prefix() {
        // "/a" includes "/ab" (string-prefix semantics like the existing
        // LIKE filter in handle_read_trace), "/b" does not.
        let (lo, hi) = path_prefix_range("/a");
        let hi = hi.expect("successor exists");
        assert!("/ab" >= lo.as_str() && "/ab" < hi.as_str());
        assert!("/b" >= hi.as_str());
    }

    /// Phase-16 W2 (A2) + W6d (A6): the DLQ read DTO is self-locating — it carries
    /// `trace_id` + `created_at` (read off the persisted envelope), and the `since`
    /// filter excludes older entries. W6d: now read from the durable `dead_letters`
    /// table (DB source of truth) instead of an in-memory `VecDeque`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn read_dead_letters_dto_is_self_locating_and_since_filters() {
        use crate::persist::writer::ColonyWriteOp;

        let trace = "019ebb7e-0000-7000-8000-000000000abc";
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("c.db");
        let db = crate::ColonyDb::open(&db_path).unwrap();
        for ts in [100i64, 200] {
            db.send_op(ColonyWriteOp::InsertDeadLetter {
                sender_path: "/sender".into(),
                original_target: "/target".into(),
                resolved_target: "/target".into(),
                error_code: "no_route".into(),
                trace_id: trace.into(),
                created_at: ts,
                message_json: format!(
                    r#"{{"trace_id":"{trace}","created_at":{ts},"target":"/target"}}"#
                ),
            })
            .await;
        }
        db.shutdown_async().await;

        let db2 = crate::ColonyDb::open(&db_path).unwrap();
        let reply = handle_read_dead_letters(&db2, Some(150), None, 1000);
        assert_eq!(
            reply.entries.len(),
            1,
            "since=150 keeps only created_at>=150"
        );
        let dto = &reply.entries[0];
        assert_eq!(dto.created_at, 200);
        assert_eq!(dto.trace_id, trace);
        assert_eq!(dto.sender_path, "/sender");
        assert_eq!(dto.error_code, "no_route");
        db2.shutdown_async().await;
    }

    /// Phase-13.5-A6-T3 failing-test-first: pins the `sender` pass-through behaviour
    /// (must-fix #2) for unknown `/colony/<x>` endpoints.
    ///
    /// Proves:
    /// - `RouteAction::Done` returned (terminal, no cascade loop).
    /// - Exactly 1 DLQ entry with `ColonyEndpointUnimplemented`.
    /// - `dlq.sender_path == "/probe"` (sender from RouteAction::ColonyDispatch).
    /// - `dlq.resolved_target == "/colony/bogus"` (endpoint).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dispatch_unknown_endpoint_returns_done_with_dlq_push() {
        use crate::CellFactoryRegistry;
        use crate::edge_table::EdgeTable;
        use crate::persist::colony_db::ColonyDb;
        use meclaw_core::{Body, MessageBuilder, Path};
        use std::collections::{HashMap, VecDeque};

        let td = tempfile::TempDir::new().expect("tempdir");
        let colony_db = ColonyDb::open(&td.path().join("c.db")).expect("open colony.db");
        let mut registry = HashMap::new();
        let mut edges = EdgeTable::new();
        let mut dead_letters = VecDeque::new();
        let factories: CellFactoryRegistry = HashMap::new();
        let templates_snapshot = crate::templates::TemplatesRegistry::from_entries(Vec::new());

        let (inbox_self_tx, _inbox_self_rx) = tokio::sync::mpsc::channel(8);
        let (outputs_tx, _outputs_rx) = tokio::sync::mpsc::channel(8);

        let msg = MessageBuilder::new(Path::new("/colony/bogus"))
            .body(Body::Inline(meclaw_core::serde_json::json!({})))
            .build();

        let templates_rows = colony_db.read_templates().unwrap_or_default();
        let rescan_future = Box::pin(handle_rescan_templates(&colony_db, td.path()));
        let db_path = colony_db.db_path().to_path_buf();

        let action = dispatch_colony_endpoint(
            &mut registry,
            &mut crate::hive_scope::HiveScopeTable::new(),
            &mut edges,
            &mut HashMap::new(), // node_contracts — empty: no mutation endpoint in this test
            &mut dead_letters,
            &colony_db.writer_tx,
            &db_path,
            templates_snapshot,
            templates_rows,
            rescan_future,
            &factories,
            td.path(),
            &inbox_self_tx,
            &outputs_tx,
            Path::new("/colony/bogus"),
            msg,
            Path::new("/probe"),
            60_000,
            60_000,
            1000,
            false,
            None,
            0,
            None,
        )
        .await;

        assert!(matches!(action, crate::colony::RouteAction::Done));
        assert_eq!(dead_letters.len(), 1);
        let dlq = &dead_letters[0];
        assert!(matches!(
            dlq.reason,
            crate::dead_letter::DeadLetterReason::ColonyEndpointUnimplemented
        ));
        assert_eq!(dlq.sender_path.as_str(), "/probe");
        assert_eq!(dlq.resolved_target.as_str(), "/colony/bogus");
        assert_eq!(dlq.original_target.as_str(), "/colony/bogus");

        colony_db.shutdown_async().await;
    }

    /// Phase-13.5-A6-T4 F4 pin: cell→/colony/mutations expects the body as
    /// UBF top-level slots {"diff": {...}, "scope": "...", "ctx": {...}}.
    /// NO messages[] array, NO tool_call turn (that would follow the HTTP shape,
    /// not the cell convention).
    ///
    /// Spec Z.221 + Z.412 do not say this explicitly — A6 pin, clarification
    /// → phase-16 doc-audit backlog.
    #[test]
    fn dispatch_mutations_body_form_is_diff_scope_ctx_top_level_f4() {
        let body = meclaw_core::serde_json::json!({
            "diff": { "add_edges": [{"from": "/a", "to": "/b"}] },
            "scope": "/main",
            "ctx": {}
        });
        assert!(
            body.get("diff").and_then(|v| v.as_object()).is_some(),
            "F4-PIN: mutations body has top-level 'diff' object"
        );
        assert!(
            body.get("scope").and_then(|v| v.as_str()).is_some(),
            "F4-PIN: mutations body has top-level 'scope' string"
        );
        assert!(
            body.get("ctx").and_then(|v| v.as_object()).is_some(),
            "F4-PIN: mutations body has top-level 'ctx' object"
        );
    }

    /// Phase-13.5-A6-T4 F4 pin: cell→/colony/<read> expects the filter under
    /// body.query. Spec Z.221 + Z.412 do not say this — A6 pin, clarification
    /// → phase-16 doc-audit backlog.
    #[test]
    fn dispatch_reads_body_form_is_query_top_level_f4() {
        let body = meclaw_core::serde_json::json!({
            "query": { "path_prefix": "/main", "limit": 50 }
        });
        let q = body
            .get("query")
            .and_then(|v| v.as_object())
            .expect("query object");
        assert_eq!(q.get("path_prefix").and_then(|v| v.as_str()), Some("/main"));
        assert_eq!(q.get("limit").and_then(|v| v.as_u64()), Some(50));
    }

    /// Phase-13.5-A6-T4 F7 pin: /colony/dead_letters with body.operation="drain"
    /// → drain. Otherwise (e.g. body.operation="read" or absent) → read.
    /// Spec Z.401 says "both (read + drain)" without a body form — A6 pin,
    /// clarification → phase-16 doc-audit backlog.
    #[test]
    fn dispatch_dead_letters_drain_marker_is_body_operation_f7() {
        let drain_body = meclaw_core::serde_json::json!({"operation": "drain"});
        let op = drain_body
            .get("operation")
            .and_then(|v| v.as_str())
            .unwrap_or("read");
        assert_eq!(op, "drain", "F7-PIN: drain marker is body.operation");

        let read_body = meclaw_core::serde_json::json!({"query": {"limit": 10}});
        let op = read_body
            .get("operation")
            .and_then(|v| v.as_str())
            .unwrap_or("read");
        assert_eq!(op, "read", "F7-PIN: default operation is read");
    }

    /// Build a minimal `RegistryEntry` with the given `path`/`active` for
    /// filter-unit-tests. Closures are inert (`unreachable!`) — these tests
    /// never spawn/wake a cell, they only exercise the read-projection.
    fn stub_entry(path: &meclaw_core::Path, active: bool, failed: bool) -> crate::RegistryEntry {
        let (sender, _receiver) = tokio::sync::mpsc::channel::<meclaw_core::Message>(1);
        crate::RegistryEntry {
            handle: meclaw_core::ActorHandle::new(path.clone(), sender),
            respawn: Box::new(|| unreachable!()),
            wake: None,
            restart_count: 0,
            restart_limit: 5,
            cell_id: meclaw_core::Uuid::now_v7(),
            cell_type: "echo".into(),
            status: crate::colony::CellStatus::Awake,
            eager_on_reconnect: true,
            active,
            failed,
            stop_tx: None,
            death_ack_rx: None,
        }
    }

    /// Phase-13.5-lifecycle-3b T8.3 (F7): `?active=true` keeps only `active`
    /// entries, `?active=false` only inactive, `None` keeps all. This is the
    /// filter core shared by BOTH the HTTP-Inbox path (`ColonyMsg::ReadRegistry`
    /// arm) and the cell→/colony read path (`dispatch_colony_endpoint`).
    #[test]
    fn handle_read_registry_active_filter_is_symmetric() {
        let mut registry = std::collections::HashMap::new();
        let live = meclaw_core::Path::new("/live");
        let dead = meclaw_core::Path::new("/dead");
        registry.insert(live.clone(), stub_entry(&live, true, false));
        registry.insert(dead.clone(), stub_entry(&dead, false, false));

        let only_active = handle_read_registry(&registry, None, None, None, Some(true), 100);
        let paths: Vec<&str> = only_active
            .entries
            .iter()
            .map(|e| e.path.as_str())
            .collect();
        assert_eq!(paths, vec!["/live"], "Some(true) keeps only active");

        let only_inactive = handle_read_registry(&registry, None, None, None, Some(false), 100);
        let paths: Vec<&str> = only_inactive
            .entries
            .iter()
            .map(|e| e.path.as_str())
            .collect();
        assert_eq!(paths, vec!["/dead"], "Some(false) keeps only inactive");

        let all = handle_read_registry(&registry, None, None, None, None, 100);
        assert_eq!(all.entries.len(), 2, "None keeps all");
    }

    /// Paket 6 / B2: `RegistryEntryDto.failed` is projected from
    /// `RegistryEntry.failed` and makes a failed cell (`active:false,
    /// failed:true`) distinguishable from a merely disconnected one
    /// (`active:false, failed:false`) — both share `active:false`.
    #[test]
    fn handle_read_registry_exposes_failed_distinct_from_inactive() {
        let mut registry = std::collections::HashMap::new();
        let live = meclaw_core::Path::new("/live");
        let disconnected = meclaw_core::Path::new("/disconnected");
        let failed = meclaw_core::Path::new("/failed");
        registry.insert(live.clone(), stub_entry(&live, true, false));
        registry.insert(
            disconnected.clone(),
            stub_entry(&disconnected, false, false),
        );
        registry.insert(failed.clone(), stub_entry(&failed, false, true));

        let all = handle_read_registry(&registry, None, None, None, None, 100);
        let failed_dto = all
            .entries
            .iter()
            .find(|e| e.path == "/failed")
            .expect("failed cell present");
        let disconnected_dto = all
            .entries
            .iter()
            .find(|e| e.path == "/disconnected")
            .expect("disconnected cell present");

        assert!(failed_dto.failed, "failed cell has failed:true");
        assert!(!failed_dto.active, "failed cell has active:false");
        assert!(
            !disconnected_dto.failed,
            "disconnected cell has failed:false"
        );
        assert!(
            !disconnected_dto.active,
            "disconnected cell has active:false"
        );
        assert_ne!(
            failed_dto.failed, disconnected_dto.failed,
            "failed vs. disconnected are distinguishable despite both active:false"
        );
    }

    /// Paket 6 / B3: the existing `?active=` filter already covers failed cells
    /// (failed ⟹ active=false) — no separate `?failed=` filter. `?active=false`
    /// lists the failed cell; `?active=true` does not. Regression-pin only.
    #[test]
    fn handle_read_registry_active_false_includes_failed() {
        let mut registry = std::collections::HashMap::new();
        let failed = meclaw_core::Path::new("/failed");
        registry.insert(failed.clone(), stub_entry(&failed, false, true));

        let inactive = handle_read_registry(&registry, None, None, None, Some(false), 100);
        let paths: Vec<&str> = inactive.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["/failed"],
            "active=false includes the failed cell"
        );

        let only_active = handle_read_registry(&registry, None, None, None, Some(true), 100);
        assert!(
            only_active.entries.is_empty(),
            "active=true excludes the failed cell"
        );
    }

    /// Phase-13.5-lifecycle-3b T8.3 (F7): cell→/colony/registry read path —
    /// `body.query.active` flows through `parse_read_query_path_filters` into
    /// `handle_read_registry`. Proves the filter is symmetric for the cell-emit
    /// path (mirrors the HTTP-path proof in the unit test above).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dispatch_registry_active_filter_via_cell_path() {
        use crate::CellFactoryRegistry;
        use crate::edge_table::EdgeTable;
        use crate::persist::colony_db::ColonyDb;
        use meclaw_core::{Body, MessageBuilder, Path};
        use std::collections::{HashMap, VecDeque};

        let td = tempfile::TempDir::new().expect("tempdir");
        let colony_db = ColonyDb::open(&td.path().join("c.db")).expect("open colony.db");
        let mut registry = HashMap::new();
        let live = Path::new("/live");
        let dead = Path::new("/dead");
        registry.insert(live.clone(), stub_entry(&live, true, false));
        registry.insert(dead.clone(), stub_entry(&dead, false, false));
        let mut edges = EdgeTable::new();
        let mut dead_letters = VecDeque::new();
        let factories: CellFactoryRegistry = HashMap::new();
        let templates_snapshot = crate::templates::TemplatesRegistry::from_entries(Vec::new());

        let (inbox_self_tx, _inbox_self_rx) = tokio::sync::mpsc::channel(8);
        let (outputs_tx, mut outputs_rx) = tokio::sync::mpsc::channel(8);

        // Cell-emit form: filter lives under `body.query`, reply_to set so the
        // dispatcher cascades a reply we can inspect.
        let msg = MessageBuilder::new(Path::new("/colony/registry"))
            .body(Body::Inline(meclaw_core::serde_json::json!({
                "query": { "active": true }
            })))
            .reply_to(Path::new("/probe"))
            .build();

        let templates_rows = colony_db.read_templates().unwrap_or_default();
        let rescan_future = Box::pin(handle_rescan_templates(&colony_db, td.path()));
        let db_path = colony_db.db_path().to_path_buf();

        let action = dispatch_colony_endpoint(
            &mut registry,
            &mut crate::hive_scope::HiveScopeTable::new(),
            &mut edges,
            &mut HashMap::new(), // node_contracts — empty: no mutation endpoint in this test
            &mut dead_letters,
            &colony_db.writer_tx,
            &db_path,
            templates_snapshot,
            templates_rows,
            rescan_future,
            &factories,
            td.path(),
            &inbox_self_tx,
            &outputs_tx,
            Path::new("/colony/registry"),
            msg,
            Path::new("/probe"),
            60_000,
            60_000,
            1000,
            false,
            None,
            0,
            None,
        )
        .await;

        // Reply cascades back to /probe; extract the registry entries from it.
        match action {
            crate::colony::RouteAction::Cascade { msg, .. } => {
                let body = match msg.body {
                    Body::Inline(v) => v,
                    other => panic!("expected inline reply body, got {other:?}"),
                };
                let entries = body
                    .get("registry")
                    .and_then(|v| v.as_array())
                    .expect("reply has registry array");
                let paths: Vec<&str> = entries
                    .iter()
                    .filter_map(|e| e.get("path").and_then(|v| v.as_str()))
                    .collect();
                assert_eq!(
                    paths,
                    vec!["/live"],
                    "cell-path ?active=true keeps only active"
                );
            }
            _ => panic!("expected Cascade reply from /colony/registry dispatch"),
        }
        let _ = outputs_rx.try_recv();

        colony_db.shutdown_async().await;
    }

    /// GH #359 — the wiring proof for `/colony/trace`. Its three siblings answer
    /// through a pure `build_*_read_reply` helper that the integration test
    /// drives directly; the trace read is async and needs the colony DB, so its
    /// refusal is taken inside the dispatcher and has to be pinned there.
    ///
    /// A syntactically broken UUID used to become "no filter": a caller asking
    /// for ONE trace got the newest 100 entries of EVERY trace back.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dispatch_trace_refuses_a_broken_uuid_instead_of_reading_every_trace() {
        use crate::CellFactoryRegistry;
        use crate::edge_table::EdgeTable;
        use crate::persist::colony_db::ColonyDb;
        use meclaw_core::{Body, MessageBuilder, Path};
        use std::collections::{HashMap, VecDeque};

        let td = tempfile::TempDir::new().expect("tempdir");
        let colony_db = ColonyDb::open(&td.path().join("c.db")).expect("open colony.db");
        let mut registry = HashMap::new();
        let mut edges = EdgeTable::new();
        let mut dead_letters = VecDeque::new();
        let factories: CellFactoryRegistry = HashMap::new();
        let templates_snapshot = crate::templates::TemplatesRegistry::from_entries(Vec::new());

        let (inbox_self_tx, _inbox_self_rx) = tokio::sync::mpsc::channel(8);
        let (outputs_tx, mut outputs_rx) = tokio::sync::mpsc::channel(8);

        let msg = MessageBuilder::new(Path::new("/colony/trace"))
            .body(Body::Inline(meclaw_core::serde_json::json!({
                "query": { "trace_id": "not-a-uuid" }
            })))
            .reply_to(Path::new("/probe"))
            .build();

        let templates_rows = colony_db.read_templates().unwrap_or_default();
        let rescan_future = Box::pin(handle_rescan_templates(&colony_db, td.path()));
        let db_path = colony_db.db_path().to_path_buf();

        let action = dispatch_colony_endpoint(
            &mut registry,
            &mut crate::hive_scope::HiveScopeTable::new(),
            &mut edges,
            &mut HashMap::new(),
            &mut dead_letters,
            &colony_db.writer_tx,
            &db_path,
            templates_snapshot,
            templates_rows,
            rescan_future,
            &factories,
            td.path(),
            &inbox_self_tx,
            &outputs_tx,
            Path::new("/colony/trace"),
            msg,
            Path::new("/probe"),
            60_000,
            60_000,
            1000,
            false,
            None,
            0,
            None,
        )
        .await;

        match action {
            crate::colony::RouteAction::Cascade { msg, .. } => {
                let body = match msg.body {
                    Body::Inline(v) => v,
                    other => panic!("expected inline reply body, got {other:?}"),
                };
                assert!(
                    body["trace"].as_array().is_none(),
                    "a refused filter must not answer a trace list: {body}"
                );
                assert_eq!(body["trace"]["status"], "error");
                assert_eq!(body["trace"]["error_code"], "invalid_query");
            }
            _ => panic!("expected Cascade reply from /colony/trace dispatch"),
        }
        let _ = outputs_rx.try_recv();

        colony_db.shutdown_async().await;
    }

    // ---- W13 hardening: the `?type=` filter -----------------------------------

    /// Build a template directory carrying a `config.json` of the given type,
    /// and the `TemplateRow` that a scan would have recorded for it.
    fn template_row(
        root: &std::path::Path,
        name: &str,
        cell_type: &str,
    ) -> crate::persist::colony_db::TemplateRow {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.json"),
            format!(r#"{{"cell":{{"type":"{cell_type}"}},"params":{{}}}}"#),
        )
        .unwrap();
        crate::persist::colony_db::TemplateRow {
            template_id: format!("id-{name}"),
            name: name.into(),
            version: None,
            filesystem_path: dir.display().to_string(),
            description_json: "null".into(),
            tags_json: "[]".into(),
            author: None,
            scanned_at: 0,
        }
    }

    fn names_of(reply: &crate::api_dto::ReadTemplatesReply) -> Vec<&str> {
        reply.entries.iter().map(|e| e.name.as_str()).collect()
    }

    /// The `?type=` filter was a silent no-op: the DTO carried the field, the
    /// UI's own filter form sent it, and the handler dropped it — so a caller
    /// asking for `type=store` got every template back and had no way to tell.
    #[test]
    fn read_templates_filters_on_the_declared_cell_type() {
        let td = tempfile::TempDir::new().unwrap();
        let rows = vec![
            template_row(td.path(), "echo-basic", "echo"),
            template_row(td.path(), "memory", "store"),
            template_row(td.path(), "chat", "llm"),
        ];

        let all = handle_read_templates_from_rows(rows.clone(), None, None, 100);
        assert_eq!(names_of(&all).len(), 3, "no filter lists everything");

        let stores = handle_read_templates_from_rows(rows.clone(), Some("store".into()), None, 100);
        assert_eq!(names_of(&stores), vec!["memory"]);

        // An unknown type is an empty list, not an error — the caller must not
        // have to guess a closed set to get a well-formed answer.
        let none = handle_read_templates_from_rows(rows.clone(), Some("nosuch".into()), None, 100);
        assert!(none.entries.is_empty(), "unknown type yields an empty list");

        // …and the two filters compose rather than override each other.
        let both = handle_read_templates_from_rows(
            rows.clone(),
            Some("echo".into()),
            Some("echo-basic".into()),
            100,
        );
        assert_eq!(names_of(&both), vec!["echo-basic"]);
        let contradictory =
            handle_read_templates_from_rows(rows, Some("llm".into()), Some("memory".into()), 100);
        assert!(contradictory.entries.is_empty());
    }

    /// A template directory the filter cannot classify (no `config.json`, or an
    /// unreadable one) is a non-match, never a failed read.
    #[test]
    fn an_unclassifiable_template_is_a_non_match_not_an_error() {
        let td = tempfile::TempDir::new().unwrap();
        let mut orphan = template_row(td.path(), "orphan", "echo");
        orphan.filesystem_path = td.path().join("does-not-exist").display().to_string();
        let rows = vec![orphan, template_row(td.path(), "real", "echo")];

        assert_eq!(
            names_of(&handle_read_templates_from_rows(
                rows.clone(),
                Some("echo".into()),
                None,
                100
            )),
            vec!["real"]
        );
        // Without the filter it is still listed: the classification is only ever
        // asked for when someone asks for it.
        assert_eq!(
            handle_read_templates_from_rows(rows, None, None, 100)
                .entries
                .len(),
            2
        );
    }
}
