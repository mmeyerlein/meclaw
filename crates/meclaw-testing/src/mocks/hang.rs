//! `HangMockCell` — stateful test cell for the Paket-3 message_timeout
//! B-backstop demos.
//!
//! Two configurable behaviours via `params`:
//! - `hang_forever: true` → `handle()` does `std::future::pending().await`
//!   (never returns) — only the substrate B-backstop can end it. Used to
//!   prove the backstop FIRES (demo requirement #2).
//! - `sleep_ms: <n>` → `handle()` sleeps `n` ms, then emits one UBF body to
//!   `echo_to` (or `msg.target` if absent). A measurable-but-finite handle —
//!   used to prove that `cell.message_timeout: 0`/`-1` DISABLES the backstop
//!   (demo requirement #3): with the backstop off, a 300ms handle completes
//!   normally even though a small backstop would have killed it.
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
    /// Output target. `Some(path)` → emit there; `None` → emit to `msg.target`.
    pub echo_to: Option<meclaw_core::Path>,
}

impl HangMockCell {
    /// Construct from `params`.
    /// - `hang_forever` (bool, default `false`).
    /// - `sleep_ms` (u64, default `0`).
    /// - `echo_to` (string path, optional).
    pub fn from_params(params: &meclaw_core::JsonValue) -> Result<Self, String> {
        let hang_forever = params
            .get("hang_forever")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let sleep_ms = params.get("sleep_ms").and_then(|v| v.as_u64()).unwrap_or(0);
        let echo_to = params
            .get("echo_to")
            .and_then(|v| v.as_str())
            .map(meclaw_core::Path::new);
        Ok(Self {
            hang_forever,
            sleep_ms,
            echo_to,
        })
    }
}

impl meclaw_colony::stateful_cell::StatefulCell for HangMockCell {
    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a mut self,
        msg: meclaw_core::Message,
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
            let target = self.echo_to.clone().unwrap_or_else(|| msg.target.clone());
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
        let c = HangMockCell::from_params(&json!({})).unwrap();
        assert!(!c.hang_forever);
        assert_eq!(c.sleep_ms, 0);
        assert!(c.echo_to.is_none());
    }

    #[test]
    fn from_params_reads_fields() {
        let c = HangMockCell::from_params(&json!({
            "hang_forever": true,
            "sleep_ms": 300,
            "echo_to": "/sink"
        }))
        .unwrap();
        assert!(c.hang_forever);
        assert_eq!(c.sleep_ms, 300);
        assert_eq!(c.echo_to.unwrap().as_str(), "/sink");
    }
}
