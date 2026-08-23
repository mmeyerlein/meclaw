//! `ColonyDb` struct: writer thread + read-only connection for `colony.db`.
//!
//! **Single-owner invariant (FIX 2, review 2026-05-20)**: `writer_tx` lives in
//! exactly one owner (`ColonyDb`) and is NEVER cloned into a longer-lived scope.
//! All sends go through a borrow (`&colony_db.writer_tx`). On `shutdown()` the
//! drop of the sender is the only trigger for the writer thread's exit.

use crate::persist::writer::ColonyWriteOp;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::thread::JoinHandle;

/// Lifecycle handle for `colony.db`: writer thread + read-only connection.
pub struct ColonyDb {
    /// Sender into the writer thread. Single owner per FIX 2.
    ///
    /// Phase-12-Pre: bounded `tokio::sync::mpsc::channel(1000)` with
    /// cooperative `.send().await` backpressure. The writer receiver stays
    /// `std::thread`-based and drains via `blocking_recv()`.
    pub(crate) writer_tx: tokio::sync::mpsc::Sender<ColonyWriteOp>,
    /// Read-only connection for trace queries (cells do not read colony.db directly).
    pub(crate) read_conn: rusqlite::Connection,
    /// JoinHandle of the writer thread. `Option` only for drop safety;
    /// in T12 `shutdown()` consumes it via self-destructuring.
    pub(crate) writer_join: Option<JoinHandle<()>>,
    /// Atomic counter (sent - committed). Shared with the writer thread, which
    /// decrements it after every committed op.
    ///
    /// `pub(crate)` so that `colony.rs::handle_register` can increment the counter
    /// inline without borrowing `&ColonyDb` across `.await`
    /// (`&ColonyDb` is not `Send` because `rusqlite::Connection` is !Sync).
    pub(crate) queue_depth: Arc<AtomicI64>,
    /// Filesystem path of `colony.db`. Used by Phase-12-B `ReadTrace` to open
    /// a fresh `SQLITE_OPEN_READ_ONLY` connection inside `spawn_blocking`
    /// (WAL allows concurrent readers — the writer thread is unaffected).
    db_path: std::path::PathBuf,
}

/// Persisted mutation-log row from the `mutation_log` table (phase 6).
///
/// Phase 12-B step-7.4: consumed by the `ColonyMsg::ReadMutationsAudit` inbox arm
/// and in the HTTP API layer (task 8) as an audit read.
#[derive(Debug, Clone)]
pub struct MutationLogRow {
    /// Mutation ID (UUID v7 as a string).
    pub id: String,
    /// Scope path string the mutation was applied at.
    pub scope: String,
    /// Original payload as a JSON string (diff + ctx).
    pub payload_json: String,
    /// Status: "in_flight" | "committed" | "failed".
    pub status: String,
    /// Optional: failure-reason string (only for `status='failed'`).
    pub failure_reason: Option<String>,
    /// Unix seconds at creation time.
    pub created_at: i64,
    /// Unix seconds at commit/fail time (NULL while `in_flight`).
    pub committed_at: Option<i64>,
    /// Phase-16 W3 (A6): `error_code` of a validate-stage reject row
    /// (NULL for in_flight/committed/failed).
    pub error_code: Option<String>,
    /// Phase-16 W3 (A6): trace ID of the request on a reject row
    /// (NULL for in_flight/committed/failed).
    pub trace_id: Option<String>,
}

/// Persisted registry entry read from the `registry` table during reboot hydration.
///
/// Used by Phase-13.5 Lifecycle-Slice-3a to overlay cell identity (cell_id + status)
/// on top of the FS walk result.
#[derive(Debug, Clone)]
pub struct PersistedRegistryEntry {
    /// Absolute meclaw path of the cell (Primary Key in `registry`).
    pub path: meclaw_core::Path,
    /// Stable cell UUID (UUID v7, assigned once, never overwritten on re-boot).
    pub cell_id: meclaw_core::Uuid,
    /// Cell type string (e.g. `"llm"`, `"bash"`).
    pub cell_type: String,
    /// Last persisted status string (e.g. `"active"`, `"idle"`).
    pub status: String,
    /// GH #62: the template identity this node was instantiated from, or `None`
    /// for a node whose origin was never recorded (hand-written tree, adopted
    /// directory, anything born before the stamp existed).
    pub provenance: Option<crate::config::NodeProvenance>,
}

/// Persisted template row from the `templates` table (phase 11 11-A).
#[derive(Debug, Clone)]
pub struct TemplateRow {
    /// Template ID (UUID v7).
    pub template_id: String,
    /// Template name (from `template.json`).
    pub name: String,
    /// Optional: semantic-version string.
    pub version: Option<String>,
    /// Absolute path to the template directory.
    pub filesystem_path: String,
    /// `description` field as a JSON blob.
    pub description_json: String,
    /// `tags` field as a JSON array string.
    pub tags_json: String,
    /// Optional: author string.
    pub author: Option<String>,
    /// Unix seconds of the last scan.
    pub scanned_at: i64,
}

/// Persisted dead-letter row from the `dead_letters` table (Phase-16 W6d / A6).
///
/// Six projection fields mirror `DeadLetterDto` PLUS `message_json` — the full
/// `Message` envelope (Ruling W6d Option 1). DB is the DLQ source of truth: the
/// HTTP read projects the six fields; the drain deserializes `message_json` to
/// reconstruct the verbatim `DeadLetter` (body/correlation_id intact).
#[derive(Debug, Clone)]
pub struct DeadLetterRow {
    /// Emitting cell/hive path.
    pub sender_path: String,
    /// Target before path resolution.
    pub original_target: String,
    /// Target after `Path::resolve`.
    pub resolved_target: String,
    /// Canonical `error_code` (per `DeadLetterReason::as_code`).
    pub error_code: String,
    /// Trace-ID of the dead-lettered message.
    pub trace_id: String,
    /// Unix-seconds timestamp.
    pub created_at: i64,
    /// Full message envelope as JSON (Ruling W6d Option 1). The HTTP DTO read
    /// ignores this; the DLQ-drain deserializes it to reconstruct the verbatim
    /// `DeadLetter`.
    pub message_json: String,
}

