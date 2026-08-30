//! Write ops for the `colony.db` writer thread.
//!
//! Three operation variants:
//! - `InitialApply` — atomic bundle for the first boot (FIX 3, review 2026-05-20):
//!   edges + hive scopes in ONE transaction. Guards against a mixed state on a
//!   crash mid-first-boot.
//! - `UpsertRegistry` — per `ColonyMsg::Register`, op-before-ack invariant (T22).
//! - `InsertMessageLog` — per successful routing hop (T32 fills the schema with FIX 1).
//!
//! **Phase-6 extension**: `apply_op` collects optional `oneshot::Sender<()>` acks
//! per op; `run_writer` fires them AFTER `tx.commit()` — see phase-6 plan T2.
//! `send_op` (fire-and-forget) stays the default; durable writes go through
//! `ColonyDb::insert_mutation_log_durable`/`update_mutation_log_durable`, which
//! enqueue an op with `ack: Some(tx)` and then `rx.await`.

use crate::bootstrap::PlannedEdge;
use meclaw_core::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

// Phase 12-Pre: the main channel is a tokio::sync::mpsc::Receiver; the writer
// thread drains it via blocking_recv() (the canonical bridge for an async sender
// + a sync receiver in a std::thread). std::sync::mpsc remains only for the
// phase-11 template ack channels (mpsc_acks: Vec<std::sync::mpsc::Sender<()>>).

/// Maximum batch size per transaction.
const BATCH_MAX: usize = 64;

