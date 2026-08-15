//! Output path per spec § Output path.
//!
//! - `CellOutput { target, content }` is what a cell emits.
//! - `CellEmission` is the enriched package that `cell_task` sends to colony's
//!   outputs mailbox: cell output plus parent context.
//! - `OutputSink` is the per-message-local wrapper that `cell_task` constructs
//!   and hands to the cell by `&` reference. `push(CellOutput)` enriches it with
//!   `sender_path`/`parent_message_id`/`trace_id`/`input_ttl`/`input_headers` and sends.

use crate::headers::Headers;
use crate::path::Path;
use serde_json::Value;
use tokio::sync::mpsc;
use uuid::Uuid;

/// What a cell emits. Pure cell view: no envelope knowledge.
#[derive(Debug, Clone)]
pub struct CellOutput {
    /// Set directly by the cell in 3a; from phase 4 on typically via an edge.
    pub target: Path,
    /// JSON content with an optional `"header"` section (extracted in 3b).
    pub content: Value,
}

/// Enriched package that `cell_task` sends to colony's outputs mailbox.
/// Carries the parent context that only `cell_task` knows (local stack state).
#[derive(Debug, Clone)]
pub struct CellEmission {
    /// Absolute path of the emitting cell (set by colony as `reply_to`).
    pub sender_path: Path,
    /// ID of the consumed inbound message; becomes the follow-up message's
    /// `parent_message_id`. `None` on source messages (e.g. event-originated
    /// emissions from long-running cells using `OriginSink`). Per overview l.852.
    pub parent_message_id: Option<Uuid>,
    /// Trace ID of the consumed message; copied into the follow-up message.
    pub trace_id: Uuid,
    /// TTL of the consumed message (already decremented once by `route()` when
    /// the cell received it). It is carried over as the follow-up message's start
    /// TTL; `route()` decrements again on the next hop. Spec § TTL semantics:
    /// "colony decrements on every routing decision", one decrement operation
    /// per cell-to-cell hop, not per message construction.
    pub input_ttl: u32,
    /// Headers from the consumed input message (spec § 822: colony
    /// propagates input-headers into the output; cell-emitted
    /// content.header overlays the `hop` compartment via the outputs-arm).
    pub input_headers: Headers,
    /// `reply_to` of the consumed input message: target for substrate-side
    /// `contract_violation` error replies at the central emits check (Slice 3).
    /// `None` for source emissions (OriginSink) and inputs without reply_to.
    pub input_reply_to: Option<Path>,
    /// Cell-emitted target.
    pub target: Path,
    /// Cell-emitted content (the body is built in colony).
    pub content: Value,
    /// W2b (Ruling A1, ruling 2026-06-12): when `true`, this emission is a
    /// substrate-generated **error reply** addressed to a known sender
    /// (`target` == the input's `reply_to`) — e.g. `consumes_violation` or the
    /// `message_timeout` backstop. The outputs-arm delivers it DIRECTLY to
    /// `target` via `route_with_log` (registry lookup, the `contract_violation`
    /// pattern), NOT through the sender's out-edges. It is feedback to a known
    /// sender, not a routing emission, so the A1 no_route rule does not apply.
    /// Normal emissions leave this `false`.
    pub direct_reply: bool,
}

/// Per-message lifetime; exists only for the duration of one `cell.handle()`
/// call. `cell_task` constructs the sink with the parent context, hands it to
/// `cell.handle()` by `&`, and drops it afterwards. Pushing more than once is
/// allowed (multi-send: spec § Output path, "emit frequency per handle() call").
#[derive(Clone)]
pub struct OutputSink {
    tx: mpsc::Sender<CellEmission>,
    sender_path: Path,
    parent_message_id: Uuid,
    trace_id: Uuid,
    input_ttl: u32,
    input_headers: Headers,
    /// `reply_to` of the consumed input message; copied into every emission.
    input_reply_to: Option<Path>,
}

impl OutputSink {
    /// Create a new sink with the parent context from the consumed input message.
    pub fn new(
        tx: mpsc::Sender<CellEmission>,
        sender_path: Path,
        parent_message_id: Uuid,
        trace_id: Uuid,
        input_ttl: u32,
        input_headers: Headers,
        input_reply_to: Option<Path>,
    ) -> Self {
        Self {
            tx,
            sender_path,
            parent_message_id,
            trace_id,
            input_ttl,
            input_headers,
            input_reply_to,
        }
    }

    /// The absolute path of the cell this sink belongs to — the value the
    /// colony stamps as `reply_to` on every emission (GH #132).
    ///
    /// This is NOT topology knowledge: a cell learns its own address, not its
    /// neighbours'. It is the same string the substrate already puts on the
    /// wire for this cell, exposed so a cell can reason about **where it itself
    /// sits** (the `store`'s internal write surface derives its owning scope
    /// from it). Set by `cell_task` from the registry path; a cell cannot forge
    /// it.
    pub fn sender_path(&self) -> &Path {
        &self.sender_path
    }