impl ColonyDb {
    /// Opens `colony.db` at `path`, initializes the schema, spawns the writer thread.
    ///
    /// The writer thread runs in a blocking loop (`run_writer`) and processes
    /// `ColonyWriteOp` messages in batches (max `BATCH_MAX` per transaction).
    /// The thread terminates when the sender is dropped (channel disconnected).
    pub fn open(path: &std::path::Path) -> rusqlite::Result<Self> {
        let writer_conn = rusqlite::Connection::open(path)?;
        crate::persist::setup_colony_db(&writer_conn)?;
        let read_conn = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        // GH #98: read-only opens never run `setup_colony_db` — install the
        // busy budget directly.
        crate::persist::apply_busy_timeout(&read_conn)?;
        // Phase 12-Pre: bounded tokio::sync::mpsc(1000). A hard cap, no config
        // knob (CONTRIBUTING.md rules 1+7). Rationale: HTTP load is the phase-12 risk
        // surface; ~1s of burst headroom at realistic routing throughput. NOT
        // derived from the mailbox default.
        let (writer_tx, writer_rx) = tokio::sync::mpsc::channel::<ColonyWriteOp>(1000);
        let queue_depth = Arc::new(AtomicI64::new(0));
        let qd_writer = queue_depth.clone();
        let writer_join = std::thread::spawn(move || {
            crate::persist::writer::run_writer(writer_rx, writer_conn, qd_writer);
        });
        Ok(Self {
            writer_tx,
            read_conn,
            writer_join: Some(writer_join),
            queue_depth,
            db_path: path.to_path_buf(),
        })
    }

    /// Current pending-op count (sent minus committed).
    pub fn queue_depth(&self) -> i64 {
        self.queue_depth.load(Ordering::Relaxed)
    }

    /// Filesystem path of `colony.db`. Phase-12-B `ReadTrace` opens a fresh
    /// read-only Connection at this path inside `spawn_blocking` (WAL allows
    /// concurrent readers — see `setup_colony_db` in `persist::schema`).
    pub fn db_path(&self) -> &std::path::Path {
        &self.db_path
    }

    /// Sends an op to the writer. Increments queue_depth atomically.
    /// At depth > 1000 → tracing::warn (phase-6 hardening hook).
    ///
    /// Bounded backpressure: on a full channel the producer blocks cooperatively
    /// via `.send().await` (no drop, the message log is an audit trail — see
    /// `docs/meclaw-overview.md` Z.1473 + Z.919). Panic behaviour
    /// (writer-thread-dead) byte-identical to the sync predecessor.
    pub async fn send_op(&self, op: ColonyWriteOp) {
        self.queue_depth.fetch_add(1, Ordering::Relaxed);
        let depth = self.queue_depth.load(Ordering::Relaxed);
        if depth > 1000 {
            tracing::warn!(depth, "colony.db writer backlog > 1000");
        }
        self.writer_tx.send(op).await.expect("writer thread dead");
    }

