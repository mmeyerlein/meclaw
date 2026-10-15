//! GH #160 — a cell learns its own doorway from the authority, not from a file.
//!
//! `docs/meclaw-overview.md` § Database isolation has no exceptions left: a cell
//! touches only its own `cell.db`, and `colony.db` is out of bounds even for
//! reading. The one site that used to be exempt — the `vault` unlock attestation
//! — now asks the colony. This file proves the mechanism it asks through, against
//! a **live colony** and with a **non-vault** cell, which is what makes it a
//! substrate capability rather than a vault special case.
//!
//! What is asserted, and why each one is its own case: the answer contains every
//! inbound edge (else a tampered edge could hide), nothing outbound (else a vault
//! could not reply to a legitimate grant), nothing from another cell's
//! neighbourhood (else "cells know no topology" is gone), and it tracks the graph
//! live (else a spawn-time snapshot would attest to a topology that no longer
//! exists — at boot, cells are spawned before edges are applied, so such a
//! snapshot would be empty for every colony that has ever booted).

use meclaw_colony::{ColonyMsg, NeighbourhoodView};
use meclaw_core::Path;
use meclaw_testing::ColonyHandle;
use meclaw_testing::mocks::EchoMockCell;

/// A colony wired `a -> b`, `c -> b`, `b -> d`, and an unrelated `x -> y`.
async fn wired_colony() -> (tempfile::TempDir, ColonyHandle) {
    let td = tempfile::TempDir::new().unwrap();
    let h = ColonyHandle::new_with_factories_at(&td, vec![]);
    for p in ["/a", "/b", "/c", "/d", "/x", "/y"] {
        let path = Path::new(p);
        h.spawn(path.clone(), move || EchoMockCell::new(Path::new(p)))
            .await;
    }
    for (from, to) in [("/a", "/b"), ("/c", "/b"), ("/b", "/d"), ("/x", "/y")] {
        h.add_edge(meclaw_core::Uuid::now_v7(), Path::new(from), Path::new(to))
            .await;
    }
    (td, h)
}

fn view(h: &ColonyHandle, of: &str) -> NeighbourhoodView {
    NeighbourhoodView::new(Path::new(of), h.runtime().inbox_tx)
}

const BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cell_sees_every_inbound_edge_and_only_those() {
    let (_td, h) = wired_colony().await;

    let inbound = view(&h, "/b")
        .inbound(BUDGET)
        .await
        .expect("the colony answers");
    assert_eq!(
        inbound,
        vec![Path::new("/a"), Path::new("/c")],
        "both inbound edges, sorted; and NOT /d, which is where /b sends"
    );

    let unrelated = view(&h, "/y").inbound(BUDGET).await.expect("answered");
    assert_eq!(
        unrelated,
        vec![Path::new("/x")],
        "a view answers about its own path and no other"
    );

    let nobody = view(&h, "/a").inbound(BUDGET).await.expect("answered");
    assert!(
        nobody.is_empty(),
        "a cell nothing points at gets an empty list, which is an answer and not an error"
    );
    h.shutdown().await;
}

/// The answer follows the graph. This is the case a spawn-time snapshot fails:
/// the edge that matters is added after the cell exists — which at boot is the
/// normal order, and in an attack is the whole point.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_edge_added_after_the_cell_shows_up_in_its_neighbourhood() {
    let (_td, h) = wired_colony().await;
    let v = view(&h, "/d");

    assert_eq!(
        v.inbound(BUDGET).await.expect("answered"),
        vec![Path::new("/b")],
        "the wiring it was built with"
    );

    h.add_edge(
        meclaw_core::Uuid::now_v7(),
        Path::new("/x"),
        Path::new("/d"),
    )
    .await;

    assert_eq!(
        v.inbound(BUDGET).await.expect("answered"),
        vec![Path::new("/b"), Path::new("/x")],
        "and the one that was wired in afterwards — a snapshot would have missed it"
    );
    h.shutdown().await;
}

/// The message the capability rides on is itself narrow: it names one path and
/// answers with paths. Asserted directly, because this is the type-level reason a
/// cell cannot widen the question it is allowed to ask.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_question_carries_one_path_and_nothing_else() {
    let (_td, h) = wired_colony().await;
    let (ack, rx) = tokio::sync::oneshot::channel();
    h.runtime()
        .inbox_tx
        .send(ColonyMsg::ReadInboundEdges {
            of: Path::new("/b"),
            ack,
        })
        .await
        .expect("the colony inbox is open");
    let answer = tokio::time::timeout(BUDGET, rx)
        .await
        .expect("the colony must answer a neighbourhood read")
        .expect("ack channel");
    assert_eq!(answer, vec![Path::new("/a"), Path::new("/c")]);
    h.shutdown().await;
}
