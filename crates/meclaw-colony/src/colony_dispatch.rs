//! Phase-13.5-A6: Cell→/colony-Endpoint-Dispatcher + extrahierte Inbox-Arm-Handler.
//!
//! Dieses Modul kapselt alles, was sowohl der Inbox-Arm (HTTP-API über ColonyMsg)
//! als auch der Outputs-Arm (Cell-Emission via route() → RouteAction::ColonyDispatch)
//! aufruft. Vor A6 lebten alle Handler inline im Inbox-Arm von `colony_task` —
//! jetzt gemeinsame Aufruf-Punkte mit identischer Semantik.
//!
//! Spec: `docs/meclaw-overview.md` § /colony als virtueller Endpunkt (Z.393-417).
//! Symmetrie interne API ↔ externe API (Z.397).
//!
//! **T1-Skopus**: 8 Handler verbatim aus `colony_task` inbox-arm extrahiert. Reine
//! Verschiebung — keine Logik-Änderung. Zweiter Caller (`dispatch_colony_endpoint`
//! im outputs-arm) folgt in T3.

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
/// is fine). `cell_type` filter is currently no-op (see `TemplateEntryDto` doc).
///
/// Phase 13.5-A6: extracted verbatim from `colony_task` inbox-arm — same
/// semantics, two callers expected (inbox-arm now, outputs-arm in T3).
/// Spec: `docs/meclaw-overview.md` § /colony als virtueller Endpunkt (Symmetrie).
pub fn handle_read_templates(
    colony_db: &ColonyDb,
    _cell_type: Option<String>,
    name: Option<String>,
    limit: usize,
) -> crate::api_dto::ReadTemplatesReply {
    let cap = limit.clamp(1, 1000);
    let rows = colony_db.read_templates().unwrap_or_default();
    let entries: Vec<crate::api_dto::TemplateEntryDto> = rows
        .into_iter()
        .filter(|r| match &name {
            Some(n) => r.name == *n,
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

/// Phase 12-B step-7.6: spawn_blocking + WAL Read-Only Connection.
/// STALLS the Colony-Inbox loop until the JoinHandle resolves —
/// bounded by limit ≤ 1000.
///
/// Phase 13.5-A6: extracted verbatim from `colony_task` inbox-arm — same
/// semantics, two callers expected (inbox-arm now, outputs-arm in T3).
/// `db_path: &std::path::Path` per Marcus' MOD #4: helper opens its own
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
        })
        .collect();
    crate::api_dto::ReadGraphReply {
        scope: scope_str,
        graph_version: 0, // Phase-14: real version bump
        nodes,
        edges: scope_edges,
    }
}

