//! Issue #57 — the degraded `store` cell: what a wake returns when the cell's
//! `cell.db` cannot be opened at all.
//!
//! The factory's `WakeFn` runs synchronously inside the colony task, so a panic
//! there kills every cell in the process, not one (the panic-free colony hot
//! path invariant, A1′ class). A `store` cell without its database can do
//! nothing useful — but it must still be a cell, because the wake signature
//! (`WakeFn`) has to hand the colony a live mailbox plus stop-wiring back.
//!
//! Two rejected alternatives, both worse than answering with an error:
//! - **An in-memory substitute database.** Writes would look accepted and vanish
//!   at the next wake — data-loss masking, the one failure mode a store must
//!   never have.
//! - **Dropping the mailbox receiver** so the cell merely looks dead. The colony
//!   has already flipped the entry to `Awake` at that point, no watcher exists
//!   for a task that was never spawned, and the sender would fail silently per
//!   message. The requester learns nothing.
//!
//! So the degraded cell answers every message with an error message naming the
//! defect. `error_code` stays inside the closed `store` set of
//! `docs/cell-types.md` § store (`sql_error` — the database layer is what
//! failed); no new code is invented for a failure mode that is, from the
//! caller's side, "the database is unusable".
//!
//! Spec note: `cell-types.md` § store classifies internal errors (DB corruption,
//! spawn failure) as "cell crash + restart". That is exactly what the wake path
//! cannot express today — a crash inside the colony task is a COLONY crash. The
//! supervisor-visible variant needs a `Result`-returning `WakeFn` (issue #57
//! option 2, its own package); until then, answering loudly is the honest
//! substitute.

use meclaw_colony::StatelessCell;
use meclaw_core::serde_json::json;
use meclaw_core::{Body, CellOutput, Message, OutputSink};

/// A `store` cell that has no usable `cell.db`.
///
/// Stateless by construction — there is no state to hold, and the stateless
/// dispatcher needs no `DbConn`, which is precisely what is missing. Every
/// message is answered with `finish_reason: "error"` + `error_code: "sql_error"`
/// and a text naming the wake defect.
pub struct DegradedStoreCell {
    /// Operator-readable reason the wake could not build a real cell. Copied
    /// verbatim into the error turn's text.
    reason: String,
}

impl DegradedStoreCell {
    /// Build the degraded cell from the wake-time failure reason. Implementation
    /// detail of [`crate::store::StoreCellFactory`]'s `WakeFn`.
    #[doc(hidden)]
    pub fn new(reason: String) -> Self {
        Self { reason }
    }
}

/// Best-effort `tool_call` id of the inbound message, so a tool loop can
/// correlate the failure with its call. Empty string when the body carries no
/// id (or no inline body at all) — mirrors the `store` cell's own fallback.
fn tool_call_id(msg: &Message) -> String {
    let Body::Inline(v) = &msg.body else {
        return String::new();
    };
    v.get("messages")
        .and_then(|m| m.as_array())
        .and_then(|turns| turns.first())
        .and_then(|turn| turn.get("id"))
        .and_then(|id| id.as_str())
        .unwrap_or_default()
        .to_string()
}

#[allow(clippy::manual_async_fn)]
impl StatelessCell for DegradedStoreCell {
    /// Answer any message with one error turn naming the wake defect. The reply
    /// goes to `msg.reply_to`, falling back to `msg.target` — the same target
    /// rule the healthy [`crate::store::StoreCell`] uses.
    fn handle<'a>(
        &'a self,
        msg: Message,
        sink: &'a OutputSink,
    ) -> impl std::future::Future<Output = ()> + Send + 'a {
        async move {
            let reply_target = msg.reply_to.clone().unwrap_or_else(|| msg.target.clone());
            let body = json!({
                "header": {
                    "finish_reason": "error",
                    "error_code": "sql_error",
                },
                "messages": [{
                    "origin": "tool",
                    "type": "tool_result",
                    "text": format!("store cell is degraded: {}", self.reason),
                    "id": tool_call_id(&msg),
                }]
            });
            let _ = sink
                .push(CellOutput {
                    target: reply_target,
                    content: body,
                })
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::{CellEmission, Headers, MessageBuilder, Path, Uuid};
    use tokio::sync::mpsc;

    fn sink_pair() -> (OutputSink, mpsc::Receiver<CellEmission>) {
        let (tx, rx) = mpsc::channel(8);
        (
            OutputSink::new(
                tx,
                Path::new("/notes"),
                Uuid::now_v7(),
                Uuid::now_v7(),
                64,
                Headers::new(),
                None,
            ),
            rx,
        )
    }

    #[tokio::test]
    async fn answers_every_message_with_a_named_error() {
        let (sink, mut rx) = sink_pair();
        let cell = DegradedStoreCell::new("cell.db could not be opened".to_string());
        let msg = MessageBuilder::new(Path::new("/notes"))
            .body(Body::Inline(json!({"messages": [{
                "origin": "assistant", "type": "tool_call", "text": "{}", "id": "call_7"
            }]})))
            .reply_to(Path::new("/sink"))
            .build();
        cell.handle(msg, &sink).await;
        let em = rx.recv().await.expect("the degraded cell answers");
        assert_eq!(em.target, Path::new("/sink"), "the reply goes to reply_to");
        assert_eq!(em.content["header"]["finish_reason"], "error");
        assert_eq!(em.content["header"]["error_code"], "sql_error");
        assert_eq!(em.content["messages"][0]["id"], "call_7");
        assert!(
            em.content["messages"][0]["text"]
                .as_str()
                .unwrap()
                .contains("cell.db"),
            "the text names the wake defect"
        );
    }

    /// No `reply_to` → the reply goes to the message's own target (the healthy
    /// store cell's fallback), and a body without a tool_call id still answers.
    #[tokio::test]
    async fn falls_back_to_target_and_tolerates_a_missing_call_id() {
        let (sink, mut rx) = sink_pair();
        let cell = DegradedStoreCell::new("boom".to_string());
        let msg = MessageBuilder::new(Path::new("/notes"))
            .body(Body::Inline(json!({"messages": []})))
            .build();
        cell.handle(msg, &sink).await;
        let em = rx.recv().await.expect("the degraded cell answers");
        assert_eq!(em.target, Path::new("/notes"));
        assert_eq!(em.content["messages"][0]["id"], "");
    }
}
