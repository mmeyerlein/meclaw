//! `ColonyDb`-Struct: Writer-Thread + read-only-Connection für `colony.db`.
//!
//! **Single-Owner-Invariante (FIX 2, Marcus-Review 2026-05-20)**: `writer_tx` lebt
//! genau in einem Owner (`ColonyDb`), wird NIE in einen längerlebigen Scope geklont.
//! Alle Sends gehen per Borrow (`&colony_db.writer_tx`). Beim `shutdown()` ist das
//! Drop des Senders der einzige Auslöser für den Writer-Thread-Exit.

use crate::persist::writer::ColonyWriteOp;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::thread::JoinHandle;

/// Lifecycle-Handle für `colony.db`: Writer-Thread + read-only-Connection.
pub struct ColonyDb {
    /// Sender in den Writer-Thread. Single-Owner per FIX 2.
    ///
    /// Phase-12-Pre: bounded `tokio::sync::mpsc::channel(1000)` mit
    /// kooperativem `.send().await`-Backpressure. Writer-Receiver bleibt
    /// `std::thread`-based und drained via `blocking_recv()`.
    pub(crate) writer_tx: tokio::sync::mpsc::Sender<ColonyWriteOp>,
    /// Read-only-Connection für Trace-Queries (Cells lesen colony.db nicht direkt).
    pub(crate) read_conn: rusqlite::Connection,
    /// JoinHandle des Writer-Threads. `Option` nur für Drop-Sicherheit;
    /// in T12 `shutdown()` consumiert ihn via self-Destructuring.
    pub(crate) writer_join: Option<JoinHandle<()>>,
    /// Atomic counter (sent - committed). Shared mit Writer-Thread, der
    /// nach jedem committed Op dekrementiert.
    ///
    /// `pub(crate)` damit `colony.rs::handle_register` den Counter inline
    /// inkrementieren kann, ohne `&ColonyDb` über `.await` zu borrowen
    /// (`&ColonyDb` ist nicht `Send`, weil `rusqlite::Connection` !Sync ist).
    pub(crate) queue_depth: Arc<AtomicI64>,
    /// Filesystem path of `colony.db`. Used by Phase-12-B `ReadTrace` to open
    /// a fresh `SQLITE_OPEN_READ_ONLY`-Connection inside `spawn_blocking`
    /// (WAL allows concurrent readers — the writer thread is unaffected).
    db_path: std::path::PathBuf,
}

/// Persistierte Mutation-Log-Row aus der `mutation_log`-Tabelle (Phase 6).
///
/// Phase 12-B step-7.4: konsumiert von `ColonyMsg::ReadMutationsAudit`-Inbox-Arm
/// und in der HTTP-API-Schicht (Task 8) als Audit-Read.
#[derive(Debug, Clone)]
pub struct MutationLogRow {
    /// Mutation-ID (UUID v7 als String).
    pub id: String,
    /// Scope-Pfad-String, an dem die Mutation appliziert wurde.
    pub scope: String,
    /// Original-Payload als JSON-String (diff + ctx).
    pub payload_json: String,
    /// Status: "in_flight" | "committed" | "failed".
    pub status: String,
    /// Optional: Failure-Reason-String (nur bei `status='failed'`).
    pub failure_reason: Option<String>,
    /// Unix-Sekunden bei Anlage.
    pub created_at: i64,
    /// Unix-Sekunden bei Commit/Fail (NULL solange `in_flight`).
    pub committed_at: Option<i64>,
    /// Phase-16 W3 (A6): `error_code` einer Validate-Stage-Reject-Row
    /// (NULL bei in_flight/committed/failed).
    pub error_code: Option<String>,
    /// Phase-16 W3 (A6): Trace-ID des Antrags bei einer Reject-Row
    /// (NULL bei in_flight/committed/failed).
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
}