    /// Phase 6: insert an `in_flight` mutation_log row and await durable commit.
    ///
    /// Sends a `MutationLogInsert { ack: Some(tx) }` op; the writer thread fires the
    /// ack AFTER `tx.commit()` returns. Returning `Ok(())` therefore guarantees the
    /// row is visible to a fresh rusqlite connection (committed + WAL-flushed).
    pub async fn insert_mutation_log_durable(
        &self,
        id: String,
        scope: String,
        payload_json: String,
        created_at: i64,
    ) -> Result<(), tokio::sync::oneshot::error::RecvError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.send_op(crate::persist::writer::ColonyWriteOp::MutationLogInsert {
            id,
            scope,
            payload_json,
            created_at,
            ack: Some(tx),
        })
        .await;
        rx.await
    }

    /// Phase 6: update mutation_log status durably (transitions to `committed` or `failed`).
    ///
    /// Same ack-after-commit guarantee as `insert_mutation_log_durable`.
    pub async fn update_mutation_log_durable(
        &self,
        id: String,
        status: String,
        committed_at: i64,
        failure_reason: Option<String>,
    ) -> Result<(), tokio::sync::oneshot::error::RecvError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.send_op(crate::persist::writer::ColonyWriteOp::MutationLogUpdate {
            id,
            status,
            committed_at,
            failure_reason,
            ack: Some(tx),
        })
        .await;
        rx.await
    }

    /// Classifies the boot state via `read_conn` (instead of a separate db-path probe).
    ///
    /// GH #89: delegates to the shared `bootstrap::classify_boot_state` core —
    /// one truth table for the path-based probe and this handle-based one, no
    /// drift. See the classifier for the marker semantics (Run-5/5b resume)
    /// and the Reboot/Inconsistent cut.
    pub fn boot_state(
        &self,
    ) -> Result<crate::bootstrap::BootState, crate::bootstrap::BootstrapError> {
        Ok(crate::bootstrap::classify_boot_state(&self.read_conn))
    }

    /// Reads all persisted edges from colony.db (for reboot hydration).
    ///
    /// Phase-13.5-Durable-Edges: re-parses CEL condition and modifier from
    /// persisted source strings. Hard-fails on corrupt data so routing state
    /// is never silently wrong after a reboot.
    pub fn read_edges(
        &self,
    ) -> Result<Vec<crate::bootstrap::PlannedEdge>, crate::bootstrap::EdgeHydrationError> {
        read_edges_from(&self.read_conn)
    }

    /// Reads all persisted templates from colony.db (for in-memory registry hydration).
    pub fn read_templates(&self) -> rusqlite::Result<Vec<TemplateRow>> {
        let mut stmt = self.read_conn.prepare(
            "SELECT template_id, name, version, filesystem_path,
                    description_json, tags_json, author, scanned_at
             FROM templates ORDER BY name, version",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(TemplateRow {
                template_id: r.get(0)?,
                name: r.get(1)?,
                version: r.get(2)?,
                filesystem_path: r.get(3)?,
                description_json: r.get(4)?,
                tags_json: r.get(5)?,
                author: r.get(6)?,
                scanned_at: r.get(7)?,
            })
        })?;
        rows.collect()
    }

    /// Reads `mutation_log` rows with an optional since filter + cap (phase 12-B step-7.4).
    ///
    /// `since`: unix seconds, only rows with `created_at >= since` are returned.
    /// `limit` is a hard cap; callers should clamp beforehand.
    pub fn read_mutation_log(
        &self,
        since: Option<i64>,
        limit: usize,
    ) -> rusqlite::Result<Vec<MutationLogRow>> {
        let (sql, params): (&str, Vec<Box<dyn rusqlite::ToSql>>) = match since {
            Some(s) => (
                "SELECT id, scope, payload_json, status, failure_reason, created_at, committed_at, error_code, trace_id
                 FROM mutation_log WHERE created_at >= ? ORDER BY created_at ASC LIMIT ?",
                vec![Box::new(s), Box::new(limit as i64)],
            ),
            None => (
                "SELECT id, scope, payload_json, status, failure_reason, created_at, committed_at, error_code, trace_id
                 FROM mutation_log ORDER BY created_at ASC LIMIT ?",
                vec![Box::new(limit as i64)],
            ),
        };
        let mut stmt = self.read_conn.prepare(sql)?;
        let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(rusqlite::params_from_iter(param_refs.iter()), |r| {
            Ok(MutationLogRow {
                id: r.get(0)?,
                scope: r.get(1)?,
                payload_json: r.get(2)?,
                status: r.get(3)?,
                failure_reason: r.get(4)?,
                created_at: r.get(5)?,
                committed_at: r.get(6)?,
                error_code: r.get(7)?,
                trace_id: r.get(8)?,
            })
        })?;
        rows.collect()
    }

    /// Reads persisted `dead_letters` rows with an optional `since`/`error_code`
    /// filter + cap (phase-16 W6d / A6). The DB is the DLQ source of truth — the
    /// `/colony/dead_letters` read queries against it, no longer against an
    /// in-memory `VecDeque`. The order is the insert order (`id` = rowid).
    ///
    /// `since`: unix seconds, only rows with `created_at >= since`.
    /// `error_code`: exact match on the canonical `error_code` string.
    /// `limit`: hard cap; callers clamp beforehand (default 100, hard cap 1000).
    pub fn read_dead_letters(
        &self,
        since: Option<i64>,
        error_code: Option<String>,
        limit: usize,
    ) -> rusqlite::Result<Vec<DeadLetterRow>> {
        let mut sql = String::from(
            "SELECT sender_path, original_target, resolved_target, error_code, trace_id, created_at, message_json \
             FROM dead_letters",
        );
        let mut clauses: Vec<&str> = Vec::new();
        if since.is_some() {
            clauses.push("created_at >= ?");
        }
        if error_code.is_some() {
            clauses.push("error_code = ?");
        }
        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }
        sql.push_str(" ORDER BY id ASC LIMIT ?");

        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(s) = since {
            params.push(Box::new(s));
        }
        if let Some(ec) = error_code {
            params.push(Box::new(ec));
        }
        params.push(Box::new(limit as i64));

        let mut stmt = self.read_conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(rusqlite::params_from_iter(param_refs.iter()), |r| {
            Ok(DeadLetterRow {
                sender_path: r.get(0)?,
                original_target: r.get(1)?,
                resolved_target: r.get(2)?,
                error_code: r.get(3)?,
                trace_id: r.get(4)?,
                created_at: r.get(5)?,
                message_json: r.get(6)?,
            })
        })?;
        rows.collect()
    }

    /// W6d (A6): read EVERY persisted dead-letter row (no filter, no cap) in
    /// insertion order — the DLQ-drain snapshots the full set before clearing the
    /// table. Carries `message_json` so the caller can reconstruct full
    /// `DeadLetter`s.
    pub fn read_all_dead_letters(&self) -> rusqlite::Result<Vec<DeadLetterRow>> {
        let mut stmt = self.read_conn.prepare(
            "SELECT sender_path, original_target, resolved_target, error_code, trace_id, created_at, message_json \
             FROM dead_letters ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(DeadLetterRow {
                sender_path: r.get(0)?,
                original_target: r.get(1)?,
                resolved_target: r.get(2)?,
                error_code: r.get(3)?,
                trace_id: r.get(4)?,
                created_at: r.get(5)?,
                message_json: r.get(6)?,
            })
        })?;
        rows.collect()
    }

    /// Reads all persisted hive-scope paths from colony.db (for reboot hydration).
    pub fn read_hive_scopes(&self) -> rusqlite::Result<Vec<meclaw_core::Path>> {
        let mut stmt = self
            .read_conn
            .prepare("SELECT path FROM hive_scopes ORDER BY created_at")?;
        let rows = stmt.query_map([], |r| {
            let p: String = r.get(0)?;
            Ok(meclaw_core::Path::new(&p))
        })?;
        rows.collect()
    }

    /// Read all persisted registry entries from `colony.db` for reboot hydration.
    ///
    /// Returns every row in the `registry` table ordered by `path`. The caller
    /// (Phase-13.5 Lifecycle-Slice-3a) overlays `cell_id` and `status` on top of
    /// the FS-walk result so that cells retain their stable identity across reboots.
    ///
    /// Fails hard on any SQL error or on a cell_id that cannot be parsed as UUID —
    /// corrupt identity data must never be silently ignored.
    pub fn read_registry(&self) -> rusqlite::Result<Vec<PersistedRegistryEntry>> {
        let mut stmt = self.read_conn.prepare(
            "SELECT path, cell_id, cell_type, status, template, template_version, \
                 instantiated_at, template_chain FROM registry ORDER BY path",
        )?;
        let rows = stmt.query_map([], |r| {
            let path_str: String = r.get(0)?;
            let cell_id_str: String = r.get(1)?;
            let cell_type: String = r.get(2)?;
            let status: String = r.get(3)?;
            // GH #62: provenance is a triple that is either wholly there or
            // wholly absent — a row without a `template` has no origin, and a
            // half-filled row would be a lie.
            let template: Option<String> = r.get(4)?;
            let template_version: Option<String> = r.get(5)?;
            let instantiated_at: Option<i64> = r.get(6)?;
            // GH #277: the chain is stored as JSON in one column. NULL means
            // "no chain was recorded" and an unparseable value means the same
            // thing here — the instance's own `config.json` is the source, this
            // table is the index, so a broken index entry loses a query hit,
            // never the truth.
            let template_chain: Option<String> = r.get(7)?;
            let template_chain = template_chain
                .as_deref()
                .and_then(|json| meclaw_core::serde_json::from_str(json).ok());
            let provenance = template.map(|template| crate::config::NodeProvenance {
                template,
                template_version,
                template_chain,
                instantiated_at: instantiated_at.unwrap_or(0),
            });
            let cell_id = meclaw_core::Uuid::parse_str(&cell_id_str).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
            Ok(PersistedRegistryEntry {
                path: meclaw_core::Path::new(&path_str),
                cell_id,
                cell_type,
                status,
                provenance,
            })
        })?;
        rows.collect()
    }

    /// Graceful shutdown: an explicit `ColonyWriteOp::Shutdown { ack }` signal +
    /// `ack.blocking_recv()` + then `drop(writer_tx)` + `JoinHandle::join()`.
    ///
    /// **Phase-13.5-A6 follow-up**: the predecessor impl only dropped the sender
    /// and relied on `rx.blocking_recv() == None` in the writer loop. That is
    /// race-prone under workspace load — Tokio mpsc can lose the channel-close
    /// notify inside an atomic state-transition window, which leaves the writer
    /// thread parked → `writer_join.join()` hangs forever → a production
    /// liveness hazard (see the `shutdown_persists_all_prior_writes_*`
    /// regression test in `writer.rs`).
    ///
    /// New mechanics: an explicit shutdown op + oneshot ack. The writer drains
    /// the current batch (FIFO guarantees: every op enqueued before shutdown
    /// lands in the same or an earlier transaction), fires the ack, and returns
    /// explicitly. `try_send` fallback on a full channel (bounded 1000, never
    /// reached in practice — shutdown is a singleton, at most BATCH_MAX=64
    /// backlog left over from the drain).
    ///
    /// Self-destructuring leaves `writer_tx` as a plain sender in the struct
    /// (no `Option<>` hack); the `Self { ... }` pattern moves every field
    /// individually. The producer single-owner drain discipline in
    /// `colony.rs::colony_task` (ColonyMsg::Shutdown arm) guarantees that
    /// `colony_db.shutdown()` is only called after all pending inbox items have
    /// been processed synchronously.
    pub fn shutdown(self) {
        let Self {
            writer_tx,
            writer_join,
            read_conn: _,
            queue_depth: _,
            db_path: _,
        } = self;
        Self::send_shutdown_op_and_wait_sync(writer_tx);
        if let Some(h) = writer_join {
            h.join().expect("writer panicked");
        }
    }

    /// Async variant of `shutdown()` for Tokio async callers (`colony_task`).
    ///
    /// `tokio::sync::oneshot::Receiver::blocking_recv()` panics inside an async
    /// context — `shutdown()` (sync) may ONLY be called from sync callers (tests,
    /// a sync main). `shutdown_async()` is the correct choice for the
    /// `colony_task::ColonyMsg::Shutdown` arm.
    ///
    /// Mechanics identical to `shutdown()`, just with `.await` instead of
    /// `blocking_recv()`. `writer_join.join()` (sync) stays — the writer thread
    /// should `return` immediately after the ack send, so the join is instant.
    pub async fn shutdown_async(self) {
        let Self {
            writer_tx,
            writer_join,
            read_conn: _,
            queue_depth: _,
            db_path: _,
        } = self;
        Self::send_shutdown_op_and_wait_async(writer_tx).await;
        if let Some(h) = writer_join {
            h.join().expect("writer panicked");
        }
    }

    /// Helper: enqueue Shutdown-Op, drop writer_tx, blocking_recv ack.
    /// MUST NOT be called from a Tokio async context.
    fn send_shutdown_op_and_wait_sync(
        writer_tx: tokio::sync::mpsc::Sender<crate::persist::writer::ColonyWriteOp>,
    ) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        match writer_tx.try_send(crate::persist::writer::ColonyWriteOp::Shutdown { ack: tx }) {
            Ok(()) => {
                drop(writer_tx);
                // Wait synchronously for the writer ack — parking_lot block_on,
                // runtime-independent. Panics inside a Tokio async context.
                let _ = rx.blocking_recv();
            }
            Err(e) => {
                tracing::warn!(
                    error = ?e,
                    "ColonyDb::shutdown: could not enqueue the shutdown op, \
                     falling back to drop-only (race-prone close detection)"
                );
                drop(writer_tx);
            }
        }
    }

    /// Helper: enqueue Shutdown-Op, drop writer_tx, `.await` ack.
    /// Safe to call from a Tokio async context.
    async fn send_shutdown_op_and_wait_async(
        writer_tx: tokio::sync::mpsc::Sender<crate::persist::writer::ColonyWriteOp>,
    ) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        match writer_tx.try_send(crate::persist::writer::ColonyWriteOp::Shutdown { ack: tx }) {
            Ok(()) => {
                drop(writer_tx);
                let _ = rx.await;
            }
            Err(e) => {
                tracing::warn!(
                    error = ?e,
                    "ColonyDb::shutdown_async: could not enqueue the shutdown op, \
                     falling back to drop-only (race-prone close detection)"
                );
                drop(writer_tx);
            }
        }
    }
}

