//! Origin-emission sink for source cells (proxy/timer/mcp in Phase 10).
//! Sibling primitive to `OutputSink`. Every `emit()` produces a
//! `CellEmission` with `parent_message_id: None` (source per overview
//! Z.852) and a fresh `trace_id` (every source event starts its own
//! trace).

use crate::headers::Headers;
use crate::output::{CellEmission, CellOutput};
use crate::path::Path;
use tokio::sync::mpsc;
use uuid::Uuid;

/// Per-cell-lifetime sink for source emissions. Sibling to `OutputSink`.
///
/// Differences from `OutputSink`:
/// - `parent_message_id: None` on every emission (source per overview
///   Z.852).
/// - Fresh `trace_id` per `emit()` (every source event starts its own
///   trace; no inheritance from a consumed input message because there
///   is none).
///
/// Used by long-running cells (`proxy`, `timer`, `mcp`) in the
/// `handle_event` path — see `LongRunningCell` in `meclaw-colony`.
///
/// Deliberately not `Clone` (GH #617): a cell only ever holds `&OriginSink`, and
/// without `Clone` it cannot turn that into an owned sink and call
/// `with_ingress` on it. No caller in the tree needed the clone.
pub struct OriginSink {
    tx: mpsc::Sender<CellEmission>,
    sender_path: Path,
    default_ttl: u32,
    /// GH #617 — `contract.ingress.carries_trace`; `false` unless
    /// `with_ingress` was called.
    carries_trace: bool,
}

impl OriginSink {
    /// Create a new origin sink. `default_ttl` is the start TTL of every
    /// emission (subject to per-hop decrement in `route()`).
    pub fn new(tx: mpsc::Sender<CellEmission>, sender_path: Path, default_ttl: u32) -> Self {
        Self {
            tx,
            sender_path,
            default_ttl,
            carries_trace: false,
        }
    }

    /// Emit a `CellOutput` as a source emission (parent_message_id=None,
    /// fresh trace_id). Backpressure via `mpsc::Sender::send`.
    ///
    /// The error is boxed for the reason given on `ActorHandle::send`
    /// (GH #406): `SendError<CellEmission>` carries the whole undelivered
    /// emission, so the allocation belongs in the failure branch rather than in
    /// the size of every success.
    pub async fn emit(
        &self,
        out: CellOutput,
    ) -> Result<(), Box<mpsc::error::SendError<CellEmission>>> {
        let emission = CellEmission {
            sender_path: self.sender_path.clone(),
            parent_message_id: None,
            trace_id: Uuid::now_v7(),
            input_ttl: self.default_ttl,
            // Source emissions have no consumed input → no reply_to to carry.
            input_reply_to: None,
            input_headers: Headers::new(),
            target: out.target,
            content: out.content,
            // Source emissions are never substrate error replies.
            direct_reply: false,
        };
        self.tx.send(emission).await.map_err(Box::new)
    }

    /// Grant the ingress handle, for a cell that declares
    /// `contract.ingress.carries_trace`. A builder step rather than a fourth `new`
    /// parameter: every existing call site keeps meaning what it meant.
    ///
    /// Outside tests only the colony calls this, from the declaration
    /// `contract.ingress.carries_trace`, when it builds the sink of a long-running
    /// cell. It is `pub` because the colony lives in another crate. The budget does
    /// not depend on it: the colony's outputs arm reads the same declaration from
    /// the node contract and re-stamps the TTL of every undeclared source emission
    /// (`colony.rs`, outputs arm).
    #[must_use]
    pub fn with_ingress(mut self) -> Self {
        self.carries_trace = true;
        self
    }

    /// The ingress handle, iff the cell declared it — `None` otherwise, and a cell
    /// without a handle cannot ask (cf. `AttachmentReader::for_contract`).
    pub fn ingress(&self) -> Option<IngressEmitter> {
        self.carries_trace.then(|| IngressEmitter {
            tx: self.tx.clone(),
            sender_path: self.sender_path.clone(),
        })
    }
}

