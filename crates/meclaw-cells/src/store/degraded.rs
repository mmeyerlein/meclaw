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
/// message is answered with `finish_reason: "error"`, `error_code: "sql_error"`,
/// an `operation` naming the refused op (see [`refused_operation`]) and a text
/// naming the wake defect.
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

/// GH #370 — the op this reply refuses, read off the inbound body alone.
///
/// Fourteen shipped `store` templates declare `hop.operation` `required: true`,
/// so a degraded reply without the field is discarded by the colony's central
/// emits check and replaced by a `contract_violation` answer — the database
/// defect this cell exists to report would never reach the caller.
///
/// The value follows the healthy store's **stated** rule (`cell-types.md`
/// § store, GH #331): the operation the caller asked for, or the literal `error`
/// when nothing parseable arrived. Two or more `tool_call` turns are a bundle
/// (GH #295) and are named `bundle` as a whole — naming the first leg would
/// claim the others had been considered separately.
///
/// It is **not** byte-for-byte the healthy cell's behaviour, and the difference
/// is worth naming because it is easy to assume otherwise. Two inbound shapes
/// diverge, both because the healthy cell's `error` there comes from a PARSE
/// REFUSAL and this cell never dispatches anything to refuse:
///
/// - `[text, tool_call]` — one call, but not in `messages[0]`. The healthy
///   single-op path reads `messages[0]`, finds a non-`tool_call` turn and
///   refuses with `invalid_input` + `error`; this function filters for the first
///   `tool_call` anywhere and names its op. (`llm`'s `translate.rs` really
///   produces such bodies, which is why `parse_tool_calls` skips prose rather
///   than refusing it — GH #295.)
/// - a bundle with an unparsable leg — the healthy `run_bundle` refuses the
///   whole message with `invalid_input` + `error`; this function has already
///   counted two calls and says `bundle`.
///
/// Both divergences are contract-conform (a non-empty string is all the
/// declaration asks for) and both name the inbound message MORE precisely than
/// `error` would. Neither is a licence to let the two rules drift further: if
/// the healthy cell's parse surface changes, re-read this one.
fn refused_operation(msg: &Message) -> String {
    const UNKNOWN: &str = "error";
    let Body::Inline(v) = &msg.body else {
        return UNKNOWN.to_string();
    };
    let Some(turns) = v.get("messages").and_then(|m| m.as_array()) else {
        return UNKNOWN.to_string();
    };
    let mut calls = turns
        .iter()
        .filter(|t| t.get("type").and_then(|t| t.as_str()) == Some("tool_call"))
        .peekable();
    let Some(first) = calls.next() else {
        return UNKNOWN.to_string();
    };
    if calls.peek().is_some() {
        return "bundle".to_string();
    }
    first
        .get("text")
        .and_then(|t| t.as_str())
        .and_then(|t| meclaw_core::serde_json::from_str::<meclaw_core::JsonValue>(t).ok())
        .as_ref()
        .and_then(|args| args.get("operation"))
        .and_then(|op| op.as_str())
        .unwrap_or(UNKNOWN)
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
                    "operation": refused_operation(&msg),
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
    use meclaw_core::{CellEmission, Headers, JsonValue, MessageBuilder, Path, Uuid};
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

    /// Build a degraded reply to a body carrying the given turns.
    async fn reply_to_turns(turns: JsonValue) -> CellEmission {
        let (sink, mut rx) = sink_pair();
        let cell = DegradedStoreCell::new("cell.db could not be opened".to_string());
        let msg = MessageBuilder::new(Path::new("/notes"))
            .body(Body::Inline(json!({ "messages": turns })))
            .build();
        cell.handle(msg, &sink).await;
        rx.recv().await.expect("the degraded cell answers")
    }

    fn call(text: &str) -> JsonValue {
        json!({"origin": "assistant", "type": "tool_call", "text": text, "id": "c"})
    }

    /// GH #370 — the refused op reaches the header. Fourteen shipped stores
    /// declare `hop.operation` `required: true`, so a degraded reply without it
    /// is discarded and replaced by `contract_violation` — the DB defect the
    /// cell exists to report would never reach the caller.
    #[tokio::test]
    async fn names_the_refused_operation() {
        let em = reply_to_turns(json!([call(r#"{"operation":"insert","table":"items"}"#)])).await;
        assert_eq!(
            em.content["header"]["operation"], "insert",
            "the degraded reply names the op it refused: {:?}",
            em.content
        );
    }

    /// Nothing parseable arrived → the literal `error`, the same fallback the
    /// healthy store's error surface uses (GH #331, `cell-types.md` § store).
    #[tokio::test]
    async fn falls_back_to_the_error_literal_when_no_op_is_readable() {
        for turns in [
            json!([]),
            json!([{"origin": "user", "type": "text", "text": "hi", "id": ""}]),
            json!([call("not json at all")]),
            json!([call(r#"{"table":"items"}"#)]),
        ] {
            let em = reply_to_turns(turns.clone()).await;
            assert_eq!(
                em.content["header"]["operation"], "error",
                "no readable op in {turns} must still stamp the field: {:?}",
                em.content
            );
        }
    }

    /// Two or more `tool_call` turns are a bundle (GH #295); the healthy store
    /// stamps `operation: "bundle"` for the whole message and so does the
    /// degraded one — a single leg name would claim the others were run.
    #[tokio::test]
    async fn a_refused_bundle_is_named_a_bundle() {
        let em = reply_to_turns(json!([
            call(r#"{"operation":"insert","table":"items"}"#),
            call(r#"{"operation":"select","table":"items"}"#),
        ]))
        .await;
        assert_eq!(
            em.content["header"]["operation"], "bundle",
            "a refused bundle is one bundle, not its first leg: {:?}",
            em.content
        );
    }

    /// The two shapes where this cell does NOT say what the healthy one says,
    /// pinned so the divergence stays deliberate (see [`refused_operation`]).
    /// Both are contract-conform and both name the message more precisely than
    /// the healthy cell's parse-refusal `error` does.
    #[tokio::test]
    async fn the_two_divergences_from_the_healthy_cell_are_deliberate() {
        // `[text, tool_call]`: the healthy single-op path reads messages[0],
        // refuses the non-tool_call turn and stamps `error`.
        let em = reply_to_turns(json!([
            {"origin": "assistant", "type": "text", "text": "here goes", "id": ""},
            call(r#"{"operation":"insert","table":"items"}"#),
        ]))
        .await;
        assert_eq!(
            em.content["header"]["operation"], "insert",
            "a call behind a prose turn is still the op that was refused: {:?}",
            em.content
        );

        // A bundle with an unparsable leg: the healthy `run_bundle` refuses the
        // whole message with `error`; two calls have already been counted here.
        let em = reply_to_turns(json!([
            call(r#"{"operation":"insert","table":"items"}"#),
            call("{ this is not json"),
        ]))
        .await;
        assert_eq!(
            em.content["header"]["operation"], "bundle",
            "a bundle stays a bundle even when a leg would not have parsed: {:?}",
            em.content
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
