//! Schreib-Ops für `colony.db`-Writer-Thread.
//!
//! Drei Operation-Varianten:
//! - `InitialApply` — atomarer Bundle für Erst-Boot (FIX 3, review 2026-05-20):
//!   Edges + Hive-Scopes in EINER Transaktion. Schützt vor Mischzustand bei
//!   Crash-mid-first-boot.
//! - `UpsertRegistry` — pro `ColonyMsg::Register`, op-before-ack-Invariante (T22).
//! - `InsertMessageLog` — pro erfolgreichem Routing-Hop (T32 füllt das Schema mit FIX 1).
//!
//! **Phase-6-Erweiterung**: `apply_op` sammelt optionale `oneshot::Sender<()>`-Acks
//! pro Op; `run_writer` feuert sie NACH `tx.commit()` — siehe Phase-6-Plan T2.
//! `send_op` (fire-and-forget) bleibt der Default; durable Writes laufen über
//! `ColonyDb::insert_mutation_log_durable`/`update_mutation_log_durable`, die
//! eine Op mit `ack: Some(tx)` enqueuen und `rx.await` machen.

use crate::bootstrap::PlannedEdge;
use meclaw_core::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

// Phase 12-Pre: Main-Channel ist tokio::sync::mpsc::Receiver; Writer-Thread
// drained via blocking_recv() (kanonische Bridge für async-Sender +
// sync-Receiver in std::thread). std::sync::mpsc bleibt nur für die
// Phase-11-Template-Ack-Channels (mpsc_acks: Vec<std::sync::mpsc::Sender<()>>).

/// Maximale Batch-Größe pro Transaktion.
const BATCH_MAX: usize = 64;