/// Operations for the `colony.db` writer thread.
pub enum ColonyWriteOp {
    /// Atomic first-boot bundle (FIX 3): edges + hive_scopes in one transaction.
    InitialApply {
        /// Edges from the bootstrap plan.
        edges: Vec<PlannedEdge>,
        /// Hive-scope paths from the bootstrap plan.
        hive_scopes: Vec<Path>,
    },
    /// Registry upsert with cell_id-stable semantics.
    ///
    /// **Phase-13.5 Lifecycle-3b: does NOT manage the `status` column on conflict.**
    /// The first INSERT seeds `status = 'active'` (a fresh node is active), but the
    /// `ON CONFLICT(path)` path only bumps `updated_at` and leaves `status` untouched.
    /// `SetRegistryStatus` is the sole write-authority for `status` — re-registration
    /// or reboot must NOT clobber an `'inactive'` previously written by it.
    UpsertRegistry {
        /// Cell path (primary key).
        path: Path,
        /// UUID v7, assigned once, never overwritten.
        cell_id: String,
        /// Cell-type string.
        cell_type: String,
        /// Unix seconds, set once on the first insert.
        created_at: i64,
        /// Unix seconds, bumped per reboot/status touch.
        updated_at: i64,
    },
    /// Phase-13.5 Lifecycle-3b: UPDATE-only of the `registry.status` column for an
    /// existing row (`UPDATE registry SET status=?, updated_at=? WHERE path=?`).
    /// Unlike `UpsertRegistry` this is NOT an UPSERT — the row always exists per
    /// No-Delete, and `cell_id`/`cell_type`/`created_at` stay untouched. Carries
    /// the edge-derived activity (`"active"`/`"inactive"`) into persistence.
    SetRegistryStatus {
        /// Cell path (primary key of the row to update).
        path: Path,
        /// New status string, e.g. `"active"` or `"inactive"`.
        status: String,
        /// Unix seconds of the status change.
        updated_at: i64,
    },
    /// GH #491: UPDATE-only of the `registry.dormant` column for an existing
    /// row (`UPDATE registry SET dormant=?, updated_at=? WHERE path=?`).
    ///
    /// The durable record of an EXPLICIT decision that a node is asleep —
    /// `add_nodes[].birth: "inactive"` and the `ref` marker's equivalent. It is
    /// not a second activity state: activity stays derived from the edge table,
    /// and the marker only tells the connectivity recompute that this node's
    /// inactivity was DECLARED, so a mutation that merely REACHES it (a
    /// recompute scope that expands over its subtree) must not derive it active
    /// again. A mutation that ADDRESSES the node clears it, and then the
    /// ordinary reconnect applies as it always did.
    ///
    /// Same shape and the same reason as [`Self::SetRegistryStatus`]: the row is
    /// created by `UpsertRegistry`, this op is the SOLE write-authority for the
    /// column, and it is always enqueued AFTER the `UpsertRegistry` of the same
    /// path (the writer channel is FIFO, so the row exists by then).
    SetRegistryDormant {
        /// Cell path (primary key of the row to update).
        path: Path,
        /// `true` when the node's sleep was declared, `false` once a mutation
        /// that names it has woken it.
        dormant: bool,
        /// Unix seconds of the change.
        updated_at: i64,
    },
    /// GH #169: UPDATE-only re-address of an existing `registry` row
    /// (`UPDATE registry SET path=?, updated_at=? WHERE path=?`) — the durable
    /// half of a `move_nodes` relocation.
    ///
    /// An UPDATE and not a delete-plus-insert, because the two are different
    /// claims about the world. A move says: this is the same cell, it lives
    /// somewhere else now. Deleting the old row and inserting a new one at the
    /// target would re-stamp `created_at`, drop the provenance columns and mint
    /// a second identity for a cell that only ever had one — which is precisely
    /// the cost the issue was written about. Moving the row keeps `cell_id`,
    /// `cell_type`, `status`, `created_at` and the three provenance columns
    /// untouched; only the address and `updated_at` change.
    ///
    /// Always enqueued BEFORE the relocated node's `UpsertRegistry` (the writer
    /// channel is FIFO), so that upsert lands on the moved row's
    /// `ON CONFLICT(path)` branch and bumps nothing but `updated_at`.
    MoveRegistryPath {
        /// The address the cell is leaving (primary key of the row to move).
        from: Path,
        /// The address it is taking. Validated free before the mutation touched
        /// anything, so the UPDATE cannot collide with a live row.
        to: Path,
        /// Unix seconds of the relocation.
        updated_at: i64,
    },
    /// GH #62: UPDATE-only fill of the four `registry` provenance columns
    /// (`template`, `template_version`, `instantiated_at` and, since GH #277,
    /// the JSON-serialized `template_chain`) for an existing row.
    ///
    /// Same shape and the same reason as [`Self::SetRegistryStatus`]: the row is
    /// created by `UpsertRegistry`, and this op is the SOLE write-authority for
    /// its provenance columns — it never inserts, never touches `cell_id`,
    /// `cell_type` or `status`, and is always enqueued AFTER the `UpsertRegistry`
    /// of the same path (the writer channel is FIFO, so the row exists by then).
    ///
    /// The columns are a query INDEX (`SELECT path FROM registry WHERE
    /// template = ?`); the authoritative record is `cell.provenance` in the
    /// node's own `config.json`, which travels with an exported tree.
    SetRegistryProvenance {
        /// Cell path (primary key of the row to update).
        path: Path,
        /// Template identity stamped into this node at instantiation.
        provenance: crate::config::NodeProvenance,
    },
    /// Message-log insert (FIX-1 fields anchored in T32).
    InsertMessageLog(MessageLogRow),
    /// Phase 6: insert in_flight row into mutation_log; ack fires after tx.commit().
    MutationLogInsert {
        /// Mutation ID (UUID v7).
        id: String,
        /// Mutation scope (path prefix).
        scope: String,
        /// Mutation payload as a JSON blob.
        payload_json: String,
        /// Unix seconds at creation time.
        created_at: i64,
        /// Optional ack sender; fires after `tx.commit()`.
        ack: Option<tokio::sync::oneshot::Sender<()>>,
    },
    /// Phase-16 W3 (A6): insert a `status='rejected'` row for a validate-stage
    /// reject (fire-and-forget). Unlike `MutationLogInsert` (`in_flight`, later
    /// updated to `committed`/`failed`), a reject is a single terminal INSERT:
    /// the mutation never reached apply, so `committed_at` stays NULL. Carries
    /// the two v3 columns `error_code` + `trace_id` plus the human `reason` in
    /// `failure_reason`. Makes schema/scope/naming rejects visible in the
    /// `/colony/mutations` audit (K-H2: previously invisible).
    MutationLogRejectInsert {
        /// Mutation ID (UUID v7).
        id: String,
        /// Mutation scope (path prefix).
        scope: String,
        /// Mutation payload as a JSON blob (diagnostic preservation of the rejected request).
        payload_json: String,
        /// `error_code` of the reject reply (e.g. `scope_out_of_bounds`).
        error_code: String,
        /// Human-readable reason (`format!("{err:?}")`), stored in `failure_reason`.
        reason: String,
        /// Trace ID of the mutation request.
        trace_id: String,
        /// Unix seconds at rejection time.
        created_at: i64,
        /// Optional ack sender; fires after `tx.commit()` — the requester's reject
        /// path waits on it so the audit row is durable before the return.
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
        /// Unix seconds at creation time.
        created_at: i64,
        /// Phase-13.5 durable edges: CEL condition as a source string.
        /// `None` = the edge has no condition (unconditional routing).
        condition: Option<String>,
        /// Phase-13.5 durable edges: ModifierSpec as a JSON string (set+delete).
        /// `None` = the edge has no modifier (identity headers).
        modifier: Option<String>,
        /// GH #283: the edge's routing phase. `true` = a default, consulted only
        /// after every regular out-edge of the same sender declined. Persisted as
        /// `edges.is_default` (schema v7) so a reboot rehydrates the phase.
        is_default: bool,
    },
    /// Phase 6 T21: delete an edge row by id (fire-and-forget; durable via FIFO).
    RemoveEdge {
        /// Edge-UUID v7 as string.
        id: String,
    },
    /// Phase 6: update mutation_log status (committed | failed); ack fires after tx.commit().
    MutationLogUpdate {
        /// Mutation ID (primary key).
        id: String,
        /// New status: "committed" or "failed".
        status: String,
        /// Unix seconds at commit/failure time.
        committed_at: i64,
        /// Optional: failure reason when status="failed".
        failure_reason: Option<String>,
        /// Optional ack sender; fires after `tx.commit()`.
        ack: Option<tokio::sync::oneshot::Sender<()>>,
    },
    /// Phase 11 11-A: insert or update a template row; ack fires synchronously (mpsc).
    UpsertTemplate {
        /// Template ID (UUID v7).
        template_id: String,
        /// Template name (from `template.json`).
        name: String,
        /// Optional: semantic-version string.
        version: Option<String>,
        /// Absolute path to the template directory.
        filesystem_path: String,
        /// `description` field as a JSON blob.
        description_json: String,
        /// `tags` field as a JSON array string.
        tags_json: String,
        /// Optional: author string.
        author: Option<String>,
        /// Unix seconds of the last scan.
        scanned_at: i64,
        /// Optional ack sender; fires synchronously after the SQL execute.
        ack: Option<std::sync::mpsc::Sender<()>>,
    },
    /// Phase 11 11-A: delete a template row by template_id; ack fires synchronously.
    RemoveTemplate {
        /// Template ID (UUID v7).
        template_id: String,
        /// Optional ack sender; fires synchronously after the SQL execute.
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
    /// Bootstrap recovery (run-5/5b finding): durable `bootstrap_in_flight`
    /// marker into the `meta` table, written BEFORE the first-apply cell loop
    /// starts. The matching clear runs inside the `InitialApply` arm — same
    /// transaction as the edges/hive_scopes bundle, so a crash anywhere
    /// mid-apply leaves the marker behind and `probe_boot_state` classifies the
    /// next boot as a resumable `FirstBoot`. Since GH #89 the classification
    /// cut is "InitialApply traces (edges/hive_scopes) = Reboot"; the marker
    /// stays the explicit resume signal and defensively wins even over states
    /// WITH bundle traces (constructionally unreachable, but resume-safe).
    SetBootstrapInFlight {
        /// Unix seconds at apply start (forensic value of the marker).
        created_at: i64,
        /// Optional ack sender; fires after `tx.commit()` (durable —
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

/// Message-log row with all phase-5 fields (FIX 1 — correlation_id, ttl, reply_to included).
///
/// The 12 columns correspond to the colony.db `message_log` schema (T6).
pub struct MessageLogRow {
    /// Message ID (UUID v7).
    pub id: String,
    /// Trace root ID (constant across the trace chain).
    pub trace_id: String,
    /// Parent message ID; None for source messages.
    pub parent_message_id: Option<String>,
    /// Correlation ID for request/response pairing (phase 8/10 — FIX 1).
    pub correlation_id: Option<String>,
    /// Post-decrement TTL at the hop (FIX 1).
    pub ttl: i64,
    /// Sender path; "@external" sentinel for source messages.
    pub from_path: String,
    /// Resolved recipient path.
    pub to_path: String,
    /// Reply target (cell address for error replies) (FIX 1).
    pub reply_to: Option<String>,
    /// Headers as JSON.
    pub headers_json: String,
    /// Body variant: "inline" or "blob".
    pub body_kind: String,
    /// Body payload: JSON when inline, UUID string when blob.
    pub body_payload: Option<String>,
    /// Unix seconds at creation time.
    pub created_at: i64,
}

/// Writer thread loop: blocking `recv()` for the first item,
/// then a `try_recv()` drain up to `BATCH_MAX` or empty, one transaction.
///
/// **FIX 3 — `InitialApply` is always atomic**: the entire bundle (edges + hive_scopes)
/// is processed in the same transaction. On a crash mid-batch SQLite rolls back.
///
/// **Phase-13.5-A6 follow-up — deterministic shutdown via `ColonyWriteOp::Shutdown`**:
/// instead of relying on `blocking_recv() == None` (race-prone under load),
/// `ColonyDb::shutdown` signals through an explicit `Shutdown { ack }` op.
/// The writer drains the current batch (including any further ops after shutdown),
/// commits, fires the ack + all op acks, and returns explicitly. FIFO ordering
/// guarantees that every op enqueued before shutdown is persisted.
///
/// Write errors: `tracing::error!` + `panic!`. The JoinHandle propagates, tests catch it.
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

        // First op: catch the shutdown signal, otherwise apply.
        match first {
            ColonyWriteOp::Shutdown { ack } => {
                shutdown_ack = Some(ack);
            }
            op => {
                apply_or_die(&tx, op, &mut acks, &mut mpsc_acks);
                count += 1;
            }
        }

        // Batch drain: every further op up to BATCH_MAX or an empty channel.
        // A shutdown inside the batch is caught, but we still process all
        // preceding ops in THIS batch (FIFO durability).
        while count < BATCH_MAX && shutdown_ack.is_none() {
            match rx.try_recv() {
                Ok(ColonyWriteOp::Shutdown { ack }) => {
                    shutdown_ack = Some(ack);
                }
                Ok(op) => {
                    apply_or_die(&tx, op, &mut acks, &mut mpsc_acks);
                    count += 1;
                }
                Err(_) => break,
            }
        }

        if let Err(e) = tx.commit() {
            tracing::error!(error = %e, "colony.db writer commit failed");
            panic!("colony.db writer commit failed: {e}");
        }
        // Fire acks AFTER tx.commit() returned — durable ack guarantee.
        for a in acks {
            let _ = a.send(());
        }
        for a in mpsc_acks {
            let _ = a.send(());
        }
        // Decrement queue_depth by the processed batch.
        queue_depth.fetch_sub(count as i64, Ordering::Relaxed);

        // Shutdown op seen: fire its ack AFTER all op acks (deterministic
        // ordering — the durable acks of the preceding ops are producer-visible
        // before the shutdown caller unblocks the join), then leave the loop.
        if let Some(ack) = shutdown_ack {
            let _ = ack.send(());
            return;
        }
    }
}

/// One policy point for "a write to `colony.db` failed".
///
/// The decision itself is not new -- a failed write to the colony's own database
/// leaves the persisted topology disagreeing with the running one, and there is
/// nothing sensible to continue with, which is why `tx.commit()` below aborts the
/// process too. What is new is that the failure is LOGGED before the abort. Until
/// v0.14.0 every arm of `apply_op` carried its own `.expect("insert edge")`, and
/// those panic without a tracing event: the operator saw the process die and had
/// no record of which op killed it. Routing every op through here also means a new
/// write op no longer costs an entry in the unwrap/expect budget (GH #233).
fn apply_or_die(
    tx: &rusqlite::Transaction<'_>,
    op: ColonyWriteOp,
    acks: &mut Vec<tokio::sync::oneshot::Sender<()>>,
    mpsc_acks: &mut Vec<std::sync::mpsc::Sender<()>>,
) {
    if let Err(e) = apply_op(tx, op, acks, mpsc_acks) {
        tracing::error!(error = %e, "colony.db writer op failed");
        panic!("colony.db writer op failed: {e}");
    }
}

fn apply_op(
    tx: &rusqlite::Transaction<'_>,
    op: ColonyWriteOp,
    acks: &mut Vec<tokio::sync::oneshot::Sender<()>>,
    mpsc_acks: &mut Vec<std::sync::mpsc::Sender<()>>,
) -> rusqlite::Result<()> {
    let now = now_unix_secs();
    match op {
        ColonyWriteOp::InitialApply { edges, hive_scopes } => {
            for s in hive_scopes {
                tx.execute(
                    "INSERT OR IGNORE INTO hive_scopes (path, created_at) VALUES (?, ?)",
                    rusqlite::params![s.as_str(), now],
                )?;
            }
            for e in edges {
                let condition = e.condition.as_ref().map(|c| c.source.clone());
                let modifier = e
                    .modifier
                    .as_ref()
                    .and_then(|m| meclaw_core::serde_json::to_string(&m.source).ok());
                tx.execute(
                    "INSERT OR IGNORE INTO edges (id, from_path, to_path, created_at, condition, modifier, is_default) \
                     VALUES (?, ?, ?, ?, ?, ?, ?)",
                    rusqlite::params![e.id.to_string(), e.from.as_str(), e.to.as_str(), now, condition, modifier, e.is_default],
                )?;
            }
            // Bootstrap-Recovery: clear the in-flight marker in the SAME
            // transaction as the bundle — the apply is complete exactly when
            // edges+hive_scopes are visible, never before, never after.
            tx.execute("DELETE FROM meta WHERE key='bootstrap_in_flight'", [])?;
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
            )?;
        }
        ColonyWriteOp::SetRegistryStatus {
            path,
            status,
            updated_at,
        } => {
            tx.execute(
                "UPDATE registry SET status=?, updated_at=? WHERE path=?",
                rusqlite::params![status, updated_at, path.as_str()],
            )?;
        }
        ColonyWriteOp::SetRegistryDormant {
            path,
            dormant,
            updated_at,
        } => {
            tx.execute(
                "UPDATE registry SET dormant=?, updated_at=? WHERE path=?",
                rusqlite::params![dormant, updated_at, path.as_str()],
            )?;
        }
        ColonyWriteOp::MoveRegistryPath {
            from,
            to,
            updated_at,
        } => {
            tx.execute(
                "UPDATE registry SET path=?, updated_at=? WHERE path=?",
                rusqlite::params![to.as_str(), updated_at, from.as_str()],
            )?;
        }
        ColonyWriteOp::SetRegistryProvenance { path, provenance } => {
            // GH #277: an absent chain stays SQL NULL — serializing `None` into
            // the string "null" would put a chain-shaped value in a column that
            // means "no chain was recorded".
            let template_chain = provenance
                .template_chain
                .as_ref()
                .and_then(|chain| meclaw_core::serde_json::to_string(chain).ok());
            tx.execute(
                "UPDATE registry SET template=?, template_version=?, template_chain=?, \
                 instantiated_at=? WHERE path=?",
                rusqlite::params![
                    provenance.template,
                    provenance.template_version,
                    template_chain,
                    provenance.instantiated_at,
                    path.as_str()
                ],
            )?;
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
            )?;
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
            )?;
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
            // Demarcation at the storage layer: a validate-stage reject has a
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
            )?;
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
            is_default,
        } => {
            tx.execute(
                "INSERT OR IGNORE INTO edges (id, from_path, to_path, created_at, condition, modifier, is_default) \
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![id, from, to, created_at, condition, modifier, is_default],
            )?;
        }
        ColonyWriteOp::RemoveEdge { id } => {
            tx.execute("DELETE FROM edges WHERE id=?", rusqlite::params![id])?;
        }
        ColonyWriteOp::InsertHiveScope { path, created_at } => {
            tx.execute(
                "INSERT OR IGNORE INTO hive_scopes (path, created_at) VALUES (?1, ?2)",
                rusqlite::params![path.as_str(), created_at],
            )?;
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
            )?;
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
            )?;
            if let Some(a) = ack {
                mpsc_acks.push(a);
            }
        }
        ColonyWriteOp::RemoveTemplate { template_id, ack } => {
            tx.execute(
                "DELETE FROM templates WHERE template_id = ?1",
                rusqlite::params![template_id],
            )?;
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
            )?;
        }
        ColonyWriteOp::DeleteAllDeadLetters { ack } => {
            tx.execute("DELETE FROM dead_letters", [])?;
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
            )?;
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

    Ok(())
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
        )
        .expect("apply_op in test");
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
                is_default: false,
            },
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .expect("apply_op in test");
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

    /// GH #283 — `InsertEdge` persists the routing phase, both ways round.
    ///
    /// The write path is what a mutation-added default depends on: `true` has
    /// to reach the `edges.is_default` column as `1`, and `false` as `0` —
    /// an edge silently written as the wrong phase would route differently
    /// after the next reboot than it did before it.
    #[test]
    fn insert_edge_persists_the_default_phase() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::persist::schema::setup_colony_db(&conn).unwrap();
        for (path, is_default, expected) in [("/plain", false, 0i64), ("/fallback", true, 1i64)] {
            let id = meclaw_core::Uuid::now_v7().to_string();
            let tx = conn.unchecked_transaction().unwrap();
            apply_op(
                &tx,
                ColonyWriteOp::InsertEdge {
                    id: id.clone(),
                    from: "/a".into(),
                    to: path.into(),
                    created_at: 0,
                    condition: None,
                    modifier: None,
                    is_default,
                },
                &mut Vec::new(),
                &mut Vec::new(),
            )
            .expect("apply_op in test");
            tx.commit().unwrap();
            let got: i64 = conn
                .query_row("SELECT is_default FROM edges WHERE id = ?", [&id], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(got, expected, "edge to {path} persisted the wrong phase");
        }
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
        )
        .expect("apply_op in test");
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
        )
        .expect("apply_op in test");
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

    /// GH #62 — `SetRegistryProvenance` is UPDATE-only, like `SetRegistryStatus`:
    /// it fills the three provenance columns of an EXISTING row and touches
    /// nothing else. `cell_id`, `cell_type` and `status` must survive, because
    /// the op is sent right after the registration that created the row.
    #[test]
    fn set_registry_provenance_fills_the_columns_without_touching_identity() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::persist::schema::setup_colony_db(&conn).unwrap();

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
        )
        .expect("apply_op in test");
        tx.commit().unwrap();

        let tx = conn.unchecked_transaction().unwrap();
        apply_op(
            &tx,
            ColonyWriteOp::SetRegistryProvenance {
                path: Path::new("/probe"),
                provenance: crate::config::NodeProvenance {
                    template: "sink-tpl".into(),
                    template_version: Some("1.0.0".into()),
                    template_chain: None,
                    instantiated_at: 1_700_000_000,
                },
            },
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .expect("apply_op in test");
        tx.commit().unwrap();

        let (tpl, ver, at, cell_id, status): (String, String, i64, String, String) = conn
            .query_row(
                "SELECT template, template_version, instantiated_at, cell_id, status \
                 FROM registry WHERE path = ?",
                ["/probe"],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(tpl, "sink-tpl");
        assert_eq!(ver, "1.0.0");
        assert_eq!(at, 1_700_000_000);
        assert_eq!(cell_id, "cell-id-original", "identity must be untouched");
        assert_eq!(status, "active", "status must be untouched");
    }

    /// An unversioned template writes SQL NULL, not the empty string — "this
    /// template declares no version" has to stay distinguishable from "".
    #[test]
    fn set_registry_provenance_writes_null_for_an_unversioned_template() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::persist::schema::setup_colony_db(&conn).unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        apply_op(
            &tx,
            ColonyWriteOp::UpsertRegistry {
                path: Path::new("/probe"),
                cell_id: "c".into(),
                cell_type: "echo".into(),
                created_at: 1,
                updated_at: 1,
            },
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .expect("apply_op in test");
        apply_op(
            &tx,
            ColonyWriteOp::SetRegistryProvenance {
                path: Path::new("/probe"),
                provenance: crate::config::NodeProvenance {
                    template: "bare".into(),
                    template_version: None,
                    template_chain: None,
                    instantiated_at: 42,
                },
            },
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .expect("apply_op in test");
        tx.commit().unwrap();
        let ver: Option<String> = conn
            .query_row(
                "SELECT template_version FROM registry WHERE path = ?",
                ["/probe"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ver, None);
    }

    /// GH #277: the fourth provenance column carries the whole chain as JSON —
    /// outermost first, the node's own template last. The round-trip has to
    /// return exactly the pairs that went in, and a node without a chain has to
    /// read SQL NULL: `"null"` in the column would be a chain-shaped lie.
    #[test]
    fn set_registry_provenance_writes_the_template_chain() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::persist::schema::setup_colony_db(&conn).unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        for path in ["/composite", "/bare"] {
            apply_op(
                &tx,
                ColonyWriteOp::UpsertRegistry {
                    path: Path::new(path),
                    cell_id: "c".into(),
                    cell_type: "echo".into(),
                    created_at: 1,
                    updated_at: 1,
                },
                &mut Vec::new(),
                &mut Vec::new(),
            )
            .expect("apply_op in test");
        }
        let chain = vec![
            ("outer".to_string(), Some("1.0.0".to_string())),
            ("inner".to_string(), None),
        ];
        apply_op(
            &tx,
            ColonyWriteOp::SetRegistryProvenance {
                path: Path::new("/composite"),
                provenance: crate::config::NodeProvenance {
                    template: "inner".into(),
                    template_version: None,
                    template_chain: Some(chain.clone()),
                    instantiated_at: 7,
                },
            },
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .expect("apply_op in test");
        apply_op(
            &tx,
            ColonyWriteOp::SetRegistryProvenance {
                path: Path::new("/bare"),
                provenance: crate::config::NodeProvenance {
                    template: "solo".into(),
                    template_version: Some("2.0.0".into()),
                    template_chain: None,
                    instantiated_at: 7,
                },
            },
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .expect("apply_op in test");
        tx.commit().unwrap();

        let (tpl, ver, chain_json): (String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT template, template_version, template_chain \
                 FROM registry WHERE path = ?",
                ["/composite"],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(tpl, "inner", "the leaf stamp stays the last chain element");
        assert_eq!(ver, None);
        let round_tripped: Vec<(String, Option<String>)> =
            meclaw_core::serde_json::from_str(&chain_json.expect("template_chain written"))
                .expect("the column holds a chain");
        assert_eq!(round_tripped, chain);

        let bare: Option<String> = conn
            .query_row(
                "SELECT template_chain FROM registry WHERE path = ?",
                ["/bare"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bare, None, "a None chain writes SQL NULL, never \"null\"");
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
            is_default: false,
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
        )
        .expect("apply_op in test");
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
        )
        .expect("apply_op in test");
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
        )
        .expect("apply_op in test");
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
        )
        .expect("apply_op in test");
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
        )
        .expect("apply_op in test");
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

    /// Phase-13.5-A6 follow-up — durability regression gate.
    ///
    /// Proof: 100 fire-and-forget writes before `shutdown()` ALL land in
    /// the DB, readable from a fresh connection after `shutdown()`.
    /// Closes the potential durability bug of a naive shutdown-op
    /// implementation that would cut the final batch short.
    ///
    /// Plus: liveness gate — `shutdown()` returns within 2s without hanging,
    /// even under workspace load (race closure for insert_message_log_writes_all_fields).
    #[tokio::test]
    async fn shutdown_persists_all_prior_writes_and_returns_within_timeout() {
        use crate::persist::ColonyDb;
        use std::sync::mpsc::{RecvTimeoutError, channel};

        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("c.db");
        let db = ColonyDb::open(&db_path).unwrap();

        // 100 fire-and-forget writes, flooding the bounded(1000) channel + forcing
        // the writer through several batch iterations (BATCH_MAX=64). Maximum race
        // pressure between "last send" and "drop(writer_tx)".
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

        // Liveness gate: shutdown in a separate std::thread; recv_timeout 30s.
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

        // Durability gate: ALL 100 rows readable from a fresh connection.
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM message_log", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            count, 100,
            "all 100 writes must persist; shutdown-op must not truncate the final batch"
        );
        // Spot-check first + last for ordering integrity (id pattern).
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