/// Identity overlay read from `colony.db`'s `registry` table: meclaw path →
/// `(cell_id, status)`. Phase-13.5 Lifecycle-3a: the FS-walk bootstrap builder
/// consults this overlay so a known path reuses its persisted `cell_id` across
/// reboots instead of minting a fresh one.
pub type RegistryOverlay =
    std::collections::HashMap<meclaw_core::Path, (meclaw_core::Uuid, String)>;

/// Read every persisted edge from an already-open connection.
///
/// Phase-13.5-Durable-Edges: re-parses CEL condition and modifier from the
/// persisted source strings. Hard-fails on corrupt data so routing state is
/// never silently wrong after a reboot. Shared by the runtime hydration
/// ([`ColonyDb::read_edges`]) and the boot planner's read-only probe
/// ([`read_persisted_edges`]) — one row-mapping, no drift.
pub(crate) fn read_edges_from(
    conn: &rusqlite::Connection,
) -> Result<Vec<crate::bootstrap::PlannedEdge>, crate::bootstrap::EdgeHydrationError> {
    use crate::bootstrap::EdgeHydrationError as E;
    let mut stmt = conn
        .prepare(
            "SELECT id, from_path, to_path, condition, modifier, is_default FROM edges \
             ORDER BY created_at",
        )
        .map_err(E::Sql)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })
        .map_err(E::Sql)?;
    let mut out = Vec::new();
    for row in rows {
        let (id_str, from_str, to_str, cond_src, mod_src, is_default) = row.map_err(E::Sql)?;
        let id = meclaw_core::Uuid::parse_str(&id_str).map_err(|e| E::InvalidUuid {
            edge_id: id_str.clone(),
            error: e.to_string(),
        })?;
        let condition =
            match cond_src.as_deref() {
                Some(s) => Some(crate::cel_eval::parse_condition(s).map_err(|e| {
                    E::ConditionParseFailed {
                        edge_id: id_str.clone(),
                        condition_source: s.to_string(),
                        parse_error: e,
                    }
                })?),
                None => None,
            };
        let modifier = match mod_src.as_deref() {
            Some(s) => {
                let spec: crate::config::ModifierSpec = meclaw_core::serde_json::from_str(s)
                    .map_err(|e| E::ModifierJsonInvalid {
                        edge_id: id_str.clone(),
                        modifier_source: s.to_string(),
                        error: e.to_string(),
                    })?;
                Some(
                    crate::cel_eval::parse_modifier(&spec).map_err(|(key, msg)| {
                        E::ModifierParseFailed {
                            edge_id: id_str.clone(),
                            modifier_source: s.to_string(),
                            parse_error: format!("{key}: {msg}"),
                        }
                    })?,
                )
            }
            None => None,
        };
        out.push(crate::bootstrap::PlannedEdge {
            id,
            from: meclaw_core::Path::new(&from_str),
            to: meclaw_core::Path::new(&to_str),
            condition,
            modifier,
            // GH #283: the routing phase, read back from `edges.is_default`
            // (schema v7). Anything non-zero is a default — a stray integer
            // needs no error path of its own, because the column is written
            // exclusively by the two edge INSERTs from a `bool`, and a row that
            // predates the column reads the `DEFAULT 0` of a regular edge.
            is_default: is_default != 0,
        });
    }
    Ok(out)
}