/// Operations für den `colony.db`-Writer-Thread.
pub enum ColonyWriteOp {
    /// Atomic First-Boot bundle (FIX 3): edges + hive_scopes in einer Transaktion.
    InitialApply {
        /// Edges aus dem Bootstrap-Plan.
        edges: Vec<PlannedEdge>,
        /// Hive-Scope-Pfade aus dem Bootstrap-Plan.
        hive_scopes: Vec<Path>,
    },
    /// Registry-Upsert mit cell_id-stabiler Semantik.
    ///
    /// **Phase-13.5 Lifecycle-3b: does NOT manage the `status` column on conflict.**
    /// The first INSERT seeds `status = 'active'` (a fresh node is active), but the
    /// `ON CONFLICT(path)` path only bumps `updated_at` and leaves `status` untouched.
    /// `SetRegistryStatus` is the sole write-authority for `status` — re-registration
    /// or reboot must NOT clobber an `'inactive'` previously written by it.
    UpsertRegistry {
        /// Cell-Pfad (Primary Key).
        path: Path,
        /// UUID v7, einmal vergeben, nie überschrieben.
        cell_id: String,
        /// Cell-Type-String.
        cell_type: String,
        /// Unix-Sekunden, einmal beim Erst-Insert.
        created_at: i64,
        /// Unix-Sekunden, bumpt pro Re-Boot/Status-Touch.
        updated_at: i64,
    },
    /// Phase-13.5 Lifecycle-3b: UPDATE-only of the `registry.status` column for an
    /// existing row (`UPDATE registry SET status=?, updated_at=? WHERE path=?`).
    /// Unlike `UpsertRegistry` this is NOT an UPSERT — the row always exists per
    /// No-Delete, and `cell_id`/`cell_type`/`created_at` stay untouched. Carries
    /// the edge-derived activity (`"active"`/`"inactive"`) into persistence.
    SetRegistryStatus {
        /// Cell-Pfad (Primary Key of the row to update).
        path: Path,
        /// New status string, e.g. `"active"` or `"inactive"`.
        status: String,
        /// Unix-Sekunden des Status-Wechsels.
        updated_at: i64,
    },
    /// Message-Log-Insert (FIX-1-Felder in T32 verankert).
    InsertMessageLog(MessageLogRow),
    /// Phase 6: insert in_flight row into mutation_log; ack fires after tx.commit().
    MutationLogInsert {
        /// Mutation-ID (UUID v7).
        id: String,
        /// Mutation-Scope (Pfad-Präfix).
        scope: String,
        /// Mutation-Payload als JSON-Blob.
        payload_json: String,
        /// Unix-Sekunden bei Anlage.
        created_at: i64,
        /// Optionaler Ack-Sender; feuert nach `tx.commit()`.
        ack: Option<tokio::sync::oneshot::Sender<()>>,
    },
    /// Phase-16 W3 (A6): insert a `status='rejected'` row for a Validate-Stage
    /// reject (fire-and-forget). Unlike `MutationLogInsert` (`in_flight`, later
    /// updated to `committed`/`failed`), a reject is a single terminal INSERT:
    /// the mutation never reached Apply, so `committed_at` stays NULL. Carries
    /// the two v3 columns `error_code` + `trace_id` plus the human `reason` in
    /// `failure_reason`. Makes schema/scope/naming rejects visible in the
    /// `/colony/mutations` audit (K-H2: previously invisible).
    MutationLogRejectInsert {
        /// Mutation-ID (UUID v7).
        id: String,
        /// Mutation-Scope (Pfad-Präfix).
        scope: String,
        /// Mutation-Payload als JSON-Blob (Diagnose-Erhalt des abgelehnten Antrags).
        payload_json: String,
        /// `error_code` der Reject-Reply (z.B. `scope_out_of_bounds`).
        error_code: String,
        /// Human-readable Reason (`format!("{err:?}")`), abgelegt in `failure_reason`.
        reason: String,
        /// Trace-ID des Mutations-Antrags.
        trace_id: String,
        /// Unix-Sekunden bei Ablehnung.
        created_at: i64,
        /// Optionaler Ack-Sender; feuert nach `tx.commit()` — der Antragsteller-
        /// Reject-Pfad wartet darauf, damit die Audit-Row vor dem Return durable ist.
        ack: Option<tokio::sync::oneshot::Sender<()>>,
    },
    /// Phase 6 T21: insert an edge row (fire-and-forget; durable via FIFO ordering
    /// before the committed-`MutationLogUpdate` enqueued in the same `handle_mutation`).
    InsertEdge {
        /// Edge-UUID v7 as string (Primary Key).
        id: String,
        /// Source path (absolute).
        from: String,
        /// Target path (absolute).
        to: String,
        /// Unix-Sekunden bei Anlage.
        created_at: i64,
        /// Phase-13.5-Durable-Edges: CEL-condition als Source-String.
        /// `None` = Edge hat keine Condition (unbedingtes Routing).
        condition: Option<String>,
        /// Phase-13.5-Durable-Edges: ModifierSpec als JSON-String (set+delete).
        /// `None` = Edge hat keinen Modifier (Identity-Headers).
        modifier: Option<String>,
    },
    /// Phase 6 T21: delete an edge row by id (fire-and-forget; durable via FIFO).
    RemoveEdge {
        /// Edge-UUID v7 as string.
        id: String,
    },
    /// Phase 6: update mutation_log status (committed | failed); ack fires after tx.commit().
    MutationLogUpdate {
        /// Mutation-ID (Primary Key).
        id: String,
        /// Neuer Status: "committed" oder "failed".
        status: String,
        /// Unix-Sekunden bei Commit/Failure.
        committed_at: i64,
        /// Optional: Fehlergrund bei status="failed".
        failure_reason: Option<String>,
        /// Optionaler Ack-Sender; feuert nach `tx.commit()`.
        ack: Option<tokio::sync::oneshot::Sender<()>>,
    },
    /// Phase 11 11-A: insert or update a template row; ack fires synchronously (mpsc).
    UpsertTemplate {
        /// Template-ID (UUID v7).
        template_id: String,
        /// Template-Name (aus `template.json`).
        name: String,
        /// Optional: Semantic-Version-String.
        version: Option<String>,
        /// Absoluter Pfad zum Template-Verzeichnis.
        filesystem_path: String,
        /// `description`-Feld als JSON-Blob.
        description_json: String,
        /// `tags`-Feld als JSON-Array-String.
        tags_json: String,
        /// Optional: Autor-String.
        author: Option<String>,
        /// Unix-Sekunden beim letzten Scan.
        scanned_at: i64,
        /// Optionaler Ack-Sender; feuert synchron nach dem SQL-Execute.
        ack: Option<std::sync::mpsc::Sender<()>>,
    },
    /// Phase 11 11-A: delete a template row by template_id; ack fires synchronously.
    RemoveTemplate {
        /// Template-ID (UUID v7).
        template_id: String,
        /// Optionaler Ack-Sender; feuert synchron nach dem SQL-Execute.
        ack: Option<std::sync::mpsc::Sender<()>>,
    },
    /// Insert a single hive-scope row into `hive_scopes`.
    ///
    /// Uses `INSERT OR IGNORE` for idempotency — a hive-scope `path` is the
    /// primary key, so re-inserting an already-known scope is a no-op.
    /// This op fills the gap left by `InitialApply` (bulk, bootstrap-only):
    /// the mutation path can now add hive-scopes one at a time.
    InsertHiveScope {
        /// Absolute hive-scope path (Primary Key in `hive_scopes`).
        path: Path,
        /// Unix-seconds at creation time.
        created_at: i64,
    },
    /// Bootstrap-Recovery (Run-5/5b-Befund): durable `bootstrap_in_flight`
    /// marker into the `meta` table, written BEFORE the first-apply cell loop
    /// starts. The matching clear runs inside the `InitialApply` arm — same
    /// transaction as the edges/hive_scopes bundle, so a crash anywhere
    /// mid-apply leaves the marker behind and `probe_boot_state` classifies the
    /// next boot as a resumable `FirstBoot` instead of `Inconsistent`.
    SetBootstrapInFlight {
        /// Unix-Sekunden beim Apply-Start (Forensik-Wert des Markers).
        created_at: i64,
        /// Optionaler Ack-Sender; feuert nach `tx.commit()` (durable —
        /// the apply must not spawn before the marker is on disk).
        ack: Option<tokio::sync::oneshot::Sender<()>>,
    },
    /// Phase-16 W6d (A6): persist a single dead-letter row into the durable
    /// `dead_letters` table (fire-and-forget — a diagnostic write must never
    /// backpressure routing). The DLQ is the last diagnostic truth after the
    /// message-persistence loss; it must survive colony shutdown/crash. The six
    /// fields mirror `DeadLetterDto` (3 paths + error_code + trace_id +
    /// created_at); the `Message` envelope is NOT stored here (the `message_log`
    /// carries it separately if ever needed).
    InsertDeadLetter {
        /// Emitting cell/hive path.
        sender_path: String,
        /// Target before path resolution.
        original_target: String,
        /// Target after `Path::resolve`.
        resolved_target: String,
        /// Canonical `error_code` (per `DeadLetterReason::as_code`).
        error_code: String,
        /// Trace-ID of the dead-lettered message.
        trace_id: String,
        /// Unix-seconds (off the dead-lettered message envelope).
        created_at: i64,
        /// Full message envelope serialized to JSON (Ruling W6d Option 1) — lets
        /// the DLQ-drain reconstruct the verbatim `DeadLetter` from the DB so the
        /// drain hook keeps returning `Vec<DeadLetter>` with body/correlation_id.
        message_json: String,
    },
    /// Phase-16 W6d (A6): delete ALL rows from the `dead_letters` table — the
    /// DB-side of the DLQ drain/DELETE (`/colony/dead_letters` DELETE). The read
    /// side snapshots the rows first; this op clears them durably.
    DeleteAllDeadLetters {
        /// Fires after the DELETE is committed — the drain caller awaits it so the
        /// returned snapshot and the on-disk clear are consistent.
        ack: Option<tokio::sync::oneshot::Sender<()>>,
    },
    /// Phase-16 W6d (A6): a no-op write FENCE. Fires `ack` after the current batch
    /// commits — guaranteeing every prior fire-and-forget op (notably
    /// `InsertDeadLetter`) is durable before the caller reads via the read-only
    /// connection. The DLQ Read/Drain paths fence first for deterministic
    /// read-after-write (the writer thread is async; the channel is FIFO).
    Fence {
        /// Fires after the batch containing all prior-enqueued ops is committed.
        ack: tokio::sync::oneshot::Sender<()>,
    },
    /// Phase-13.5-A6 follow-up: deterministic shutdown signal.
    ///
    /// Writer drains the current batch (commits, fires all ops' acks),
    /// then sends `ack` and returns — avoiding the race-prone implicit
    /// close-detection of `blocking_recv()` under load. Producers must
    /// stop sending BEFORE enqueuing this op (Single-Owner-Drain pattern,
    /// see colony.rs `ColonyMsg::Shutdown` arm).
    Shutdown {
        /// Fires after the final batch is committed and the loop is about to exit.
        ack: tokio::sync::oneshot::Sender<()>,
    },
}

