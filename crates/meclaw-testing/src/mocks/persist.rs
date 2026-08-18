//! `PersistMockCell` — counter-based test cell for the phase-5 restore tests.
//!
//! Variants:
//! - T25 (current): counter + conn only.
//! - T26: + overlay_from_db.
//! - T28: + write_snapshot + Cell trait impl.
//! - T29: + panic_after (E6 canonical order).
//! - T37: + terminal (E7, no output → the cascade stops) + emitted_target.
//!
//! # `params.emitted_target` is a field, not a delivery address (GH #226)
//!
//! The param carries the same name and the same meaning as
//! `EchoCellFactory`'s (GH #224): it writes the `target` field of this cell's
//! emission and decides nothing about where that emission goes. The colony's
//! outputs arm routes by the EMITTING cell's out-edges — a matching edge
//! overlays the target, and an emission matching no out-edge dead-letters as
//! `no_route` whatever this field says. A test topology therefore needs BOTH
//! this param and an out-edge.
//!
//! It was called `echo_to` until GH #226, and it fell back to `msg.target`
//! when absent, which turned a forgotten param into a self-loop emission
//! nobody asked for. There is no fallback any more: a cell that emits must say
//! where, and a cell that emits nothing says `terminal: true`.

/// Counter-based test cell. Increments counter per handle() call and persists
/// state via cell.db (T28).
///
/// Phase-6.5: connection ownership lives in the `cell_task_stateful` stack frame
/// (StatefulCell trait). The cell no longer has a `conn` field.
pub struct PersistMockCell {
    /// Incremented per handle() call.
    pub counter: i64,
    /// Test-only panic hook: if `Some(n)` and `counter == n` after the snapshot,
    /// panic before the output (plan E6 canonical order).
    pub panic_after: Option<i64>,
    /// Plan E7: if `true`, `handle()` emits no output → the cascade stops.
    pub terminal: bool,
    /// T37 / plan E8: the `target` field written on this cell's emission —
    /// NOT a route (see the module doc). `None` only for a `terminal` cell,
    /// which emits nothing at all.
    pub emitted_target: Option<meclaw_core::Path>,
}

impl PersistMockCell {
    /// Construct a fresh cell from params.
    /// `panic_after`: optional test field — when set, the cell panics after the
    /// counter has reached this value.
    /// `terminal`: if `true`, the cell emits no output (cascade stop).
    /// `emitted_target`: the `target` field of the emission — required unless
    /// the cell is `terminal`, because a cell that emits has to name what it
    /// writes into that field. Absence used to mean `msg.target`; see the
    /// module doc for why that fallback is gone (GH #226).
    pub fn from_params(params: &meclaw_core::JsonValue) -> Result<Self, String> {
        let panic_after = params.get("panic_after").and_then(|v| v.as_i64());
        let terminal = params
            .get("terminal")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let emitted_target = match params.get("emitted_target").and_then(|v| v.as_str()) {
            Some(s) => Some(meclaw_core::Path::new(s)),
            None if terminal => None,
            None => {
                return Err("params.emitted_target missing or not a string \
                    (required unless params.terminal is true)"
                    .to_string());
            }
        };
        Ok(Self {
            counter: 0,
            panic_after,
            terminal,
            emitted_target,
        })
    }

    /// Test helper: increments counter without output/snapshot.
    /// The real Cell trait impl (T28) does counter++ + output + snapshot + panic check.
    pub fn handle_dummy(&mut self) {
        self.counter += 1;
    }

    /// Loads counter from cell.db (system table, slot_path='counter').
    /// Absent → no-op. Cold boot ≡ restart (plan E4).
    pub fn overlay_from_db(&mut self, conn: &rusqlite::Connection) -> rusqlite::Result<()> {
        match conn.query_row(
            "SELECT value FROM system WHERE slot_path='counter'",
            [],
            |r| r.get::<_, String>(0),
        ) {
            Ok(v) => self.counter = v.parse().unwrap_or(0),
            Err(rusqlite::Error::QueryReturnedNoRows) => {}
            Err(e) => return Err(e),
        }
        Ok(())
    }
}

