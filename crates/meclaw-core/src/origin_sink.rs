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
#[derive(Clone)]
pub struct OriginSink {
    tx: mpsc::Sender<CellEmission>,
    sender_path: Path,
    default_ttl: u32,
}

impl OriginSink {
    /// Create a new origin sink. `default_ttl` is the start TTL of every
    /// emission (subject to per-hop decrement in `route()`).
    pub fn new(tx: mpsc::Sender<CellEmission>, sender_path: Path, default_ttl: u32) -> Self {
        Self {
            tx,
            sender_path,
            default_ttl,
        }
    }

    /// Emit a `CellOutput` as a source emission (parent_message_id=None,
    /// fresh trace_id). Backpressure via `mpsc::Sender::send`.
    pub async fn emit(&self, out: CellOutput) -> Result<(), mpsc::error::SendError<CellEmission>> {
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
        self.tx.send(emission).await
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
