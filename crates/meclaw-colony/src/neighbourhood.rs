//! GH #160: the read handle a cell receives when it declares that it needs to
//! know its own doorway.
//!
//! # Why this exists
//!
//! `docs/meclaw-overview.md` § Database isolation is absolute: a cell touches
//! only its own `cell.db`, and `colony.db` is out of bounds **even for reading**.
//! One place in the workspace used to be exempt — the `vault` unlock
//! attestation, which opened `colony.db` read-only to check that every edge
//! pointing at it comes from a path its sealed contract expects. The rationale
//! was sound and still is: the birth topology is exempt from the port boundary,
//! so an agent with filesystem access can rewrite the grow file and the *next*
//! boot wires in an edge no mutation would have been allowed to add. Making the
//! unlock the checkpoint is the right answer. Reading the database was not.
//!
//! The vault's defence is the sealed contract in its **own** `cell.db`, not the
//! trustworthiness of the topology it compares against. Whether the tampered
//! edge is learned by reading `colony.db` or by being told, the comparison finds
//! the same unexpected path and the vault stays LOCKED. So the exception was
//! never load-bearing, only convenient — and an exception on the books is a
//! precedent, which is the expensive part.
//!
//! # Shape of the wiring, and why this one
//!
//! * **No extra `CellFactory` parameter.** Like the `AttachmentReader` of GH #87,
//!   this is a function of things the factory already has: the contract
//!   ([`ContractView`]), the cell's own path, and the colony inbox the factory is
//!   handed anyway. Declaration-gated: no declaration, no handle, and a cell
//!   without a handle cannot ask.
//! * **One question, and it is about the asker.** The handle can ask for the
//!   `from` of every edge whose `to` is *its own path*. Not the graph, not a
//!   scope, not its own outbound edges, and never another cell's. "Cells know no
//!   topology" survives in the form that matters: a cell learns the shape of its
//!   own doorway, from the authority, and only ever in order to refuse.
//! * **Live, not a snapshot.** The answer is fetched when it is needed, not
//!   stamped at spawn — at boot the cells are spawned *before* the edges are
//!   applied, so a spawn-time snapshot of a vault's neighbourhood would be
//!   reliably empty, i.e. reliably wrong in the permissive direction.
//! * **Every ask carries an operation timeout** (`CLAUDE.md` hard rule 12): a
//!   colony that does not answer costs the caller its own deadline and is then
//!   treated as unverifiable — which is to say, as tampered.

use crate::{ColonyMsg, ContractView};
use meclaw_core::Path;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

/// The `consumes.topology` key whose declaration hands a cell the handle.
pub const INBOUND_EDGES_FACT: &str = "inbound_edges";

/// Why a neighbourhood could not be established.
///
/// Both variants mean the same thing to a caller that is deciding whether to
/// trust its surroundings: **it does not know**. Failing closed on that is the
/// caller's job and the vault does exactly that (`Attestation::Unverifiable`).
#[derive(Debug, Error, PartialEq, Eq)]
pub enum NeighbourhoodError {
    /// The colony did not answer within the operation timeout.
    #[error("the colony did not answer within {0} ms")]
    Timeout(u128),
    /// The colony is gone, or dropped the reply channel (shutdown drain).
    #[error("the colony is not answering (inbox closed or reply dropped)")]
    Unreachable,
}

/// Read-only handle on **one cell's own inbound edges**, held by a cell that
/// declared `consumes.topology.inbound_edges`.
///
/// Cheap to clone (one `Path`, one `mpsc::Sender`), which is what a factory's
/// wake/respawn closures need.
#[derive(Clone)]
pub struct NeighbourhoodView {
    of: Path,
    colony: mpsc::Sender<ColonyMsg>,
}

impl std::fmt::Debug for NeighbourhoodView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NeighbourhoodView({})", self.of.as_str())
    }
}

impl NeighbourhoodView {
    /// Hand out a view iff the cell declared `consumes.topology.inbound_edges`.
    ///
    /// `None` otherwise, and that is the whole gate: a cell that does not declare
    /// the fact has no way to obtain it.
    pub fn for_contract(
        contract: &ContractView,
        of: Path,
        colony: mpsc::Sender<ColonyMsg>,
    ) -> Option<Self> {
        let declared = contract
            .consumes
            .as_ref()
            .is_some_and(|c| c.declares_topology(INBOUND_EDGES_FACT));
        declared.then_some(Self { of, colony })
    }

    /// Build a view directly, for tests and for callers that already decided.
    ///
    /// Public because the vault's tests need to construct one without a contract;
    /// it grants nothing the declared path does not, since the path is fixed at
    /// construction and every answer is scoped to it.
    pub fn new(of: Path, colony: mpsc::Sender<ColonyMsg>) -> Self {
        Self { of, colony }
    }