/// Message-Log-Row mit allen Phase-5-Feldern (FIX 1 — correlation_id, ttl, reply_to mit).
///
/// 12 Spalten entsprechen dem colony.db `message_log`-Schema (T6).
pub struct MessageLogRow {
    /// Message-ID (UUID v7).
    pub id: String,
    /// Trace-Root-ID (konstant über die Trace-Chain).
    pub trace_id: String,
    /// Parent-Message-ID; None bei Source-Messages.
    pub parent_message_id: Option<String>,
    /// Correlation-ID für Request/Response-Paarung (Phase 8/10 — FIX 1).
    pub correlation_id: Option<String>,
    /// Post-Dekrement-TTL am Hop (FIX 1).
    pub ttl: i64,
    /// Sender-Pfad; "@external"-Sentinel für Source-Messages.
    pub from_path: String,
    /// Resolved Empfänger-Pfad.
    pub to_path: String,
    /// Reply-Target (Cell-Adresse für Error-Replies) (FIX 1).
    pub reply_to: Option<String>,
    /// Headers als JSON.
    pub headers_json: String,
    /// Body-Variante: "inline" oder "blob".
    pub body_kind: String,
    /// Body-Payload: JSON wenn inline, UUID-String wenn blob.
    pub body_payload: Option<String>,
    /// Unix-Sekunden bei Anlage.
    pub created_at: i64,
}

/// Writer-Thread-Loop: blockierendes `recv()` auf das erste Item,
/// dann `try_recv()`-Drain bis `BATCH_MAX` oder Empty, eine Transaktion.
///
/// **FIX 3 — `InitialApply` ist immer atomar**: das gesamte Bundle (edges + hive_scopes)
/// wird in derselben Transaktion verarbeitet. Bei Crash mid-batch rollbackt SQLite.
///
/// **Phase-13.5-A6-followup — deterministic shutdown via `ColonyWriteOp::Shutdown`**:
/// statt sich auf `blocking_recv() == None` (race-prone unter Last) zu verlassen,
/// signalisiert `ColonyDb::shutdown` über eine explizite `Shutdown { ack }`-Op.
/// Der Writer drained den aktuellen Batch (incl. ggf. weitere Ops nach Shutdown),
/// committet, feuert ack + alle Op-Acks, returnt explizit. FIFO-Reihenfolge stellt
/// sicher, dass alle vor Shutdown enqueued'en Ops persistiert sind.
///
/// Write-Fehler: `tracing::error!` + `panic!`. JoinHandle propagiert, Tests fangen's.
pub(crate) fn run_writer(
    mut rx: tokio::sync::mpsc::Receiver<ColonyWriteOp>,
    mut conn: rusqlite::Connection,
    queue_depth: Arc<AtomicI64>,
) {
    while let Some(first) = rx.blocking_recv() {
        let tx = conn.transaction().expect("begin tx");
        let mut acks: Vec<tokio::sync::oneshot::Sender<()>> = Vec::new();
        let mut mpsc_acks: Vec<std::sync::mpsc::Sender<()>> = Vec::new();
        let mut shutdown_ack: Option<tokio::sync::oneshot::Sender<()>> = None;
        let mut count = 0usize;

        // First op: Shutdown-Signal abfangen, sonst apply.
        match first {
            ColonyWriteOp::Shutdown { ack } => {
                shutdown_ack = Some(ack);
            }
            op => {
                apply_op(&tx, op, &mut acks, &mut mpsc_acks);
                count += 1;
            }
        }

        // Batch-Drain: alle weiteren Ops bis BATCH_MAX oder Channel empty.
        // Shutdown im Batch wird gefangen, aber wir verarbeiten alle vorherigen
        // Ops in DIESEM Batch trotzdem (FIFO-Durability).
        while count < BATCH_MAX && shutdown_ack.is_none() {
            match rx.try_recv() {
                Ok(ColonyWriteOp::Shutdown { ack }) => {
                    shutdown_ack = Some(ack);
                }
                Ok(op) => {
                    apply_op(&tx, op, &mut acks, &mut mpsc_acks);
                    count += 1;
                }
                Err(_) => break,
            }
        }

        if let Err(e) = tx.commit() {
            tracing::error!(error = %e, "colony.db writer commit failed");
            panic!("colony.db writer commit failed: {e}");
        }
        // Fire acks AFTER tx.commit() returned — durable Ack-Garantie.
        for a in acks {
            let _ = a.send(());
        }
        for a in mpsc_acks {
            let _ = a.send(());
        }
        // Decrement queue_depth um den verarbeiteten Batch.
        queue_depth.fetch_sub(count as i64, Ordering::Relaxed);

        // Shutdown-Op gesehen: ack fire NACH allen Op-Acks (deterministische
        // Reihenfolge — durable acks der vorherigen Ops sind producer-sichtbar,
        // bevor Shutdown-Caller den join unblockt), Loop explicit verlassen.
        if let Some(ack) = shutdown_ack {
            let _ = ack.send(());
            return;
        }
    }
}