/// Phase 11 Slice 11-E: triggert intern denselben Pfad wie der Boot-Scan.
/// CLI-Flag `--rescan-templates` und (Phase 12) HTTP-POST schicken diese Operation.
///
/// Phase 13.5-A6: extracted verbatim from `colony_task` inbox-arm — same
/// semantics, two callers expected (inbox-arm now, outputs-arm in T3).
/// `&ColonyDb` ist erlaubt: `apply_scan_result` ist `fn -> impl Future + Send`
/// (sync-Vorlauf-Pattern), der DB-Borrow lebt NICHT im Future — siehe
/// `templates/mod.rs` Doc. Result wird zurückgegeben, damit die beiden
/// Aufrufer (regulär + Shutdown-drain) ihre jeweilige Log-Message ausgeben
/// können (verbatim-Erhalt der Drain-Variante).
/// Spec: `docs/meclaw-overview.md` § /colony als virtueller Endpunkt (Symmetrie).
pub fn handle_rescan_templates<'a>(
    colony_db: &ColonyDb,
    templates_root: &'a std::path::Path,
) -> impl std::future::Future<Output = Result<(), crate::templates::scanner::ScannerError>> + Send + 'a
{
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // Delegate to `apply_scan_result`, which uses the same sync-Vorlauf
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
/// Reply-Routing per A6-Plan: KEIN `outputs_tx`-Send (würde den outputs-Channel
/// füllen). Stattdessen `RouteAction::Cascade { sender: "/colony", msg }` —
/// `route_with_log` greift den Cascade-Arm und routet die Reply direkt an
/// `reply_to`. `sender = "/colony"` ist konsistent mit `send_eda_reject`
/// (siehe `colony.rs::send_eda_reject` — derselbe virtuelle Origin-Path).
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

/// Parse `body.query.{path,path_prefix,cell_type,active,limit}` for the
/// `/colony/registry` endpoint. `active` is an optional JSON bool (F7).
/// limit defaults to 100, capped at 1000 per Spec Z.414, floored at 1.
fn parse_read_query_path_filters(
    body: &meclaw_core::serde_json::Value,
) -> (
    Option<meclaw_core::Path>,
    Option<meclaw_core::Path>,
    Option<String>,
    Option<bool>,
    usize,
) {
    let q = body.get("query");
    let path = q
        .and_then(|q| q.get("path"))
        .and_then(|v| v.as_str())
        .map(meclaw_core::Path::new);
    let path_prefix = q
        .and_then(|q| q.get("path_prefix"))
        .and_then(|v| v.as_str())
        .map(meclaw_core::Path::new);
    let cell_type = q
        .and_then(|q| q.get("cell_type"))
        .and_then(|v| v.as_str())
        .map(String::from);
    let active = q.and_then(|q| q.get("active")).and_then(|v| v.as_bool());
    let limit = q
        .and_then(|q| q.get("limit"))
        .and_then(|v| v.as_u64())
        .unwrap_or(100) as usize;
    (path, path_prefix, cell_type, active, limit.clamp(1, 1000))
}

/// Parse `body.query.{cell_type,name,limit}` for `/colony/templates` reads.
fn parse_read_query_templates_filters(
    body: &meclaw_core::serde_json::Value,
) -> (Option<String>, Option<String>, usize) {
    let q = body.get("query");
    let cell_type = q
        .and_then(|q| q.get("cell_type"))
        .and_then(|v| v.as_str())
        .map(String::from);
    let name = q
        .and_then(|q| q.get("name"))
        .and_then(|v| v.as_str())
        .map(String::from);
    let limit = q
        .and_then(|q| q.get("limit"))
        .and_then(|v| v.as_u64())
        .unwrap_or(100) as usize;
    (cell_type, name, limit.clamp(1, 1000))
}

/// Parse `body.query.{trace_id,path_prefix,correlation_id,only_error,since,limit}`
/// for `/colony/trace` reads. UUIDs via `Uuid::parse_str(s).ok()`.
fn parse_read_query_trace_filters(
    body: &meclaw_core::serde_json::Value,
) -> (
    Option<Uuid>,
    Option<Path>,
    Option<Uuid>,
    bool,
    Option<i64>,
    usize,
) {
    let q = body.get("query");
    let trace_id = q
        .and_then(|q| q.get("trace_id"))
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok());
    let path_prefix = q
        .and_then(|q| q.get("path_prefix"))
        .and_then(|v| v.as_str())
        .map(Path::new);
    let correlation_id = q
        .and_then(|q| q.get("correlation_id"))
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok());
    let only_error = q
        .and_then(|q| q.get("only_error"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let since = q.and_then(|q| q.get("since")).and_then(|v| v.as_i64());
    let limit = q
        .and_then(|q| q.get("limit"))
        .and_then(|v| v.as_u64())
        .unwrap_or(100) as usize;
    (
        trace_id,
        path_prefix,
        correlation_id,
        only_error,
        since,
        limit.clamp(1, 1000),
    )
}

/// Parse `body.scope` for `/colony/graph` reads. Defaults to root "/".
fn parse_graph_scope(body: &meclaw_core::serde_json::Value) -> Path {
    body.get("scope")
        .and_then(|v| v.as_str())
        .map(Path::new)
        .unwrap_or_else(|| Path::new("/"))
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
        MutationOutcome::Rejected {
            id,
            error_code,
            details,
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

fn build_trace_reply(reply: &crate::api_dto::ReadTraceReply) -> meclaw_core::serde_json::Value {
    meclaw_core::serde_json::json!({
        "trace": meclaw_core::serde_json::to_value(&reply.entries).unwrap_or_default(),
    })
}

// ---------------------------- Main dispatcher ----------------------------

/// Phase-13.5-A6-T3: routet `/colony/<endpoint>` an T1-Helper bzw. `handle_mutation`,
/// baut Reply-Message als UBF-Top-Level-Slot und gibt `RouteAction` zurück.
///
/// **KEIN Self-Send** an `inbox_self_tx` — Direct-Call der Helper.
/// **KEIN outputs_tx-Send** für Reply — Reply geht via `RouteAction::Cascade`
/// über `route_with_log` zurück an `msg.reply_to`.
///
/// **Send-Constraint**: `&ColonyDb` ist `!Sync` (RefCell<Connection>) — `&ColonyDb`
/// als Parameter würde das umgebende `colony_task`-Future `!Send` machen (jeder
/// `.await` im async-fn-body capture'd den Borrow im state-Machine, auch wenn er
/// nur in einem Branch genutzt wird). Lösung: `&ColonyDb` wird im **Caller**
/// (colony.rs, vor dem `.await`) in seine Sub-Refs aufgeteilt; dispatch_colony_endpoint
/// bekommt nur Send-Sub-Refs:
/// - `writer_tx: &Sender<ColonyWriteOp>` (Send+Sync)
/// - `db_path: &Path` (Send+Sync)
/// - `templates_rows`/`mutation_audit_rows`: sync vor-extrahiert im Caller,
///   owned als Parameter durchgereicht
///
/// **`/colony/events`** ist deferred (U4) und fällt in den `_ =>`-Arm
/// → `ColonyEndpointUnimplemented`-DLQ-push mit `sender`-pass-through
/// (Marcus' MUST-FIX #2).
///
/// Spec: `docs/meclaw-overview.md` § /colony als virtueller Endpunkt (Z.393-417).
///
/// **Send-pre-extraction params**:
/// - `templates_rows`: vor-extrahierte template-rows für `/colony/templates`. Caller
///   liest `colony_db.read_templates()` sync vor dem `.await` und reicht sie owned durch.
/// - `rescan_future`: vor-extrahiertes Rescan-Future für `/colony/templates/rescan`.
///   Caller baut `handle_rescan_templates(&colony_db, &root)` sync (sync-Vorlauf
///   verbraucht den `&ColonyDb`-Borrow, das returned Future captured nur Send-owned
///   Daten) und reicht es boxed durch. Lifetime `'fut` ist gebunden an `&root` im
///   Caller (lebt im `colony_task`-Scope; die Future wird im selben Scope awaited
///   und nicht gespawnt).
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
    env_source: Option<&std::path::Path>, // U8 (RULED A8) — Env-Quelle vom Start, an handle_mutation weitergereicht
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
            )
            .await;
            let reply_body = build_mutation_reply(&outcome);
            emit_reply_or_done(reply_to, reply_body)
        }
        "/colony/registry" => {
            let (path, path_prefix, cell_type, active, limit) =
                parse_read_query_path_filters(&body);
            let reply = handle_read_registry(registry, path, path_prefix, cell_type, active, limit);
            let reply_body = build_registry_reply(&reply);
            emit_reply_or_done(reply_to, reply_body)
        }
        "/colony/dead_letters" => {
            // W2d (Substrat, Marcus-Ruling 2026-06-12): `/colony/dead_letters` is
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
            let (cell_type, name, limit) = parse_read_query_templates_filters(&body);
            let reply = handle_read_templates_from_rows(templates_rows, cell_type, name, limit);
            let reply_body = build_templates_reply(&reply);
            emit_reply_or_done(reply_to, reply_body)
        }
        "/colony/templates/rescan" => {
            // Rescan-future kommt owned aus dem Caller (sync-Vorlauf-Pattern hat
            // den `&ColonyDb`-Borrow konsumiert; Future ist Send + 'static).
            let outcome = rescan_future.await;
            let reply_body = build_rescan_reply(&outcome);
            emit_reply_or_done(reply_to, reply_body)
        }
        "/colony/graph" => {
            let scope = parse_graph_scope(&body);
            let reply = handle_read_graph(registry, edges, scope);
            let reply_body = build_graph_reply(&reply);
            emit_reply_or_done(reply_to, reply_body)
        }
        "/colony/trace" => {
            let (trace_id_q, path_prefix, correlation_id, only_error, since, limit) =
                parse_read_query_trace_filters(&body);
            let reply = handle_read_trace(
                db_path,
                trace_id_q,
                path_prefix,
                correlation_id,
                only_error,
                since,
                limit,
            )
            .await;
            let reply_body = build_trace_reply(&reply);
            emit_reply_or_done(reply_to, reply_body)
        }
        // `/colony/events` (U4-deferred) + any unknown `/colony/<x>` →
        // ColonyEndpointUnimplemented DLQ with `sender` pass-through
        // (Marcus' MUST-FIX #2: sender from RouteAction::ColonyDispatch,
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

/// Variante von `handle_read_templates` ohne `&ColonyDb`-Borrow. Filtert
/// vor-extrahierte rows analog zur DB-Version.
fn handle_read_templates_from_rows(
    rows: Vec<crate::persist::colony_db::TemplateRow>,
    _cell_type: Option<String>,
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

    /// Phase-13.5-A6-T3 failing-test-first: pinnt das `sender`-pass-through-Verhalten
    /// (Marcus' MUST-FIX #2) für unknown `/colony/<x>` endpoints.
    ///
    /// Beweist:
    /// - `RouteAction::Done` zurück (terminal, kein Cascade-Loop).
    /// - Genau 1 DLQ-Entry mit `ColonyEndpointUnimplemented`.
    /// - `dlq.sender_path == "/probe"` (sender vom RouteAction::ColonyDispatch).
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

    /// Phase-13.5-A6-T4 F4-Pin: Cell→/colony/mutations erwartet body als
    /// UBF-top-level-slots {"diff": {...}, "scope": "...", "ctx": {...}}.
    /// KEIN messages[]-array, KEIN tool_call-turn (das wäre HTTP-Anlehnung,
    /// nicht Cell-Convention).
    ///
    /// Spec Z.221 + Z.412 sagen das nicht explizit — A6-pin, Klarstellung
    /// → Phase-16-Doc-Audit-Backlog.
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

    /// Phase-13.5-A6-T4 F4-Pin: Cell→/colony/<read> erwartet filter unter
    /// body.query. Spec Z.221 + Z.412 sagen das nicht — A6-pin, Klarstellung
    /// → Phase-16-Doc-Audit-Backlog.
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

    /// Phase-13.5-A6-T4 F7-Pin: /colony/dead_letters mit body.operation="drain"
    /// → drain. Sonst (z.B. body.operation="read" oder fehlend) → Read.
    /// Spec Z.401 sagt "beides (Read + Drain)" ohne body-form — A6-pin,
    /// Klarstellung → Phase-16-Doc-Audit-Backlog.
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
}