/// Why an ingress emission was refused. Hand-written rather than `thiserror`:
/// `meclaw-core` carries neither the dependency nor a precedent for one.
#[derive(Debug)]
pub enum IngressEmitError {
    /// A carried TTL of zero: a message with no hops left must not buy one.
    NoBudget,
    /// The colony's outputs channel is closed — the cell is on its way down.
    Send(Box<mpsc::error::SendError<CellEmission>>),
}

impl std::fmt::Display for IngressEmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoBudget => write!(f, "a carried ttl of 0 has no hop left to spend"),
            Self::Send(_) => write!(f, "the colony is not accepting emissions (outputs closed)"),
        }
    }
}

impl std::error::Error for IngressEmitError {}

/// A birth point for messages the cell did not originate: trace and budget come
/// from the wire, so one conversation stays one trace across two message logs and
/// a cycle dies on the budget it set out with. Handed out ONLY by `ingress()`.
#[derive(Clone)]
pub struct IngressEmitter {
    tx: mpsc::Sender<CellEmission>,
    sender_path: Path,
}

impl IngressEmitter {
    /// Emit carrying `trace_id` and `ttl` instead of minting them.
    /// `parent_message_id` stays `None` — nothing was consumed here, and the outputs
    /// arm reads the declaration rather than the parent. `ttl == 0` is refused before
    /// the emission: the boundary should have answered `ttl_exhausted` on the wire.
    pub async fn emit(
        &self,
        out: CellOutput,
        trace_id: Uuid,
        ttl: u32,
    ) -> Result<(), IngressEmitError> {
        if ttl == 0 {
            return Err(IngressEmitError::NoBudget);
        }
        let emission = CellEmission {
            sender_path: self.sender_path.clone(),
            // Nothing was consumed here; trace and budget are carried, not minted.
            parent_message_id: None,
            trace_id,
            input_ttl: ttl,
            input_reply_to: None,
            // Headers stay edge authority: a projected context key travels under
            // `hop` and is promoted by the entry edge, never stamped here.
            input_headers: Headers::new(),
            target: out.target,
            content: out.content,
            direct_reply: false,
        };
        self.tx
            .send(emission)
            .await
            .map_err(|e| IngressEmitError::Send(Box::new(e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CellOutput, Path, Uuid, output::CellEmission};
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn emit_sets_parent_none_and_fresh_trace() {
        let (tx, mut rx) = mpsc::channel::<CellEmission>(4);
        let sink = OriginSink::new(tx, Path::new("/lr"), 42);

        sink.emit(CellOutput {
            target: Path::new("/dst"),
            content: crate::JsonValue::Bool(true),
        })
        .await
        .unwrap();

        let em = rx.recv().await.unwrap();
        assert_eq!(em.sender_path.as_str(), "/lr");
        assert_eq!(em.target.as_str(), "/dst");
        assert_eq!(em.input_ttl, 42);
        assert_eq!(
            em.parent_message_id, None,
            "source emission: parent_message_id IS None"
        );
        assert_ne!(em.trace_id, Uuid::nil());
        assert_eq!(
            em.input_reply_to, None,
            "source emission: no consumed input, no reply_to"
        );
        assert!(em.input_headers.context.is_empty());
        assert!(em.input_headers.hop.is_empty());
    }

    #[tokio::test]
    async fn two_emits_get_distinct_trace_ids() {
        let (tx, mut rx) = mpsc::channel::<CellEmission>(4);
        let sink = OriginSink::new(tx, Path::new("/lr"), 16);
        for _ in 0..2 {
            sink.emit(CellOutput {
                target: Path::new("/dst"),
                content: crate::JsonValue::Null,
            })
            .await
            .unwrap();
        }
        let a = rx.recv().await.unwrap();
        let b = rx.recv().await.unwrap();
        assert_ne!(
            a.trace_id, b.trace_id,
            "every source event starts a fresh trace"
        );
        assert_eq!(a.parent_message_id, None);
        assert_eq!(b.parent_message_id, None);
    }
}