impl PersistMockCell {
    /// Writes a snapshot of counter + last_input into cell.db.
    ///
    /// Free-style helper: takes counter and last_input_json as params so that
    /// `Cell::handle` can call it without conflicting borrows on `self`.
    ///
    /// Plan E3 / Phase-5-Pragma: snapshot is written synchronously before the
    /// async output emit. For atomic-1-output cells (all Phase-5 cells) this
    /// ordering is semantically equivalent to E3 (snapshot-after-output). The
    /// distinction becomes relevant in Phase 7+ when multi-send is introduced.
    ///
    /// Emits a `tracing::warn!` when the underlying `snapshot_tx` takes >5 ms
    /// (Phase-13 empirical latency hook).
    pub fn write_snapshot_with(
        conn: &mut rusqlite::Connection,
        counter: i64,
        last_input_json: &str,
    ) -> rusqlite::Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_secs() as i64;
        let start = std::time::Instant::now();
        meclaw_colony::persist::snapshot_tx(
            conn,
            &[("counter".to_string(), counter.to_string())],
            last_input_json,
            now,
        )?;
        let elapsed = start.elapsed();
        if elapsed > std::time::Duration::from_millis(5) {
            tracing::warn!(
                elapsed_ms = elapsed.as_millis() as u64,
                "cell.db snapshot slow"
            );
        }
        Ok(())
    }
}

