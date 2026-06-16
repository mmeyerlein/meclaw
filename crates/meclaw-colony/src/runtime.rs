//! Live-fähige Colony-Primitives für apply-Phase (ohne Test-Wrapper).
//!
//! `ColonyRuntime` ist ein minimaler Plain-Struct, der die zwei Sender hält,
//! die `apply_bootstrap_plan` braucht: Inbox-Sender (für ColonyMsg) und
//! Outputs-Sender (für CellEmission). Test-Wrapper wie `ColonyHandle` in
//! `meclaw-testing` exposieren diese via `runtime()`-Methode.

use crate::{ColonyConfig, ColonyMsg};
use meclaw_core::CellEmission;
use tokio::sync::mpsc;

/// Minimal-Primitives einer laufenden Colony-Task. Ohne Drop, ohne JoinHandle —
/// reine Sender-Klone für den apply-Pfad.
#[derive(Clone)]
pub struct ColonyRuntime {
    /// Sender in die Colony-Inbox (für `ColonyMsg::*`-Sends).
    pub inbox_tx: mpsc::Sender<ColonyMsg>,
    /// Sender in den Outputs-Channel (für `CellEmission`-Forwards).
    pub outputs_tx: mpsc::Sender<CellEmission>,
    /// Colony-weite Verhaltens-Defaults aus `colony.json` (Phase-13.5 A7). Der
    /// Bootstrap-apply-Pfad liest `idle_timeout_default_ms` hieraus statt aus der
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