fn apply_op(
    tx: &rusqlite::Transaction<'_>,
    op: ColonyWriteOp,
    acks: &mut Vec<tokio::sync::oneshot::Sender<()>>,
    mpsc_acks: &mut Vec<std::sync::mpsc::Sender<()>>,
) {
    let now = now_unix_secs();
    match op {
        ColonyWriteOp::InitialApply { edges, hive_scopes } => {
            for s in hive_scopes {
                tx.execute(
                    "INSERT OR IGNORE INTO hive_scopes (path, created_at) VALUES (?, ?)",
                    rusqlite::params![s.as_str(), now],
                )
                .expect("insert hive_scope");
            }
            for e in edges {
                let condition = e.condition.as_ref().map(|c| c.source.clone());
                let modifier = e
                    .modifier
                    .as_ref()
                    .and_then(|m| meclaw_core::serde_json::to_string(&m.source).ok());
                tx.execute(
                    "INSERT OR IGNORE INTO edges (id, from_path, to_path, created_at, condition, modifier) \
                     VALUES (?, ?, ?, ?, ?, ?)",
                    rusqlite::params![e.id.to_string(), e.from.as_str(), e.to.as_str(), now, condition, modifier],
                )
                .expect("insert edge");
            }
            // Bootstrap-Recovery: clear the in-flight marker in the SAME
            // transaction as the bundle — the apply is complete exactly when
            // edges+hive_scopes are visible, never before, never after.
            tx.execute("DELETE FROM meta WHERE key='bootstrap_in_flight'", [])
                .expect("clear bootstrap_in_flight marker");
        }
        ColonyWriteOp::UpsertRegistry {
            path,
            cell_id,
            cell_type,
            created_at,
            updated_at,
        } => {
            tx.execute(
                "INSERT INTO registry (path, cell_id, cell_type, status, created_at, updated_at)
                 VALUES (?, ?, ?, 'active', ?, ?)
                 ON CONFLICT(path) DO UPDATE SET
                     updated_at = excluded.updated_at",
                rusqlite::params![path.as_str(), cell_id, cell_type, created_at, updated_at],
            )
            .expect("upsert registry");
        }
        ColonyWriteOp::SetRegistryStatus {
            path,
            status,
            updated_at,
        } => {
            tx.execute(
                "UPDATE registry SET status=?, updated_at=? WHERE path=?",
                rusqlite::params![status, updated_at, path.as_str()],
            )
            .expect("set registry status");
        }
        ColonyWriteOp::InsertMessageLog(row) => {
            tx.execute(
                "INSERT INTO message_log (
                    id, trace_id, parent_message_id, correlation_id, ttl,
                    from_path, to_path, reply_to, headers, body_kind, body_payload, created_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![
                    row.id,
                    row.trace_id,
                    row.parent_message_id,
                    row.correlation_id,
                    row.ttl,
                    row.from_path,
                    row.to_path,
                    row.reply_to,
                    row.headers_json,
                    row.body_kind,
                    row.body_payload,
                    row.created_at,
                ],
            )
            .expect("insert message_log");
        }
        ColonyWriteOp::MutationLogInsert {
            id,
            scope,
            payload_json,
            created_at,
            ack,
        } => {
            tx.execute(
                "INSERT INTO mutation_log (id, scope, payload_json, status, created_at)
                 VALUES (?, ?, ?, 'in_flight', ?)",
                rusqlite::params![id, scope, payload_json, created_at],
            )
            .expect("insert mutation_log");
            if let Some(a) = ack {
                acks.push(a);
            }
        }
        ColonyWriteOp::MutationLogRejectInsert {
            id,
            scope,
            payload_json,
            error_code,
            reason,
            trace_id,
            created_at,
            ack,
        } => {
            // `INSERT OR IGNORE` enforces the Validate-Reject vs. Apply-failed
            // Abgrenzung at the storage layer: a validate-stage reject has a
            // FRESH mutation id (no prior row), so this always inserts the
            // `rejected` row. An apply-stage failure that also funnels through
            // `send_eda_reject` (the staging/rename-failure path) ALREADY owns an
            // `in_flight`→`failed` row for that id — the more specific record —
            // so the reject insert no-ops instead of PK-conflicting or
            // conflating the two classes into a duplicate `rejected` row.
            tx.execute(
                "INSERT OR IGNORE INTO mutation_log
                   (id, scope, payload_json, status, failure_reason, error_code, trace_id, created_at)
                 VALUES (?, ?, ?, 'rejected', ?, ?, ?, ?)",
                rusqlite::params![
                    id,
                    scope,
                    payload_json,
                    reason,
                    error_code,
                    trace_id,
                    created_at
                ],
            )
            .expect("insert mutation_log reject row");
            if let Some(a) = ack {
                acks.push(a);
            }
        }
        ColonyWriteOp::InsertEdge {
            id,
            from,
            to,
            created_at,
            condition,
            modifier,
        } => {
            tx.execute(
                "INSERT OR IGNORE INTO edges (id, from_path, to_path, created_at, condition, modifier) \
                 VALUES (?, ?, ?, ?, ?, ?)",
                rusqlite::params![id, from, to, created_at, condition, modifier],
            )
            .expect("insert edge");
        }
        ColonyWriteOp::RemoveEdge { id } => {
            tx.execute("DELETE FROM edges WHERE id=?", rusqlite::params![id])
                .expect("delete edge");
        }
        ColonyWriteOp::InsertHiveScope { path, created_at } => {
            tx.execute(
                "INSERT OR IGNORE INTO hive_scopes (path, created_at) VALUES (?1, ?2)",
                rusqlite::params![path.as_str(), created_at],
            )
            .expect("insert hive_scope");
        }
        ColonyWriteOp::MutationLogUpdate {
            id,
            status,
            committed_at,
            failure_reason,
            ack,
        } => {
            tx.execute(
                "UPDATE mutation_log SET status=?, committed_at=?, failure_reason=? WHERE id=?",
                rusqlite::params![status, committed_at, failure_reason, id],
            )
            .expect("update mutation_log");
            if let Some(a) = ack {
                acks.push(a);
            }
        }
        ColonyWriteOp::UpsertTemplate {
            template_id,
            name,
            version,
            filesystem_path,
            description_json,
            tags_json,
            author,
            scanned_at,
            ack,
        } => {
            tx.execute(
                "INSERT INTO templates (template_id, name, version, filesystem_path,
                                        description_json, tags_json, author, scanned_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(name, COALESCE(version, '')) DO UPDATE SET
                     template_id      = excluded.template_id,
                     filesystem_path  = excluded.filesystem_path,
                     description_json = excluded.description_json,
                     tags_json        = excluded.tags_json,
                     author           = excluded.author,
                     scanned_at       = excluded.scanned_at",
                rusqlite::params![
                    template_id,
                    name,
                    version,
                    filesystem_path,
                    description_json,
                    tags_json,
                    author,
                    scanned_at
                ],
            )
            .expect("upsert template");
            if let Some(a) = ack {
                mpsc_acks.push(a);
            }
        }
        ColonyWriteOp::RemoveTemplate { template_id, ack } => {
            tx.execute(
                "DELETE FROM templates WHERE template_id = ?1",
                rusqlite::params![template_id],
            )
            .expect("delete template");
            if let Some(a) = ack {
                mpsc_acks.push(a);
            }
        }
        ColonyWriteOp::InsertDeadLetter {
            sender_path,
            original_target,
            resolved_target,
            error_code,
            trace_id,
            created_at,
            message_json,
        } => {
            tx.execute(
                "INSERT INTO dead_letters
                   (sender_path, original_target, resolved_target, error_code, trace_id, created_at, message_json)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![
                    sender_path,
                    original_target,
                    resolved_target,
                    error_code,
                    trace_id,
                    created_at,
                    message_json
                ],
            )
            .expect("insert dead_letter");
        }
        ColonyWriteOp::DeleteAllDeadLetters { ack } => {
            tx.execute("DELETE FROM dead_letters", [])
                .expect("delete all dead_letters");
            if let Some(a) = ack {
                acks.push(a);
            }
        }
        ColonyWriteOp::Fence { ack } => {
            // No SQL — the ack fires after this batch's commit (see run_writer),
            // which is exactly the fence guarantee: all prior-enqueued ops durable.
            acks.push(ack);
        }
        ColonyWriteOp::SetBootstrapInFlight { created_at, ack } => {
            tx.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('bootstrap_in_flight', ?)",
                rusqlite::params![created_at.to_string()],
            )
            .expect("set bootstrap_in_flight marker");
            if let Some(a) = ack {
                acks.push(a);
            }
        }
        ColonyWriteOp::Shutdown { .. } => {
            unreachable!(
                "Shutdown is handled at run_writer loop level (NOT via apply_op); \
                 see Phase-13.5-A6-followup deterministic-shutdown design"
            );
        }
    }
}