/// `StatefulCell` trait implementation on `PersistMockCell` (phase-9 A5).
///
/// E6 canonical order: counter++ → db.call(snapshot).await → panic-check (sync,
/// before output) → output emit (async).
///
/// Panic before output: a panicking cell emits no output (no cascade, no extra
/// message_log hop). Snapshot before panic: the restart overlay sees the
/// pre-panic counter value.
///
/// Phase-9 A5: cell.db `Connection` is wrapped in `meclaw_colony::DbConn` and
/// arrives as `&mut DbConn` — sync ops are dispatched via `db.call(|c| { ... })`.
impl meclaw_colony::stateful_cell::StatefulCell for PersistMockCell {
    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a mut self,
        msg: meclaw_core::Message,
        sink: &'a meclaw_core::OutputSink,
        db: &'a mut meclaw_colony::DbConn,
    ) -> impl std::future::Future<Output = ()> + Send + 'a {
        async move {
            // 1. SYNC: counter++ (outside db.call — counter lives in a self field).
            self.counter += 1;
            let counter = self.counter;
            let last_input_json = match &msg.body {
                meclaw_core::Body::Inline(v) => {
                    meclaw_core::serde_json::to_string(v).unwrap_or_default()
                }
                meclaw_core::Body::Blob(id) => format!(r#"{{"blob":"{}"}}"#, id),
            };
            // 2. SYNC inside db.call: snapshot (before panic so restart overlay
            //    sees pre-panic counter). Owned-move last_input_json.
            let _ = db
                .call(move |c| PersistMockCell::write_snapshot_with(c, counter, &last_input_json))
                .await;
            // 3. SYNC: panic-check before output — panicking cell emits no output.
            if Some(counter) == self.panic_after {
                panic!("PersistMockCell panic_after triggered at counter={counter}");
            }
            // 4. ASYNC: output emit (skipped when terminal=true → cascade stops).
            // target: the configured emitted_target — no fallback (GH #226).
            if let Some(target) = self.emitted_target.clone()
                && !self.terminal
            {
                let _ = sink
                    .push(meclaw_core::CellOutput {
                        target,
                        content: meclaw_core::serde_json::json!({
                            "messages": [],
                            "header": {"counter": counter}
                        }),
                    })
                    .await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn persist_mock_cell_increments_counter_on_handle_dummy() {
        use meclaw_core::serde_json::json;
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("cell.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        meclaw_colony::persist::setup_cell_db(&conn).unwrap();
        let mut c = PersistMockCell::from_params(&json!({"terminal": true})).unwrap();
        c.handle_dummy();
        assert_eq!(c.counter, 1);
        c.handle_dummy();
        assert_eq!(c.counter, 2);
    }

    /// GH #226: a non-terminal cell must name the target it writes. The
    /// Phase-5 pragma (no strict param schema for the mock cell) still holds
    /// for every other field — this is the one required one, and a non-object
    /// `params` carries none of them.
    #[test]
    fn persist_mock_cell_from_params_rejects_non_object() {
        let p = meclaw_core::serde_json::json!(42);
        let Err(err) = PersistMockCell::from_params(&p) else {
            panic!("expected a rejection, got a cell")
        };
        assert!(err.contains("emitted_target"), "got: {err}");
    }

    /// GH #226: the param is required for an emitting cell and absent for a
    /// terminal one — no silent fallback to `msg.target` in either case.
    #[test]
    fn from_params_requires_emitted_target_unless_terminal() {
        use meclaw_core::serde_json::json;
        let Err(err) = PersistMockCell::from_params(&json!({})) else {
            panic!("expected a rejection, got a cell")
        };
        assert!(
            err.contains("params.emitted_target"),
            "an emitting cell without a target must say so, got: {err}"
        );
        let terminal = PersistMockCell::from_params(&json!({"terminal": true}))
            .expect("a terminal cell emits nothing and needs no target");
        assert!(terminal.emitted_target.is_none());
        let emitting = PersistMockCell::from_params(&json!({"emitted_target": "/sink"}))
            .expect("a named target is accepted");
        assert_eq!(emitting.emitted_target.unwrap().as_str(), "/sink");
    }

    /// GH #226: the emission carries the configured target verbatim. Before the
    /// fix an absent param produced an emission to `msg.target` — a self-loop
    /// the test author never asked for; now there is nothing to fall back to.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_emits_to_the_configured_target_only() {
        use meclaw_colony::stateful_cell::StatefulCell;
        use meclaw_core::serde_json::json;
        use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
        use tokio::sync::mpsc;

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        meclaw_colony::persist::setup_cell_db(&conn).unwrap();
        let mut db = meclaw_colony::DbConn::wrap(conn, None);
        let mut cell = PersistMockCell::from_params(&json!({"emitted_target": "/sink"})).unwrap();
        let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
        let sink = OutputSink::new(
            tx,
            Path::new("/p"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            64,
            meclaw_core::Headers::new(),
            None,
        );
        let msg = MessageBuilder::new(Path::new("/p"))
            .body(Body::Inline(json!({"messages": []})))
            .build();
        cell.handle(msg, &sink, &mut db).await;
        let em = rx.recv().await.expect("one emission");
        assert_eq!(
            em.target.as_str(),
            "/sink",
            "the emission target is the configured one, not the inbound path"
        );
    }

    #[test]
    fn overlay_from_db_restores_counter() {
        use meclaw_core::serde_json::json;
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("cell.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        meclaw_colony::persist::setup_cell_db(&conn).unwrap();
        conn.execute(
            "INSERT INTO system (slot_path, value, updated_at) VALUES (?, ?, ?)",
            rusqlite::params!["counter", "42", 0],
        )
        .unwrap();
        let mut c = PersistMockCell::from_params(&json!({"terminal": true})).unwrap();
        c.overlay_from_db(&conn).unwrap();
        assert_eq!(c.counter, 42);
    }

    #[test]
    fn overlay_from_db_no_op_on_empty_db() {
        use meclaw_core::serde_json::json;
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("cell.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        meclaw_colony::persist::setup_cell_db(&conn).unwrap();
        let mut c = PersistMockCell::from_params(&json!({"terminal": true})).unwrap();
        c.overlay_from_db(&conn).unwrap();
        assert_eq!(
            c.counter, 0,
            "empty system table → counter stays at bootstrap (0)"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn persist_mock_handle_writes_snapshot_via_stateful_param() {
        use meclaw_colony::stateful_cell::StatefulCell;
        use meclaw_core::serde_json::json;
        use meclaw_core::{Body, MessageBuilder, OutputSink, Path, Uuid};
        use tokio::sync::mpsc;

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        meclaw_colony::persist::setup_cell_db(&conn).unwrap();
        let mut db = meclaw_colony::DbConn::wrap(conn, None);

        let mut cell = PersistMockCell::from_params(&json!({"emitted_target": "/sink"})).unwrap();
        let (tx, _rx) = mpsc::channel(8);
        let sink = OutputSink::new(
            tx,
            Path::new("/p"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            64,
            meclaw_core::Headers::new(),
            None,
        );
        let msg = MessageBuilder::new(Path::new("/p"))
            .body(Body::Inline(json!({"messages": []})))
            .build();
        cell.handle(msg, &sink, &mut db).await;

        // Read back via db.call (the connection now lives in DbConn).
        let v: String = db
            .call(|c| {
                c.query_row(
                    "SELECT value FROM system WHERE slot_path='counter'",
                    [],
                    |r| r.get(0),
                )
                .unwrap()
            })
            .await;
        assert_eq!(v, "1");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn snapshot_written_after_handle_completes() {
        use meclaw_core::serde_json::json;
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("cell.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        meclaw_colony::persist::setup_cell_db(&conn).unwrap();
        drop(conn);
        let mut conn2 = rusqlite::Connection::open(&db_path).unwrap();
        let mut c = PersistMockCell::from_params(&json!({"terminal": true})).unwrap();
        c.counter = 7;
        // Phase-6.5: the cell.db connection lives externally (cell_task_stateful frame).
        // write_snapshot directly (without the StatefulCell trait), as a helper test.
        PersistMockCell::write_snapshot_with(&mut conn2, c.counter, r#"{"msg":"x"}"#).unwrap();
        let v: String = conn2
            .query_row(
                "SELECT value FROM system WHERE slot_path='counter'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, "7");
    }
}