    /// Push a `CellOutput` to colony's outputs mailbox, enriched with the parent
    /// context. Backpressure applies via `mpsc::Sender::send`.
    pub async fn push(&self, out: CellOutput) -> Result<(), mpsc::error::SendError<CellEmission>> {
        self.tx.send(self.enrich(out, false)).await
    }

    /// Push a substrate-generated **error reply** addressed to a known sender
    /// (`out.target` == the input's `reply_to`). Sets `direct_reply` so the
    /// outputs-arm delivers it DIRECTLY via `route_with_log` rather than through
    /// the sender's out-edges (W2b Ruling A1, ruling 2026-06-12). Used by the
    /// `consumes_violation` ingress check and the `message_timeout` backstop.
    pub async fn push_direct_reply(
        &self,
        out: CellOutput,
    ) -> Result<(), mpsc::error::SendError<CellEmission>> {
        self.tx.send(self.enrich(out, true)).await
    }

    fn enrich(&self, out: CellOutput, direct_reply: bool) -> CellEmission {
        CellEmission {
            sender_path: self.sender_path.clone(),
            parent_message_id: Some(self.parent_message_id),
            trace_id: self.trace_id,
            input_ttl: self.input_ttl,
            input_headers: self.input_headers.clone(),
            input_reply_to: self.input_reply_to.clone(),
            target: out.target,
            content: out.content,
            direct_reply,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn sink_push_enriches_with_parent_context() {
        let (tx, mut rx) = mpsc::channel(4);
        let parent = Uuid::now_v7();
        let trace = Uuid::now_v7();
        let sink = OutputSink::new(
            tx,
            Path::new("/me"),
            parent,
            trace,
            17,
            Headers::new(),
            None,
        );
        sink.push(CellOutput {
            target: Path::new("/dst"),
            content: json!({"x": 1}),
        })
        .await
        .unwrap();
        let em = rx.recv().await.unwrap();
        assert_eq!(em.sender_path.as_str(), "/me");
        assert_eq!(em.parent_message_id, Some(parent));
        assert_eq!(em.trace_id, trace);
        assert_eq!(em.input_ttl, 17);
        assert_eq!(em.target.as_str(), "/dst");
        assert_eq!(em.content, json!({"x": 1}));
    }

    #[tokio::test]
    async fn sink_propagates_input_headers_to_emission() {
        use serde_json::json;
        let (tx, mut rx) = mpsc::channel(4);
        let mut ctx = serde_json::Map::new();
        ctx.insert("session_id".into(), json!("s1"));
        let input_headers = Headers::from_parts(ctx, serde_json::Map::new());
        let sink = OutputSink::new(
            tx,
            Path::new("/me"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            10,
            input_headers.clone(),
            None,
        );
        sink.push(CellOutput {
            target: Path::new("/dst"),
            content: Value::Null,
        })
        .await
        .unwrap();
        let em = rx.recv().await.unwrap();
        assert_eq!(
            em.input_headers.context.get("session_id"),
            Some(&json!("s1"))
        );
    }

    #[tokio::test]
    async fn source_emission_carries_none_parent_message_id() {
        let (tx, mut rx) = mpsc::channel(4);
        let emission = CellEmission {
            sender_path: Path::new("/lr"),
            parent_message_id: None,
            trace_id: Uuid::now_v7(),
            input_ttl: 64,
            input_headers: Headers::new(),
            input_reply_to: None,
            target: Path::new("/dst"),
            content: Value::Null,
            direct_reply: false,
        };
        tx.send(emission).await.unwrap();
        let em = rx.recv().await.unwrap();
        assert_eq!(em.parent_message_id, None);
    }

    #[tokio::test]
    async fn sink_push_carries_input_reply_to_into_emission() {
        let (tx, mut rx) = mpsc::channel(4);
        let sink = OutputSink::new(
            tx,
            Path::new("/me"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            9,
            Headers::new(),
            Some(Path::new("/a")),
        );
        sink.push(CellOutput {
            target: Path::new("/dst"),
            content: Value::Null,
        })
        .await
        .unwrap();
        let em = rx.recv().await.unwrap();
        assert_eq!(
            em.input_reply_to,
            Some(Path::new("/a")),
            "sink copies the consumed input's reply_to into the emission"
        );
    }

    #[tokio::test]
    async fn sink_push_is_multi_send_capable() {
        let (tx, mut rx) = mpsc::channel(4);
        let sink = OutputSink::new(
            tx,
            Path::new("/me"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            5,
            Headers::new(),
            None,
        );
        for i in 0..3 {
            sink.push(CellOutput {
                target: Path::new("/dst"),
                content: json!({"i": i}),
            })
            .await
            .unwrap();
        }
        for i in 0..3 {
            let em = rx.recv().await.unwrap();
            assert_eq!(em.content, json!({"i": i}));
        }
    }
}