fn now_unix_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_apply_variant_bundles_edges_and_scopes() {
        let op = ColonyWriteOp::InitialApply {
            edges: vec![],
            hive_scopes: vec![Path::new("/")],
        };
        match op {
            ColonyWriteOp::InitialApply { hive_scopes, edges } => {
                assert_eq!(hive_scopes.len(), 1);
                assert_eq!(edges.len(), 0);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn upsert_registry_variant_holds_cell_id_and_type() {
        let op = ColonyWriteOp::UpsertRegistry {
            path: Path::new("/x"),
            cell_id: "abc".into(),
            cell_type: "echo".into(),
            created_at: 100,
            updated_at: 100,
        };
        match op {
            ColonyWriteOp::UpsertRegistry { cell_id, .. } => assert_eq!(cell_id, "abc"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn message_log_row_has_fix1_fields() {
        let row = MessageLogRow {
            id: "id1".into(),
            trace_id: "tr1".into(),
            parent_message_id: None,
            correlation_id: Some("corr1".into()),
            ttl: 63,
            from_path: "@external".into(),
            to_path: "/x".into(),
            reply_to: Some("/reply".into()),
            headers_json: "{}".into(),
            body_kind: "inline".into(),
            body_payload: Some("null".into()),
            created_at: 0,
        };
        assert_eq!(row.correlation_id.as_deref(), Some("corr1"));
        assert_eq!(row.ttl, 63);
        assert_eq!(row.reply_to.as_deref(), Some("/reply"));
    }

    #[test]
    fn mutation_log_insert_variant_holds_payload_and_ack() {
        let (ack_tx, _ack_rx) = tokio::sync::oneshot::channel();
        let op = ColonyWriteOp::MutationLogInsert {
            id: "mid-1".into(),
            scope: "/main".into(),
            payload_json: "{}".into(),
            created_at: 100,
            ack: Some(ack_tx),
        };
        match op {
            ColonyWriteOp::MutationLogInsert { id, ack, .. } => {
                assert_eq!(id, "mid-1");
                assert!(ack.is_some());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn mutation_log_update_variant_holds_status_and_reason() {
        let op = ColonyWriteOp::MutationLogUpdate {
            id: "mid-1".into(),
            status: "failed".into(),
            committed_at: 200,
            failure_reason: Some("crash_during_commit".into()),
            ack: None,
        };
        match op {
            ColonyWriteOp::MutationLogUpdate {
                status,
                failure_reason,
                ..
            } => {
                assert_eq!(status, "failed");
                assert_eq!(failure_reason.as_deref(), Some("crash_during_commit"));
            }
            _ => panic!("wrong variant"),
        }
    }

    /// Phase-16 W3 (A6): `MutationLogRejectInsert` writes a durable
    /// `status='rejected'` row carrying all five reject fields
    /// (status/error_code/failure_reason/trace_id/created_at). `committed_at`
    /// stays NULL — a validate-reject never reaches Apply/commit, which keeps it
    /// cleanly distinct from an Apply-`failed` row (in_flight→failed, committed_at set).
    #[test]
    fn mutation_log_reject_insert_writes_rejected_row_with_five_fields() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::persist::schema::setup_colony_db(&conn).unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        apply_op(
            &tx,
            ColonyWriteOp::MutationLogRejectInsert {
                id: "rej-1".into(),
                scope: "/main".into(),
                payload_json: r#"{"scope":"/main"}"#.into(),
                error_code: "scope_out_of_bounds".into(),
                reason: "ScopeOutOfBounds(\"/x\")".into(),
                trace_id: "trace-rej-1".into(),
                created_at: 4242,
                ack: None,
            },
            &mut Vec::new(),
            &mut Vec::new(),
        );
        tx.commit().unwrap();

        #[allow(clippy::type_complexity)]
        let (status, error_code, reason, trace_id, created_at, committed_at): (
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            i64,
            Option<i64>,
        ) = conn
            .query_row(
                "SELECT status, error_code, failure_reason, trace_id, created_at, committed_at
                 FROM mutation_log WHERE id = 'rej-1'",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(status, "rejected");
        assert_eq!(error_code.as_deref(), Some("scope_out_of_bounds"));
        assert_eq!(reason.as_deref(), Some("ScopeOutOfBounds(\"/x\")"));
        assert_eq!(trace_id.as_deref(), Some("trace-rej-1"));
        assert_eq!(created_at, 4242);
        assert!(committed_at.is_none(), "reject never commits");
    }

    #[tokio::test]
    async fn upsert_template_then_read_back() {
        let td = tempfile::TempDir::new().unwrap();
        let db = crate::ColonyDb::open(&td.path().join("c.db")).unwrap();
        let (ack_tx, ack_rx) = std::sync::mpsc::channel();
        db.send_op(ColonyWriteOp::UpsertTemplate {
            template_id: "t-1".into(),
            name: "echo".into(),
            version: Some("1.0".into()),
            filesystem_path: "/tmp/templates/echo@1.0".into(),
            description_json: "{}".into(),
            tags_json: "[]".into(),
            author: None,
            scanned_at: 42,
            ack: Some(ack_tx),
        })
        .await;
        ack_rx.recv().unwrap();
        let templates = db.read_templates().unwrap();
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].name, "echo");
        assert_eq!(templates[0].version.as_deref(), Some("1.0"));
    }

    #[tokio::test]
    async fn upsert_template_replaces_existing_by_name_version() {
        let td = tempfile::TempDir::new().unwrap();
        let db = crate::ColonyDb::open(&td.path().join("c.db")).unwrap();
        for path in ["/tmp/a", "/tmp/b"] {
            let (tx, rx) = std::sync::mpsc::channel();
            db.send_op(ColonyWriteOp::UpsertTemplate {
                template_id: "t-1".into(),
                name: "echo".into(),
                version: Some("1.0".into()),
                filesystem_path: path.into(),
                description_json: "{}".into(),
                tags_json: "[]".into(),
                author: None,
                scanned_at: 42,
                ack: Some(tx),
            })
            .await;
            rx.recv().unwrap();
        }
        let templates = db.read_templates().unwrap();
        assert_eq!(templates.len(), 1, "upsert must replace, not duplicate");
        assert_eq!(templates[0].filesystem_path, "/tmp/b");
    }

    #[tokio::test]
    async fn remove_template_deletes_row() {
        let td = tempfile::TempDir::new().unwrap();
        let db = crate::ColonyDb::open(&td.path().join("c.db")).unwrap();
        let (tx1, rx1) = std::sync::mpsc::channel();
        db.send_op(ColonyWriteOp::UpsertTemplate {
            template_id: "t-1".into(),
            name: "echo".into(),
            version: None,
            filesystem_path: "/tmp/a".into(),
            description_json: "{}".into(),
            tags_json: "[]".into(),
            author: None,
            scanned_at: 42,
            ack: Some(tx1),
        })
        .await;
        rx1.recv().unwrap();
        let (tx2, rx2) = std::sync::mpsc::channel();
        db.send_op(ColonyWriteOp::RemoveTemplate {
            template_id: "t-1".into(),
            ack: Some(tx2),
        })
        .await;
        rx2.recv().unwrap();
        assert!(db.read_templates().unwrap().is_empty());
    }

    #[tokio::test]
    async fn insert_message_log_writes_all_fields() {
        use crate::persist::ColonyDb;
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("c.db");
        let db = ColonyDb::open(&db_path).unwrap();

        db.send_op(ColonyWriteOp::InsertMessageLog(MessageLogRow {
            id: "msg-id-1".into(),
            trace_id: "trace-1".into(),
            parent_message_id: None,
            correlation_id: Some("corr-1".into()),
            ttl: 64,
            from_path: "@external".into(),
            to_path: "/dst".into(),
            reply_to: Some("/reply-target".into()),
            headers_json: r#"{"k":"v"}"#.into(),
            body_kind: "inline".into(),
            body_payload: Some("null".into()),
            created_at: 100,
        }))
        .await;

        // Test-Timeout-Guard via shutdown analog T13.
        use std::sync::mpsc::{RecvTimeoutError, channel};
        let (done_tx, done_rx) = channel();
        std::thread::spawn(move || {
            db.shutdown();
            done_tx.send(()).unwrap();
        });
        match done_rx.recv_timeout(std::time::Duration::from_secs(30)) {
            Ok(()) => {}
            Err(RecvTimeoutError::Timeout) => panic!("shutdown hung"),
            Err(e) => panic!("{e:?}"),
        }

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        #[allow(clippy::type_complexity)]
        let row: (
            String,
            String,
            Option<String>,
            Option<String>,
            i64,
            String,
            String,
            Option<String>,
            String,
            String,
            Option<String>,
            i64,
        ) = conn
            .query_row(
                "SELECT id, trace_id, parent_message_id, correlation_id, ttl,
                        from_path, to_path, reply_to, headers, body_kind, body_payload, created_at
                 FROM message_log WHERE id = ?",
                ["msg-id-1"],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                        r.get(8)?,
                        r.get(9)?,
                        r.get(10)?,
                        r.get(11)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(row.0, "msg-id-1");
        assert_eq!(row.3, Some("corr-1".to_string()));
        assert_eq!(row.4, 64);
        assert_eq!(row.7, Some("/reply-target".to_string()));
        assert_eq!(row.5, "@external");
    }

    /// Phase-13.5-Durable-Edges Task 3 — `InsertEdge` persists `condition` and `modifier`.
    ///
    /// Proves that the two new fields are written to the `edges` table and
    /// are readable by a fresh `rusqlite::Connection` on the same in-memory DB.
    #[test]
    fn insert_edge_persists_condition_and_modifier_strings() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::persist::schema::setup_colony_db(&conn).unwrap();
        let id = meclaw_core::Uuid::now_v7().to_string();
        let tx = conn.unchecked_transaction().unwrap();
        apply_op(
            &tx,
            ColonyWriteOp::InsertEdge {
                id: id.clone(),
                from: "/a".into(),
                to: "/b".into(),
                created_at: 0,
                condition: Some("hop.kind == 'text'".into()),
                modifier: Some(r#"{"set_hop":{"tier":"'gold'"}}"#.into()),
            },
            &mut Vec::new(),
            &mut Vec::new(),
        );
        tx.commit().unwrap();
        let (c, m): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT condition, modifier FROM edges WHERE id = ?",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(c.as_deref(), Some("hop.kind == 'text'"));
        assert_eq!(m.as_deref(), Some(r#"{"set_hop":{"tier":"'gold'"}}"#));
    }

    /// Phase-13.5 Lifecycle-3b Task 1.3 — `SetRegistryStatus` does an UPDATE-only
    /// (not UPSERT): it flips `status` of an existing row and bumps `updated_at`
    /// WITHOUT touching `cell_id`. Seed via `UpsertRegistry` (status='active'),
    /// then `SetRegistryStatus { status: "inactive" }`, then probe a fresh read:
    /// status must be 'inactive' and cell_id must be unchanged.
    #[test]
    fn set_registry_status_updates_status_without_resetting_cell_id() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::persist::schema::setup_colony_db(&conn).unwrap();

        // Seed an 'active' row (UpsertRegistry hardcodes status='active').
        let tx = conn.unchecked_transaction().unwrap();
        apply_op(
            &tx,
            ColonyWriteOp::UpsertRegistry {
                path: Path::new("/probe"),
                cell_id: "cell-id-original".into(),
                cell_type: "echo".into(),
                created_at: 100,
                updated_at: 100,
            },
            &mut Vec::new(),
            &mut Vec::new(),
        );
        tx.commit().unwrap();

        // Flip to 'inactive' via SetRegistryStatus (UPDATE-only).
        let tx = conn.unchecked_transaction().unwrap();
        apply_op(
            &tx,
            ColonyWriteOp::SetRegistryStatus {
                path: Path::new("/probe"),
                status: "inactive".into(),
                updated_at: 200,
            },
            &mut Vec::new(),
            &mut Vec::new(),
        );
        tx.commit().unwrap();

        // Fresh-read probe: status flipped, cell_id untouched, updated_at bumped.
        let (status, cell_id, updated_at): (String, String, i64) = conn
            .query_row(
                "SELECT status, cell_id, updated_at FROM registry WHERE path = ?",
                ["/probe"],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(status, "inactive", "status must be flipped to inactive");
        assert_eq!(
            cell_id, "cell-id-original",
            "cell_id must NOT be reset by SetRegistryStatus (no UPSERT)"
        );
        assert_eq!(updated_at, 200, "updated_at must be bumped");
    }

    /// Phase-13.5 Task 4 — `InitialApply` persists `condition` and `modifier` per edge.
    ///
    /// Proves that bootstrap-declared CEL edges survive reboot: the two new
    /// columns are written by the `InitialApply` arm and are readable from the
    /// same in-memory DB after commit.
    #[test]
    fn initial_apply_persists_edge_condition_source() {
        use crate::config::ModifierSpec;

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::persist::schema::setup_colony_db(&conn).unwrap();

        let spec: ModifierSpec =
            meclaw_core::serde_json::from_str(r#"{"set_hop":{"tier":"'gold'"}}"#).unwrap();
        let edge = crate::bootstrap::PlannedEdge {
            id: meclaw_core::Uuid::now_v7(),
            from: meclaw_core::Path::new("/a"),
            to: meclaw_core::Path::new("/b"),
            condition: Some(crate::cel_eval::parse_condition("hop.kind == 'text'").unwrap()),
            modifier: Some(crate::cel_eval::parse_modifier(&spec).unwrap()),
        };
        let id_str = edge.id.to_string();

        let tx = conn.unchecked_transaction().unwrap();
        apply_op(
            &tx,
            ColonyWriteOp::InitialApply {
                edges: vec![edge],
                hive_scopes: vec![],
            },
            &mut Vec::new(),
            &mut Vec::new(),
        );
        tx.commit().unwrap();

        let (c, m): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT condition, modifier FROM edges WHERE id = ?",
                [&id_str],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(c.as_deref(), Some("hop.kind == 'text'"));
        let m_val: meclaw_core::serde_json::Value =
            meclaw_core::serde_json::from_str(m.as_deref().unwrap()).unwrap();
        let expected: meclaw_core::serde_json::Value =
            meclaw_core::serde_json::from_str(r#"{"set_hop":{"tier":"'gold'"}}"#).unwrap();
        assert_eq!(m_val, expected);
    }

    /// Phase-13.5 T2 — `InsertHiveScope` persists a single hive-scope row.
    ///
    /// Proves: `apply_op` with `InsertHiveScope { path: "/foo", created_at: 100 }`
    /// inserts exactly one row into `hive_scopes` with the correct values.
    /// A fresh `rusqlite::Connection` (post-commit) reads back `("/foo", 100)`.
    #[test]
    fn insert_hive_scope_persists_path_and_created_at() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::persist::schema::setup_colony_db(&conn).unwrap();

        let tx = conn.unchecked_transaction().unwrap();
        apply_op(
            &tx,
            ColonyWriteOp::InsertHiveScope {
                path: meclaw_core::Path::new("/foo"),
                created_at: 100,
            },
            &mut Vec::new(),
            &mut Vec::new(),
        );
        tx.commit().unwrap();

        let (path, created_at): (String, i64) = conn
            .query_row(
                "SELECT path, created_at FROM hive_scopes WHERE path = '/foo'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(path, "/foo");
        assert_eq!(created_at, 100);
    }

    /// Bootstrap-Recovery — `SetBootstrapInFlight` persists the durable meta
    /// marker `bootstrap_in_flight`; a second apply replaces it idempotently.
    #[test]
    fn set_bootstrap_in_flight_persists_meta_marker() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::persist::schema::setup_colony_db(&conn).unwrap();

        let tx = conn.unchecked_transaction().unwrap();
        apply_op(
            &tx,
            ColonyWriteOp::SetBootstrapInFlight {
                created_at: 42,
                ack: None,
            },
            &mut Vec::new(),
            &mut Vec::new(),
        );
        tx.commit().unwrap();

        let value: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key='bootstrap_in_flight'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(value, "42");
    }

    /// Bootstrap-Recovery — the `InitialApply` bundle clears the marker in the
    /// SAME transaction as the edges/hive_scopes writes: after commit the
    /// bundle rows are visible AND the marker is gone (atomic completion).
    #[test]
    fn initial_apply_clears_bootstrap_marker_in_same_tx() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::persist::schema::setup_colony_db(&conn).unwrap();

        // Marker committed first (the apply-start write).
        let tx = conn.unchecked_transaction().unwrap();
        apply_op(
            &tx,
            ColonyWriteOp::SetBootstrapInFlight {
                created_at: 1,
                ack: None,
            },
            &mut Vec::new(),
            &mut Vec::new(),
        );
        tx.commit().unwrap();

        // InitialApply bundle: hive scope + marker clear, one transaction.
        let tx = conn.unchecked_transaction().unwrap();
        apply_op(
            &tx,
            ColonyWriteOp::InitialApply {
                edges: vec![],
                hive_scopes: vec![Path::new("/h")],
            },
            &mut Vec::new(),
            &mut Vec::new(),
        );
        tx.commit().unwrap();

        let marker_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM meta WHERE key='bootstrap_in_flight'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(marker_count, 0, "InitialApply must clear the marker");
        let scopes: i64 = conn
            .query_row("SELECT COUNT(*) FROM hive_scopes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(scopes, 1, "the bundle rows commit together with the clear");
    }

    /// Phase-13.5-A6 follow-up — Durability-Regression-Gate.
    ///
    /// Beweis: 100 fire-and-forget Writes vor `shutdown()` landen ALLE in
    /// der DB, von einer frischen Connection lesbar nach `shutdown()`.
    /// Schließt den potentiellen Durability-Bug einer naiven Shutdown-Op-
    /// Implementation, die den letzten Batch vorzeitig schneiden würde.
    ///
    /// Plus: Liveness-Gate — `shutdown()` returnt innerhalb 2s ohne Hang,
    /// auch unter Workspace-Last (Race-Schließung für insert_message_log_writes_all_fields).
    #[tokio::test]
    async fn shutdown_persists_all_prior_writes_and_returns_within_timeout() {
        use crate::persist::ColonyDb;
        use std::sync::mpsc::{RecvTimeoutError, channel};

        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("c.db");
        let db = ColonyDb::open(&db_path).unwrap();

        // 100 Writes fire-and-forget, fluten den bounded(1000)-Channel + zwingen
        // den Writer zu mehreren Batch-Iterationen (BATCH_MAX=64). Maximaler
        // Race-Druck zwischen "letzter Send" und "drop(writer_tx)".
        for i in 0..100 {
            db.send_op(ColonyWriteOp::InsertMessageLog(MessageLogRow {
                id: format!("msg-{i}"),
                trace_id: format!("trace-{i}"),
                parent_message_id: None,
                correlation_id: None,
                ttl: 64,
                from_path: "@external".into(),
                to_path: "/dst".into(),
                reply_to: None,
                headers_json: "{}".into(),
                body_kind: "inline".into(),
                body_payload: Some("null".into()),
                created_at: i as i64,
            }))
            .await;
        }

        // Liveness-Gate: shutdown in separater std::thread; recv_timeout 30s.
        let (done_tx, done_rx) = channel();
        std::thread::spawn(move || {
            db.shutdown();
            done_tx.send(()).unwrap();
        });
        match done_rx.recv_timeout(std::time::Duration::from_secs(30)) {
            Ok(()) => {}
            Err(RecvTimeoutError::Timeout) => panic!("shutdown hung"),
            Err(e) => panic!("{e:?}"),
        }

        // Durability-Gate: ALLE 100 Rows lesbar von fresh Connection.
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM message_log", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            count, 100,
            "all 100 writes must persist; shutdown-op must not truncate the final batch"
        );
        // Spot-check first + last für Reihenfolge-Integrität (id-pattern).
        for i in [0, 50, 99] {
            let id: String = conn
                .query_row(
                    "SELECT id FROM message_log WHERE id=?",
                    [format!("msg-{i}")],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(id, format!("msg-{i}"));
        }
    }
}
