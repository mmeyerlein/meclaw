//! `HangMockCell` — stateful test cell for the Paket-3 message_timeout
//! B-backstop demos.
//!
//! Two configurable behaviours via `params`:
//! - `hang_forever: true` → `handle()` does `std::future::pending().await`
//!   (never returns) — only the substrate B-backstop can end it. Used to
//!   prove the backstop FIRES (demo requirement #2).
//! - `sleep_ms: <n>` → `handle()` sleeps `n` ms, then emits one UBF body to
//!   `emitted_target`. A measurable-but-finite handle —
//!   used to prove that `cell.message_timeout: 0`/`-1` DISABLES the backstop
//!   (demo requirement #3): with the backstop off, a 300ms handle completes
//!   normally even though a small backstop would have killed it.
//!
//! # `params.emitted_target` is a field, not a delivery address (GH #226)
//!
//! Same name, same meaning as `EchoCellFactory`'s (GH #224): it writes the
//! `target` field of this cell's emission and decides nothing about where the
//! emission goes — the out-edge of the EMITTING cell does, and an emission
//! matching none dead-letters as `no_route`. It was called `echo_to` until
//! GH #226 and fell back to `msg.target` when absent, so a forgotten param
//! became a self-loop emission instead of a spawn error. The fallback is gone:
//! a cell that reaches the emit needs the param, and `hang_forever: true` —
//! which never reaches it — does not.
//!
//! Phase-6.5 connection ownership: the cell holds NO `conn` field — the
//! `cell.db` `DbConn` arrives as `&mut DbConn` in `handle()`. This cell does
//! not touch `cell.db` at all (it is a pure timing fixture); the parameter is
//! present only to satisfy the `StatefulCell` trait.

/// Stateful timing fixture. `handle()` either hangs forever or sleeps a
/// configurable duration before emitting a single output.
pub struct HangMockCell {
    /// If `true`, `handle()` hangs forever (`std::future::pending`).
    pub hang_forever: bool,
    /// Sleep duration before emitting (ignored when `hang_forever`).
    pub sleep_ms: u64,
    /// The `target` field written on the emission — NOT a route (module doc).
    /// `None` only for `hang_forever`, which never reaches the emit.
    pub emitted_target: Option<meclaw_core::Path>,
}

impl HangMockCell {
    /// Construct from `params`.
    /// - `hang_forever` (bool, default `false`).
    /// - `sleep_ms` (u64, default `0`).
    /// - `emitted_target` (string path) — required unless `hang_forever`,
    ///   which never reaches the emit. No `msg.target` fallback (GH #226).
    pub fn from_params(params: &meclaw_core::JsonValue) -> Result<Self, String> {
        let hang_forever = params
            .get("hang_forever")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let sleep_ms = params.get("sleep_ms").and_then(|v| v.as_u64()).unwrap_or(0);
        let emitted_target = match params.get("emitted_target").and_then(|v| v.as_str()) {
            Some(s) => Some(meclaw_core::Path::new(s)),
            None if hang_forever => None,
            None => {
                return Err("params.emitted_target missing or not a string \
                    (required unless params.hang_forever is true)"
                    .to_string());
            }
        };
        Ok(Self {
            hang_forever,
            sleep_ms,
            emitted_target,
        })
    }
}

impl meclaw_colony::stateful_cell::StatefulCell for HangMockCell {
    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a mut self,
        _msg: meclaw_core::Message,
        sink: &'a meclaw_core::OutputSink,
        _db: &'a mut meclaw_colony::DbConn,
    ) -> impl std::future::Future<Output = ()> + Send + 'a {
        async move {
            if self.hang_forever {
                // Never returns — only the B-backstop can end this handle().
                std::future::pending::<()>().await;
            }
            if self.sleep_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(self.sleep_ms)).await;
            }
            // Emit a single valid UBF body so a `/sink` CaptureCell sees a
            // positive receipt (header.done:true marks a normal completion).
            // No fallback target (GH #226): `from_params` already refused a
            // reachable emit without one.
            let Some(target) = self.emitted_target.clone() else {
                return;
            };
            let _ = sink
                .push(meclaw_core::CellOutput {
                    target,
                    content: meclaw_core::serde_json::json!({
                        "header": {"done": true},
                        "messages": [
                            {"origin": "assistant", "type": "text", "text": "slept and finished"}
                        ]
                    }),
                })
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    #[test]
    fn from_params_defaults() {
        let c = HangMockCell::from_params(&json!({"hang_forever": true})).unwrap();
        assert!(c.hang_forever);
        assert_eq!(c.sleep_ms, 0);
        assert!(c.emitted_target.is_none());
    }

    #[test]
    fn from_params_reads_fields() {
        let c = HangMockCell::from_params(&json!({
            "hang_forever": true,
            "sleep_ms": 300,
            "emitted_target": "/sink"
        }))
        .unwrap();
        assert!(c.hang_forever);
        assert_eq!(c.sleep_ms, 300);
        assert_eq!(c.emitted_target.unwrap().as_str(), "/sink");
    }

    /// GH #226: a cell that reaches the emit must name the target it writes.
    /// Absence used to mean `msg.target` — a self-loop emission instead of a
    /// spawn error. `hang_forever` never reaches the emit, so it is exempt.
    #[test]
    fn from_params_requires_emitted_target_unless_hang_forever() {
        let Err(err) = HangMockCell::from_params(&json!({"sleep_ms": 5})) else {
            panic!("expected a rejection, got a cell")
        };
        assert!(
            err.contains("params.emitted_target"),
            "a sleeping cell without a target must say so, got: {err}"
        );
        assert!(
            HangMockCell::from_params(&json!({"hang_forever": true})).is_ok(),
            "a cell that never emits needs no target"
        );
    }

    /// GH #226: the emission carries the configured target verbatim — there is
    /// no inbound path to fall back to any more.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_emits_to_the_configured_target_only() {
        use meclaw_colony::stateful_cell::StatefulCell;
        use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
        use tokio::sync::mpsc;

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        meclaw_colony::persist::setup_cell_db(&conn).unwrap();
        let mut db = meclaw_colony::DbConn::wrap(conn, None);
        let mut cell =
            HangMockCell::from_params(&json!({"sleep_ms": 1, "emitted_target": "/sink"})).unwrap();
        let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
        let sink = OutputSink::new(
            tx,
            Path::new("/h"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            64,
            meclaw_core::Headers::new(),
            None,
        );
        let msg = MessageBuilder::new(Path::new("/h"))
            .body(Body::Inline(json!({"messages": []})))
            .build();
        cell.handle(msg, &sink, &mut db).await;
        let em = rx.recv().await.expect("one emission");
        assert_eq!(em.target.as_str(), "/sink");
    }
}
