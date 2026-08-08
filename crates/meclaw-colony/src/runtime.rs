//! Live-capable colony primitives for the apply phase (without a test wrapper).
//!
//! `ColonyRuntime` is a minimal plain struct holding the two senders that
//! `apply_bootstrap_plan` needs: the inbox sender (for ColonyMsg) and the outputs
//! sender (for CellEmission). Test wrappers such as `ColonyHandle` in
//! `meclaw-testing` expose them via a `runtime()` method.

use crate::{ColonyConfig, ColonyMsg};
use meclaw_core::CellEmission;
use tokio::sync::mpsc;

/// The minimal primitives of a running colony task. No Drop, no JoinHandle —
/// pure sender clones for the apply path.
#[derive(Clone)]
pub struct ColonyRuntime {
    /// Sender into the colony inbox (for `ColonyMsg::*` sends).
    pub inbox_tx: mpsc::Sender<ColonyMsg>,
    /// Sender into the outputs channel (for `CellEmission` forwards).
    pub outputs_tx: mpsc::Sender<CellEmission>,
    /// Colony-wide behaviour defaults from `colony.json` (phase-13.5 A7). The
    /// bootstrap-apply path reads `idle_timeout_default_ms` from here rather than
    /// from the
    /// `DEFAULT_IDLE_TIMEOUT_MS`-Konstante.
    pub colony_config: ColonyConfig,
    /// Per-colony blob store (Phase-13.5 A8). Passed to `spawn_cell` at the
    /// bootstrap-apply path so spawned cells can resolve `Body::Blob` at the
    /// delivery boundary. `None` when no store is wired (some tests).
    pub blob_store: Option<std::sync::Arc<crate::DiskBlobStore>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn runtime_holds_inbox_and_outputs_senders() {
        let (inbox_tx, _inbox_rx) = mpsc::channel(8);
        let (outputs_tx, _outputs_rx) = mpsc::channel(8);
        let rt = ColonyRuntime {
            inbox_tx,
            outputs_tx,
            colony_config: crate::ColonyConfig::default(),
            blob_store: None,
        };
        assert!(!rt.inbox_tx.is_closed());
        assert!(!rt.outputs_tx.is_closed());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn runtime_clone_does_not_close_senders() {
        let (inbox_tx, _inbox_rx) = mpsc::channel(8);
        let (outputs_tx, _outputs_rx) = mpsc::channel(8);
        let rt = ColonyRuntime {
            inbox_tx,
            outputs_tx,
            colony_config: crate::ColonyConfig::default(),
            blob_store: None,
        };
        let rt2 = rt.clone();
        assert!(!rt2.inbox_tx.is_closed());
    }
}