/// Persistierte Template-Row aus der `templates`-Tabelle (Phase 11 11-A).
#[derive(Debug, Clone)]
pub struct TemplateRow {
    /// Template-ID (UUID v7).
    pub template_id: String,
    /// Template-Name (aus `template.json`).
    pub name: String,
    /// Optional: Semantic-Version-String.
    pub version: Option<String>,
    /// Absoluter Pfad zum Template-Verzeichnis.
    pub filesystem_path: String,
    /// `description`-Feld als JSON-Blob.
    pub description_json: String,
    /// `tags`-Feld als JSON-Array-String.
    pub tags_json: String,
    /// Optional: Autor-String.
    pub author: Option<String>,
    /// Unix-Sekunden beim letzten Scan.
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
    /// Öffnet `colony.db` an `path`, initialisiert Schema, spawnt Writer-Thread.
    ///
    /// Der Writer-Thread läuft in einer blockierenden Loop (`run_writer`) und verarbeitet
    /// `ColonyWriteOp`-Nachrichten in Batches (max `BATCH_MAX` pro Transaktion).
    /// Der Thread beendet sich, wenn der Sender gedroppt wird (Channel disconnected).
    pub fn open(path: &std::path::Path) -> rusqlite::Result<Self> {
        let writer_conn = rusqlite::Connection::open(path)?;
        crate::persist::setup_colony_db(&writer_conn)?;
        let read_conn = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        // Phase 12-Pre: bounded tokio::sync::mpsc(1000). Hartes Cap, kein
        // Config-Knopf (CLAUDE.md Regel 1+7). Begründung: HTTP-Last ist die
        // Phase-12-Risikofläche; ~1s Burst-Headroom bei realistischem Routing-
        // Durchsatz. NICHT von Mailbox-Default abgeleitet.
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

    /// Sendet einen Op zum Writer. Inkrementiert queue_depth atomic.
    /// Bei depth > 1000 → tracing::warn (Phase-6 hardening hook).
    ///
    /// Bounded-Backpressure: bei vollem Channel blockt der Producer kooperativ
    /// via `.send().await` (kein Drop, Message-Log ist Audit-Trail — siehe
    /// `docs/meclaw-overview.md` Z.1473 + Z.919). Panic-Verhalten
    /// (writer-thread-dead) byte-identisch zum sync-Vorgänger.
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

    /// Klassifiziere Boot-State via `read_conn` (statt separater db-path-Probe).
    pub fn boot_state(
        &self,
    ) -> Result<crate::bootstrap::BootState, crate::bootstrap::BootstrapError> {
        // Bootstrap-Recovery (Run-5/5b): mirror of the marker check in
        // `bootstrap::probe_boot_state` (path-based probe) — a durable
        // `bootstrap_in_flight` marker means an interrupted FIRST apply;
        // classify as resumable FirstBoot, never Inconsistent (the apply path
        // is idempotent, the filesystem is the source).
        let marker: i64 = self
            .read_conn
            .query_row(
                "SELECT COUNT(*) FROM meta WHERE key='bootstrap_in_flight'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if marker > 0 {
            return Ok(crate::bootstrap::BootState::FirstBoot);
        }
        let mut counts: Vec<i64> = Vec::with_capacity(3);
        for table in &["registry", "edges", "hive_scopes"] {
            let q = format!("SELECT COUNT(*) FROM {table}");
            let c: i64 = self.read_conn.query_row(&q, [], |r| r.get(0)).unwrap_or(0);
            counts.push(c);
        }
        let all_empty = counts.iter().all(|&c| c == 0);
        let all_full = counts.iter().all(|&c| c > 0);
        if all_empty {
            Ok(crate::bootstrap::BootState::FirstBoot)
        } else if all_full {
            Ok(crate::bootstrap::BootState::Reboot)
        } else {
            Ok(crate::bootstrap::BootState::Inconsistent {
                reason: format!(
                    "table counts: registry={}, edges={}, hive_scopes={}",
                    counts[0], counts[1], counts[2]
                ),
            })
        }
    }

    /// Lese alle persistierten Edges aus colony.db (für Re-Boot-Hydration).
    ///
    /// Phase-13.5-Durable-Edges: re-parses CEL condition and modifier from
    /// persisted source strings. Hard-fails on corrupt data so routing state
    /// is never silently wrong after a reboot.
    pub fn read_edges(
        &self,
    ) -> Result<Vec<crate::bootstrap::PlannedEdge>, crate::bootstrap::EdgeHydrationError> {
        use crate::bootstrap::EdgeHydrationError as E;
        let mut stmt = self
            .read_conn
            .prepare(
                "SELECT id, from_path, to_path, condition, modifier FROM edges ORDER BY created_at",
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
                ))
            })
            .map_err(E::Sql)?;
        let mut out = Vec::new();
        for row in rows {
            let (id_str, from_str, to_str, cond_src, mod_src) = row.map_err(E::Sql)?;
            let id = meclaw_core::Uuid::parse_str(&id_str).map_err(|e| E::InvalidUuid {
                edge_id: id_str.clone(),
                error: e.to_string(),
            })?;
            let condition = match cond_src.as_deref() {
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
            });
        }
        Ok(out)
    }

    /// Lese alle persistierten Templates aus colony.db (für In-Memory-Registry-Hydration).
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

    /// Lese `mutation_log`-Rows mit optionalem since-Filter + Cap (Phase 12-B step-7.4).
    ///
    /// `since`: Unix-Sekunden, nur Rows mit `created_at >= since` werden zurückgegeben.
    /// `limit` ist ein hartes Cap; Caller sollten vorher clampen.
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

    /// Lese persistierte `dead_letters`-Rows mit optionalem `since`/`error_code`-
    /// Filter + Cap (Phase-16 W6d / A6). DB ist die DLQ-Source-of-Truth — der
    /// `/colony/dead_letters`-Read query't hierüber, nicht mehr einen In-Memory-
    /// `VecDeque`. Reihenfolge ist die Einfüge-Reihenfolge (`id` = rowid).
    ///
    /// `since`: Unix-Sekunden, nur Rows mit `created_at >= since`.
    /// `error_code`: exakter Match auf den kanonischen `error_code`-String.
    /// `limit`: hartes Cap; Caller clampen vorher (Default 100, Hard-Cap 1000).
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

    /// Lese alle persistierten Hive-Scope-Pfade aus colony.db (für Re-Boot-Hydration).
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
        let mut stmt = self
            .read_conn
            .prepare("SELECT path, cell_id, cell_type, status FROM registry ORDER BY path")?;
        let rows = stmt.query_map([], |r| {
            let path_str: String = r.get(0)?;
            let cell_id_str: String = r.get(1)?;
            let cell_type: String = r.get(2)?;
            let status: String = r.get(3)?;
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
            })
        })?;
        rows.collect()
    }

    /// Graceful Shutdown: explizites `ColonyWriteOp::Shutdown { ack }`-Signal +
    /// `ack.blocking_recv()` + dann `drop(writer_tx)` + `JoinHandle::join()`.
    ///
    /// **Phase-13.5-A6-followup**: Vorgänger-Impl drop'te nur den Sender und
    /// verließ sich auf `rx.blocking_recv() == None` im Writer-Loop. Das ist
    /// race-prone unter Workspace-Last — Tokio mpsc kann das channel-close-
    /// notify in einem atomaren state-transition-Fenster verlieren, was den
    /// Writer-Thread im park belässt → `writer_join.join()` hängt forever
    /// → Production-Liveness-Hazard (siehe `shutdown_persists_all_prior_writes_*`-
    /// Regression-Test in `writer.rs`).
    ///
    /// Neue Mechanik: explizite Shutdown-Op + oneshot-ack. Writer drained den
    /// aktuellen Batch (FIFO sichert: alle vor Shutdown enqueued'en Ops landen
    /// in derselben oder einer früheren Transaktion), feuert ack, returnt
    /// explizit. `try_send` Fallback bei voll-Channel (bounded 1000, in der
    /// Praxis nie erreicht — Shutdown ist Singleton, max BATCH_MAX=64 backlog
    /// vom drain übrig).
    ///
    /// Self-Destructuring lässt `writer_tx` als Plain-Sender im Struct
    /// (kein Option<>-Hack); das `Self { ... }`-Pattern moved jedes Feld
    /// einzeln. Producer-Single-Owner-Drain-Disziplin in `colony.rs::colony_task`
    /// (ColonyMsg::Shutdown-Arm) sichert, dass `colony_db.shutdown()` ERST
    /// gerufen wird, nachdem alle pending inbox-Items synchron verarbeitet sind.
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

    /// Async-Variante von `shutdown()` für Tokio-async-Caller (`colony_task`).
    ///
    /// `tokio::sync::oneshot::Receiver::blocking_recv()` panickt in einem
    /// async-Context — `shutdown()` (sync) darf NUR aus sync-Callers (Tests,
    /// sync main) gerufen werden. `shutdown_async()` ist die korrekte Wahl
    /// für `colony_task::ColonyMsg::Shutdown`-Arm.
    ///
    /// Mechanik identisch zu `shutdown()`, nur mit `.await` statt
    /// `blocking_recv()`. `writer_join.join()` (sync) bleibt — der Writer-
    /// Thread sollte nach ack-send sofort `return`'en, join ist instant.
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
                // Synchron auf Writer-Ack warten — parking_lot block_on,
                // runtime-unabhängig. Panickt wenn im Tokio-async-context.
                let _ = rx.blocking_recv();
            }
            Err(e) => {
                tracing::warn!(
                    error = ?e,
                    "ColonyDb::shutdown: Shutdown-Op konnte nicht eingestellt werden, \
                     Fallback auf drop-only (race-prone close-detection)"
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
                    "ColonyDb::shutdown_async: Shutdown-Op konnte nicht eingestellt werden, \
                     Fallback auf drop-only (race-prone close-detection)"
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
        // Schema check: meta-Tabelle hat schema_version='4' (Phase-16 W6d / A6:
        // persistente dead_letters-Tabelle).
        let v: String = db
            .read_conn
            .query_row(
                "SELECT value FROM meta WHERE key='schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, "4");
        // Single-Owner-Invariante: writer_tx ist vorhanden (nicht konsumiert)
        let _ = &db.writer_tx;
        drop(db);
    }

    #[test]
    fn colony_db_open_idempotent_on_existing_file() {
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("colony.db");
        let db1 = ColonyDb::open(&db_path).unwrap();
        drop(db1);
        // Re-open derselben Datei — Setup ist idempotent (CREATE TABLE IF NOT EXISTS).
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
        // Drop the ColonyDb — writer-Sender wird gedroppt (Single-Owner-Pfad). Writer
        // verarbeitet den letzten Item, committet, beendet sich.
        drop(db);
        // T13 ersetzt den sleep durch shutdown-Barrier.
        std::thread::sleep(std::time::Duration::from_millis(200));
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let cnt: i64 = conn
            .query_row("SELECT COUNT(*) FROM hive_scopes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cnt, 3, "InitialApply atomic — alle 3 scopes persistiert");
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
        // depth >= before+1 unmittelbar nach send (Writer kann das schon verarbeitet haben)
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
        // shutdown läuft in separater std::thread; Test-seitiger Timeout via channel.
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
        // Direkt aus DB lesen — Ack-Garantie heißt: Row committed.
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

        // 100 distinkte hive_scopes — eine pro Op (zählbares Payload).
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

        // Flush-Beweis: fresh-reopen sieht alle 100 distinkten scopes.
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