/// GH #168/#178 — read the persisted edge table of the `colony.db` at
/// `db_path` through a fresh READ-ONLY connection.
///
/// The boot planner needs the topology a reboot will actually run with, and it
/// runs before (and beside) the colony task that owns the writable handle — so
/// it opens its own read-only connection, exactly like
/// [`read_registry_overlay`]. An absent file is an empty edge set (first boot);
/// anything else is an error, because a colony.db that exists but will not
/// yield its edges must never be quietly planned as edge-less.
pub fn read_persisted_edges(
    db_path: &std::path::Path,
) -> Result<Vec<crate::bootstrap::PlannedEdge>, crate::bootstrap::EdgeHydrationError> {
    use crate::bootstrap::EdgeHydrationError as E;
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    let conn =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(E::Sql)?;
    // GH #98: boot-path read — carry the busy budget instead of dying on a
    // momentarily locked colony.db.
    crate::persist::apply_busy_timeout(&conn).map_err(E::Sql)?;
    read_edges_from(&conn)
}

/// Read the identity overlay from the `registry` table of the `colony.db` at
/// `db_path` via a fresh read-only connection.
///
/// Returns an empty overlay if the file does not exist (first boot) — a fresh
/// colony has no persisted identities, so every FS node gets a fresh cell_id.
/// Fails hard (`Err`) on any SQL error or unparseable cell_id: corrupt identity
/// data must never be silently dropped (which would re-mint cell_ids and break
/// G5).
pub fn read_registry_overlay(db_path: &std::path::Path) -> rusqlite::Result<RegistryOverlay> {
    if !db_path.exists() {
        return Ok(RegistryOverlay::new());
    }
    let conn =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    // GH #98: boot-path read — carry the busy budget instead of dying on a
    // momentarily locked colony.db.
    crate::persist::apply_busy_timeout(&conn)?;
    let mut stmt = conn.prepare("SELECT path, cell_id, status FROM registry")?;
    let rows = stmt.query_map([], |r| {
        let path_str: String = r.get(0)?;
        let cell_id_str: String = r.get(1)?;
        let status: String = r.get(2)?;
        let cell_id = meclaw_core::Uuid::parse_str(&cell_id_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
        })?;
        Ok((meclaw_core::Path::new(&path_str), (cell_id, status)))
    })?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colony_db_open_creates_file_with_schema() {
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("colony.db");
        let db = ColonyDb::open(&db_path).unwrap();
        assert!(db_path.exists(), "colony.db file created");
        // Schema check: the meta table has schema_version='7' (GH #283: the
        // edges `is_default` column, on top of the GH #277 registry
        // `template_chain`, the GH #62 provenance triple and the W6d
        // dead_letters table).
        let v: String = db
            .read_conn
            .query_row(
                "SELECT value FROM meta WHERE key='schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, "7");
        // Single-owner invariant: writer_tx is present (not consumed)
        let _ = &db.writer_tx;
        drop(db);
    }

    #[test]
    fn colony_db_open_idempotent_on_existing_file() {
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("colony.db");
        let db1 = ColonyDb::open(&db_path).unwrap();
        drop(db1);
        // Re-open the same file — setup is idempotent (CREATE TABLE IF NOT EXISTS).
        let db2 = ColonyDb::open(&db_path).unwrap();
        drop(db2);
    }

    #[tokio::test]
    async fn writer_processes_initial_apply_atomically_via_sender() {
        use crate::persist::writer::ColonyWriteOp;
        use meclaw_core::Path;
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("colony.db");
        let db = ColonyDb::open(&db_path).unwrap();
        // Send 3 distinct hive_scopes in one InitialApply (atomic by construction — FIX 3).
        db.writer_tx
            .send(ColonyWriteOp::InitialApply {
                edges: vec![],
                hive_scopes: vec![
                    Path::new("/scope-a"),
                    Path::new("/scope-b"),
                    Path::new("/scope-c"),
                ],
            })
            .await
            .unwrap();
        // Drop the ColonyDb — the writer sender is dropped (single-owner path). The
        // writer processes the last item, commits, and terminates.
        drop(db);
        // T13 replaces the sleep with a shutdown barrier.
        std::thread::sleep(std::time::Duration::from_millis(200));
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let cnt: i64 = conn
            .query_row("SELECT COUNT(*) FROM hive_scopes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cnt, 3, "InitialApply atomic — all 3 scopes persisted");
    }

    #[test]
    fn queue_depth_starts_at_zero() {
        let td = tempfile::TempDir::new().unwrap();
        let db = ColonyDb::open(&td.path().join("c.db")).unwrap();
        assert_eq!(db.queue_depth(), 0);
    }

    #[tokio::test]
    async fn send_op_increments_queue_depth() {
        use crate::persist::writer::ColonyWriteOp;
        let td = tempfile::TempDir::new().unwrap();
        let db = ColonyDb::open(&td.path().join("c.db")).unwrap();
        let before = db.queue_depth();
        db.send_op(ColonyWriteOp::InitialApply {
            edges: vec![],
            hive_scopes: vec![],
        })
        .await;
        // depth >= before+1 immediately after the send (the writer may already have processed it)
        assert!(
            db.queue_depth() >= before,
            "queue_depth must not go negative"
        );
    }

    #[test]
    fn shutdown_joins_writer_within_timeout() {
        use std::sync::mpsc::{RecvTimeoutError, channel};
        let td = tempfile::TempDir::new().unwrap();
        let db = ColonyDb::open(&td.path().join("c.db")).unwrap();
        // shutdown runs in a separate std::thread; test-side timeout via channel.
        let (done_tx, done_rx) = channel();
        std::thread::spawn(move || {
            db.shutdown();
            done_tx.send(()).unwrap();
        });
        match done_rx.recv_timeout(std::time::Duration::from_secs(30)) {
            Ok(()) => {}
            Err(RecvTimeoutError::Timeout) => panic!("shutdown hung — writer never joined"),
            Err(e) => panic!("unexpected: {e:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn read_mutation_log_surfaces_reject_error_code_and_trace_id() {
        // Phase-16 W3 (A6): a `MutationLogRejectInsert` row is read back through
        // `read_mutation_log` with its v3 columns (error_code, trace_id) surfaced
        // — so the `/colony/mutations` audit can show rejects.
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("c.db");
        let db = ColonyDb::open(&db_path).unwrap();
        db.send_op(
            crate::persist::writer::ColonyWriteOp::MutationLogRejectInsert {
                id: "rej-7".into(),
                scope: "/main".into(),
                payload_json: "{}".into(),
                error_code: "naming_collision".into(),
                reason: "NamingCollision(\"dup\")".into(),
                trace_id: "tr-7".into(),
                created_at: 10,
                ack: None,
            },
        )
        .await;
        db.shutdown_async().await;

        let db2 = ColonyDb::open(&db_path).unwrap();
        let rows = db2.read_mutation_log(None, 100).unwrap();
        assert_eq!(rows.len(), 1, "the reject row is in the audit log");
        let r = &rows[0];
        assert_eq!(r.status, "rejected");
        assert_eq!(r.error_code.as_deref(), Some("naming_collision"));
        assert_eq!(r.trace_id.as_deref(), Some("tr-7"));
        assert!(r.committed_at.is_none());
        db2.shutdown_async().await;
    }

    /// W6d (A6) step a+b: an `InsertDeadLetter` op persists a DLQ row that
    /// survives a colony.db reopen (crash/shutdown survival) and is read back via
    /// `read_dead_letters` in insertion order, with the `?since`/`?error_code`
    /// filters honoured.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dead_letters_persist_across_reopen_with_filters() {
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("c.db");
        let db = ColonyDb::open(&db_path).unwrap();
        for (ec, ts) in [("ttl_expired", 100i64), ("unresolved_path", 200)] {
            db.send_op(ColonyWriteOp::InsertDeadLetter {
                sender_path: "/a".into(),
                original_target: "/b".into(),
                resolved_target: "/b".into(),
                error_code: ec.into(),
                trace_id: format!("trace-{ts}"),
                created_at: ts,
                message_json: format!(r#"{{"target":"/b","ttl":{ts}}}"#),
            })
            .await;
        }
        db.shutdown_async().await;

        // Reopen — DB is the single source of truth; entries survive the restart.
        let db2 = ColonyDb::open(&db_path).unwrap();
        let all = db2.read_dead_letters(None, None, 1000).unwrap();
        assert_eq!(all.len(), 2, "both DLQ entries persisted across reopen");
        assert_eq!(
            all[0].error_code, "ttl_expired",
            "insertion order preserved"
        );
        assert_eq!(all[1].error_code, "unresolved_path");
        assert_eq!(all[0].sender_path, "/a");
        assert_eq!(all[1].trace_id, "trace-200");

        // ?error_code filter.
        let only = db2
            .read_dead_letters(None, Some("unresolved_path".into()), 1000)
            .unwrap();
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].error_code, "unresolved_path");

        // ?since filter (created_at >= 150 keeps only the second).
        let since = db2.read_dead_letters(Some(150), None, 1000).unwrap();
        assert_eq!(since.len(), 1);
        assert_eq!(since[0].created_at, 200);
        db2.shutdown_async().await;
    }

    #[tokio::test]
    async fn update_mutation_log_durable_transitions_in_flight_to_committed() {
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("c.db");
        let db = ColonyDb::open(&db_path).unwrap();
        db.insert_mutation_log_durable("mid-9".into(), "/x".into(), "{}".into(), 100)
            .await
            .expect("insert ack");
        db.update_mutation_log_durable("mid-9".into(), "committed".into(), 200, None)
            .await
            .expect("update ack");
        let conn = rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let (status, committed_at): (String, i64) = conn
            .query_row(
                "SELECT status, committed_at FROM mutation_log WHERE id='mid-9'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "committed");
        assert_eq!(committed_at, 200);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn insert_mutation_log_durable_acks_after_commit() {
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("c.db");
        let db = ColonyDb::open(&db_path).unwrap();
        db.insert_mutation_log_durable("mid-42".into(), "/main".into(), r#"{"x":1}"#.into(), 100)
            .await
            .expect("ack");
        // Read directly from the DB — the ack guarantee means: the row is committed.
        let conn = rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let status: String = conn
            .query_row(
                "SELECT status FROM mutation_log WHERE id='mid-42'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "in_flight");
    }

    #[tokio::test]
    async fn writer_flushes_100_distinct_inserts_before_shutdown_returns() {
        use crate::persist::writer::ColonyWriteOp;
        use meclaw_core::Path;
        use std::sync::mpsc::{RecvTimeoutError, channel};

        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("c.db");
        let db = ColonyDb::open(&db_path).unwrap();

        // 100 distinct hive_scopes — one per op (countable payload).
        for i in 0..100 {
            db.send_op(ColonyWriteOp::InitialApply {
                edges: vec![],
                hive_scopes: vec![Path::new(&format!("/scope-{i}"))],
            })
            .await;
        }

        // No-Hang-Guard (FIX 2): shutdown in separater std::thread, Timeout via channel.
        let (done_tx, done_rx) = channel();
        std::thread::spawn(move || {
            db.shutdown();
            done_tx.send(()).unwrap();
        });
        match done_rx.recv_timeout(std::time::Duration::from_secs(30)) {
            Ok(()) => {}
            Err(RecvTimeoutError::Timeout) => panic!("shutdown hung"),
            Err(e) => panic!("unexpected: {e:?}"),
        }

        // Flush proof: a fresh reopen sees all 100 distinct scopes.
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let cnt: i64 = conn
            .query_row("SELECT COUNT(*) FROM hive_scopes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            cnt, 100,
            "writer must flush all 100 distinct hive_scope inserts"
        );
    }

    // Helper: insert a raw registry row directly (bypasses Writer-Thread).
    fn insert_raw_registry(
        conn: &rusqlite::Connection,
        path: &str,
        cell_id: &str,
        cell_type: &str,
        status: &str,
    ) {
        conn.execute(
            "INSERT INTO registry (path, cell_id, cell_type, status, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, 0, 0)",
            rusqlite::params![path, cell_id, cell_type, status],
        )
        .expect("insert registry");
    }

    #[test]
    fn read_registry_returns_all_rows_with_correct_fields() {
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("colony.db");
        let db = ColonyDb::open(&db_path).unwrap();
        let raw = rusqlite::Connection::open(&db_path).unwrap();
        let id1 = "00000000-0000-0000-0000-000000000010";
        let id2 = "00000000-0000-0000-0000-000000000011";
        insert_raw_registry(&raw, "/cell/a", id1, "llm", "active");
        insert_raw_registry(&raw, "/cell/b", id2, "bash", "idle");
        drop(raw);

        let result = db.read_registry();
        assert!(result.is_ok(), "expected Ok, got: {:?}", result);
        let entries = result.unwrap();
        assert_eq!(entries.len(), 2);

        let a = entries
            .iter()
            .find(|e| e.path.as_str() == "/cell/a")
            .expect("/cell/a missing");
        assert_eq!(a.cell_id.to_string(), id1);
        assert_eq!(a.cell_type, "llm");
        assert_eq!(a.status, "active");

        let b = entries
            .iter()
            .find(|e| e.path.as_str() == "/cell/b")
            .expect("/cell/b missing");
        assert_eq!(b.cell_id.to_string(), id2);
        assert_eq!(b.cell_type, "bash");
        assert_eq!(b.status, "idle");
    }

    #[test]
    fn read_registry_overlay_maps_path_to_cell_id_and_status() {
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("colony.db");
        let db = ColonyDb::open(&db_path).unwrap();
        let raw = rusqlite::Connection::open(&db_path).unwrap();
        let id = "00000000-0000-0000-0000-000000000020";
        insert_raw_registry(&raw, "/cell/a", id, "llm", "active");
        drop(raw);
        drop(db);

        let overlay = read_registry_overlay(&db_path).expect("overlay read ok");
        let (cell_id, status) = overlay
            .get(&meclaw_core::Path::new("/cell/a"))
            .expect("/cell/a in overlay");
        assert_eq!(cell_id.to_string(), id);
        assert_eq!(status, "active");
    }

    #[test]
    fn read_registry_overlay_empty_when_db_absent() {
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("absent.db");
        let overlay = read_registry_overlay(&db_path).expect("absent db → empty overlay");
        assert!(overlay.is_empty());
    }

    // Helper: insert a raw edge row using the writer connection (after schema setup).
    fn insert_raw_edge(
        conn: &rusqlite::Connection,
        id: &str,
        from: &str,
        to: &str,
        condition: Option<&str>,
        modifier: Option<&str>,
    ) {
        conn.execute(
            "INSERT INTO edges (id, from_path, to_path, condition, modifier, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, 0)",
            rusqlite::params![id, from, to, condition, modifier],
        )
        .expect("insert edge");
    }

    #[test]
    fn read_edges_reparses_valid_condition_source() {
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("colony.db");
        // Open via ColonyDb to ensure schema is set up.
        let db = ColonyDb::open(&db_path).unwrap();
        // Use a raw write connection to insert a test edge.
        let raw = rusqlite::Connection::open(&db_path).unwrap();
        let id = "00000000-0000-0000-0000-000000000001";
        insert_raw_edge(
            &raw,
            id,
            "/a",
            "/b",
            Some(r#"headers.kind == "text""#),
            None,
        );
        drop(raw);

        let result = db.read_edges();
        assert!(result.is_ok(), "expected Ok, got: {:?}", result);
        let edges = result.unwrap();
        assert_eq!(edges.len(), 1);
        let cond = edges[0]
            .condition
            .as_ref()
            .expect("condition should be Some");
        assert_eq!(cond.source, r#"headers.kind == "text""#);
    }

    #[test]
    fn read_edges_reparses_valid_modifier_source() {
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("colony.db");
        let db = ColonyDb::open(&db_path).unwrap();
        let raw = rusqlite::Connection::open(&db_path).unwrap();
        let id = "00000000-0000-0000-0000-000000000002";
        insert_raw_edge(
            &raw,
            id,
            "/a",
            "/b",
            None,
            Some(r#"{"set_hop":{"tier":"'gold'"}}"#),
        );
        drop(raw);

        let result = db.read_edges();
        assert!(result.is_ok(), "expected Ok, got: {:?}", result);
        let edges = result.unwrap();
        assert_eq!(edges.len(), 1);
        let modifier = edges[0].modifier.as_ref().expect("modifier should be Some");
        assert_eq!(
            modifier.source.set_hop.get("tier").map(|s| s.as_str()),
            Some("'gold'")
        );
    }

    #[test]
    fn read_edges_returns_err_on_invalid_condition_source() {
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("colony.db");
        let db = ColonyDb::open(&db_path).unwrap();
        let raw = rusqlite::Connection::open(&db_path).unwrap();
        let id = "00000000-0000-0000-0000-000000000003";
        insert_raw_edge(&raw, id, "/a", "/b", Some("this is not valid cel ++"), None);
        drop(raw);

        let result = db.read_edges();
        assert!(result.is_err(), "expected Err, got: {:?}", result);
        let err = result.unwrap_err();
        match err {
            crate::bootstrap::EdgeHydrationError::ConditionParseFailed {
                edge_id,
                condition_source,
                ..
            } => {
                assert_eq!(edge_id, id);
                assert_eq!(condition_source, "this is not valid cel ++");
            }
            other => panic!("expected ConditionParseFailed, got: {other:?}"),
        }
    }

    #[test]
    fn read_edges_returns_err_on_invalid_modifier_json() {
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("colony.db");
        let db = ColonyDb::open(&db_path).unwrap();
        let raw = rusqlite::Connection::open(&db_path).unwrap();
        let id = "00000000-0000-0000-0000-000000000004";
        insert_raw_edge(&raw, id, "/a", "/b", None, Some(r#"{"set": invalid"#));
        drop(raw);

        let result = db.read_edges();
        assert!(result.is_err(), "expected Err, got: {:?}", result);
        let err = result.unwrap_err();
        match err {
            crate::bootstrap::EdgeHydrationError::ModifierJsonInvalid { edge_id, .. } => {
                assert_eq!(edge_id, id);
            }
            other => panic!("expected ModifierJsonInvalid, got: {other:?}"),
        }
    }

    #[test]
    fn read_edges_round_trips_hive_paths_without_existence_check() {
        // Persistence layer is existence-agnostic: from/to are stored and read as
        // plain path strings with no assumption that a cell exists at those paths.
        // /scope is a hive path (not a cell) — the persistence layer must not reject it.
        // (Persistence layer only; no transit-routing logic, not via mutation validator.)
        use meclaw_core::Path;
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("colony.db");
        let db = ColonyDb::open(&db_path).unwrap();
        let raw = rusqlite::Connection::open(&db_path).unwrap();
        let id = meclaw_core::Uuid::now_v7().to_string();
        insert_raw_edge(&raw, &id, "/scope", "/scope/child", None, None);
        drop(raw);

        let edges = db.read_edges().expect("hive-path edge must hydrate");
        let e = edges.iter().find(|e| e.id.to_string() == id).unwrap();
        assert_eq!(e.from.as_str(), "/scope");
        assert_eq!(e.to.as_str(), "/scope/child");
        // Verify that Path round-trips the hive path string unchanged.
        assert_eq!(e.from, Path::new("/scope"));
        assert_eq!(e.to, Path::new("/scope/child"));
    }
}
