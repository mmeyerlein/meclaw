//! `EmitOnceMockCell`: Stateful Mock-Cell, die genau einmal beim ersten
//! `handle()` einen vorgegebenen `CellOutput { target, content }` emittiert
//! und alle weiteren Eingangs-Messages über einen `mpsc::Sender<Message>`
//! captured. Stateful trait — ignoriert `cell.db` (kein Persistenz-Bedarf).
//!
//! Phase-13.5-A6-Verwendung: Cell emittiert `/colony/<endpoint>` ohne
//! manuelles `reply_to`. Outputs-Arm setzt reply_to automatisch via
//! `build_follow_up_with` (Spec Z.891). Reply von /colony kommt zurück
//! an die Cell und wird vom Capture-Sender im 2. handle()-Call empfangen.

use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::Value;
use meclaw_core::{CellOutput, Message, OutputSink, Path};
use tokio::sync::mpsc;

/// Stateful Mock-Cell für A6-E2E-Tests.
///
/// Erster `handle()`-Call: emittiert den pending `CellOutput` via `OutputSink::push`.
/// Der Trigger-Input wird NICHT captured (er war nur Auslöser).
/// Alle weiteren `handle()`-Calls: leiten die Eingangs-Message an `capture_tx` weiter.
pub struct EmitOnceMockCell {
    /// Initial-Emit-Spec; `Some` beim ersten `handle()`, danach `None`.
    pending_emit: Option<CellOutput>,
    /// Capture-Channel: alle handle()-Inputs nach dem ersten Emit landen hier.
    capture_tx: mpsc::Sender<Message>,
}

impl EmitOnceMockCell {
    /// Konstruktor: initialer Emit + Capture-Channel.
    pub fn new(
        initial_target: Path,
        initial_content: Value,
        capture_tx: mpsc::Sender<Message>,
    ) -> Self {
        Self {
            pending_emit: Some(CellOutput {
                target: initial_target,
                content: initial_content,
            }),
            capture_tx,
        }
    }
}

impl StatefulCell for EmitOnceMockCell {
    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a mut self,
        msg: Message,
        sink: &'a OutputSink,
        _db: &'a mut DbConn,
    ) -> impl std::future::Future<Output = ()> + Send + 'a {
        async move {
            if let Some(emit) = self.pending_emit.take() {
                let _ = sink.push(emit).await;
                // Erster Input wird IGNORIERT (er war der Trigger).
            } else {
                let _ = self.capture_tx.send(msg).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;
    use meclaw_core::{MessageBuilder, Uuid};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn emit_once_mock_emits_initial_then_captures() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let mut db = DbConn::wrap(conn, None);

        let (out_tx, mut out_rx) = mpsc::channel(8);
        let sink = OutputSink::new(
            out_tx,
            Path::new("/probe"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            64,
            meclaw_core::Headers::new(),
            None,
        );

        let (cap_tx, mut cap_rx) = mpsc::channel(8);
        let mut cell = EmitOnceMockCell::new(
            Path::new("/colony/registry"),
            json!({"query": {"limit": 100}}),
            cap_tx,
        );

        // First handle: emits, does NOT capture.
        let trig = MessageBuilder::new(Path::new("/probe")).build();
        cell.handle(trig, &sink, &mut db).await;
        let em = out_rx.try_recv().expect("first handle emits");
        assert_eq!(em.target.as_str(), "/colony/registry");
        assert!(cap_rx.try_recv().is_err(), "trigger is not captured");

        // Second handle: no emit, captures the input.
        let reply = MessageBuilder::new(Path::new("/probe")).build();
        let reply_id = reply.id;
        cell.handle(reply, &sink, &mut db).await;
        assert!(out_rx.try_recv().is_err(), "second handle emits nothing");
        let captured = cap_rx.try_recv().expect("second handle captures");
        assert_eq!(captured.id, reply_id);
    }
}
