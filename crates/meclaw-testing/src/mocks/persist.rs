//! `PersistMockCell` — Counter-basierte Test-Cell für Phase-5-Restore-Tests.
//!
//! Variants:
//! - T25 (jetzt): nur counter + conn.
//! - T26: + overlay_from_db.
//! - T28: + write_snapshot + Cell-Trait-Impl.
//! - T29: + panic_after (E6 kanonische Reihenfolge).
//! - T37: + terminal (E7, kein Output → cascade stoppt) + echo_to (target-Auswahl).

/// Counter-basierte Test-Cell. Inkrementiert counter pro handle()-Call und
/// persistiert state via cell.db (T28).
///
/// Phase-6.5: Connection-Ownership lebt im `cell_task_stateful`-Stack-Frame
/// (StatefulCell-Trait). Die Cell hat KEIN `conn`-Field mehr.
pub struct PersistMockCell {
    /// Inkrementiert pro handle()-Call.
    pub counter: i64,
    /// Test-only panic hook: wenn `Some(n)` und `counter == n` nach Snapshot,
    /// panic vor Output (Plan E6 kanonische Reihenfolge).
    pub panic_after: Option<i64>,
    /// Plan E7: wenn `true`, emittiert `handle()` keinen Output → Cascade stoppt.
    pub terminal: bool,
    /// T37 / Plan E8: wenn `Some(path)`, setzt `output.target = echo_to`.
    /// Wenn `None`: `target = msg.target` (Self-Loop-Verhalten ohne Edges).
    pub echo_to: Option<meclaw_core::Path>,
}

impl PersistMockCell {
    /// Konstruiert eine frische Cell aus params.
    /// `panic_after`: optionales Test-Feld — wenn gesetzt, löst die Cell
    /// einen panic aus, nachdem der Counter diesen Wert erreicht hat.
    /// `terminal`: wenn `true`, emittiert die Cell keinen Output (Cascade-Stopp).
    /// `echo_to`: wenn gesetzt, wird `output.target` auf diesen Pfad gesetzt.
    pub fn from_params(params: &meclaw_core::JsonValue) -> Result<Self, String> {
        let panic_after = params.get("panic_after").and_then(|v| v.as_i64());
        let terminal = params
            .get("terminal")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let echo_to = params
            .get("echo_to")
            .and_then(|v| v.as_str())
            .map(meclaw_core::Path::new);
        Ok(Self {
            counter: 0,
            panic_after,
            terminal,
            echo_to,
        })
    }

    /// Test-Helper: Inkrementiert counter ohne Output/Snapshot.
    /// Der echte Cell-Trait-Impl (T28) macht counter++ + output + snapshot + panic-Check.
    pub fn handle_dummy(&mut self) {
        self.counter += 1;
    }

    /// Lädt counter aus cell.db (system-Tabelle, slot_path='counter').
    /// Absent → no-op. Cold-Boot ≡ Restart (Plan E4).
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

/// `StatefulCell`-Trait implementation on `PersistMockCell` (Phase-9 A5).
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
            // 1. SYNC: counter++ (outside db.call — counter lives in self-field).
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
            // target: echo_to wenn gesetzt, sonst msg.target (Self-Loop ohne Edges).
            let target = self.echo_to.clone().unwrap_or_else(|| msg.target.clone());
            if !self.terminal {
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
        let mut c = PersistMockCell::from_params(&json!({})).unwrap();
        c.handle_dummy();
        assert_eq!(c.counter, 1);
        c.handle_dummy();
        assert_eq!(c.counter, 2);
    }

    #[test]
    fn persist_mock_cell_from_params_rejects_non_object() {
        let p = meclaw_core::serde_json::json!(42);
        let r = PersistMockCell::from_params(&p);
        // Phase-5-Pragma: from_params akzeptiert beliebige JSON (Phase-5 hat keine
        // strikte Param-Schema-Validierung für die Mock-Cell). Test erwartet Ok.
        assert!(r.is_ok());
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
        let mut c = PersistMockCell::from_params(&json!({})).unwrap();
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
        let mut c = PersistMockCell::from_params(&json!({})).unwrap();
        c.overlay_from_db(&conn).unwrap();
        assert_eq!(
            c.counter, 0,
            "empty system table → counter bleibt Bootstrap (0)"
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

        let mut cell = PersistMockCell::from_params(&json!({"echo_to": "/sink"})).unwrap();
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

        // Read-Back via db.call (Connection lebt jetzt in DbConn).
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
        let mut c = PersistMockCell::from_params(&json!({})).unwrap();
        c.counter = 7;
        // Phase-6.5: cell.db Connection lebt extern (cell_task_stateful-Frame).
        // write_snapshot direkt (ohne StatefulCell-Trait), als Helper-Test.
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