    /// The path this view answers about.
    pub fn path(&self) -> &Path {
        &self.of
    }

    /// The `from` of every edge pointing at this cell, sorted and deduplicated.
    ///
    /// An empty list is a legitimate answer — a cell nothing points at — and is
    /// NOT an error: for the vault it is the strongest possible attestation.
    pub async fn inbound(
        &self,
        timeout: std::time::Duration,
    ) -> Result<Vec<Path>, NeighbourhoodError> {
        let (ack, rx) = oneshot::channel();
        if self
            .colony
            .send(ColonyMsg::ReadInboundEdges {
                of: self.of.clone(),
                ack,
            })
            .await
            .is_err()
        {
            return Err(NeighbourhoodError::Unreachable);
        }
        match tokio::time::timeout(timeout, rx).await {
            Err(_) => Err(NeighbourhoodError::Timeout(timeout.as_millis())),
            Ok(Err(_)) => Err(NeighbourhoodError::Unreachable),
            Ok(Ok(paths)) => Ok(paths),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::{CompiledConsumes, ConsumesBlock};
    use std::sync::Arc;

    fn contract(json: serde_json::Value) -> ContractView {
        let block: ConsumesBlock = serde_json::from_value(json).unwrap();
        ContractView {
            consumes: Some(Arc::new(CompiledConsumes::compile(&block))),
            ..ContractView::default()
        }
    }

    #[test]
    fn a_declared_fact_yields_a_handle() {
        let (tx, _rx) = mpsc::channel(1);
        let c = contract(serde_json::json!({"topology": {"inbound_edges": {"type": "array"}}}));
        assert!(NeighbourhoodView::for_contract(&c, Path::new("/v"), tx).is_some());
    }

    #[test]
    fn an_undeclared_cell_gets_nothing() {
        let (tx, _rx) = mpsc::channel(1);
        let c = contract(serde_json::json!({"body": {"messages": {"type": "array"}}}));
        assert!(NeighbourhoodView::for_contract(&c, Path::new("/v"), tx.clone()).is_none());
        assert!(
            NeighbourhoodView::for_contract(&ContractView::default(), Path::new("/v"), tx)
                .is_none()
        );
    }

    /// A declared body slot of the same NAME must not open the topology gate —
    /// the two namespaces are separate, and a cell that declares it reads an
    /// `inbound_edges` body slot has said nothing about the graph.
    #[test]
    fn a_body_slot_of_the_same_name_is_not_the_declaration() {
        let (tx, _rx) = mpsc::channel(1);
        let c = contract(serde_json::json!({"body": {"inbound_edges": {"type": "array"}}}));
        assert!(NeighbourhoodView::for_contract(&c, Path::new("/v"), tx).is_none());
    }

    #[tokio::test]
    async fn the_ask_is_scoped_to_the_holders_own_path() {
        let (tx, mut rx) = mpsc::channel(4);
        let view = NeighbourhoodView::new(Path::new("/main/vault"), tx);
        let answering = tokio::spawn(async move {
            let Some(ColonyMsg::ReadInboundEdges { of, ack }) = rx.recv().await else {
                panic!("expected a ReadInboundEdges");
            };
            assert_eq!(
                of.as_str(),
                "/main/vault",
                "a view can only ask about itself"
            );
            let _ = ack.send(vec![Path::new("/main/agent")]);
        });
        let got = view
            .inbound(std::time::Duration::from_secs(5))
            .await
            .expect("the colony answered");
        assert_eq!(got, vec![Path::new("/main/agent")]);
        answering.await.unwrap();
    }

    #[tokio::test]
    async fn a_dropped_reply_channel_is_unreachable_not_empty() {
        let (tx, mut rx) = mpsc::channel(4);
        let view = NeighbourhoodView::new(Path::new("/v"), tx);
        let dropping = tokio::spawn(async move {
            if let Some(ColonyMsg::ReadInboundEdges { ack, .. }) = rx.recv().await {
                drop(ack); // exactly what the Shutdown drain does
            }
        });
        assert_eq!(
            view.inbound(std::time::Duration::from_secs(5)).await,
            Err(NeighbourhoodError::Unreachable),
            "a drained colony must never look like a vault nobody points at"
        );
        dropping.await.unwrap();
    }

    #[tokio::test]
    async fn a_silent_colony_times_out() {
        let (tx, _rx) = mpsc::channel(4); // held open, never answered
        let view = NeighbourhoodView::new(Path::new("/v"), tx);
        let err = view
            .inbound(std::time::Duration::from_millis(50))
            .await
            .expect_err("a colony that never answers must not block the caller");
        assert!(matches!(err, NeighbourhoodError::Timeout(50)));
    }
}
