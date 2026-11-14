//! Deep-Audit F3 — Colony-Hot-Path panic-free invariant lock (CONTRIBUTING.md A1′).
//!
//! `route()` and the routing/dispatch path must NEVER panic on pathological
//! routing input — a panic there tears the whole colony task (every cell), a
//! heavier class than a `one_for_one` cell panic. Pathological input is handled
//! via Err/Skip/DLQ (`TtlExpired`, no-route), not a panic.
//!
//! This is a CHARACTERIZATION / LOCK test: `route()` is already panic-free today
//! (no `panic!`/`unwrap`/`expect` in its body; CEL errors return Err and skip the
//! edge — see `cel_eval.rs` unit tests). It should pass immediately and guards the
//! invariant against future regression. If it goes RED, that is a REAL FINDING
//! (the hot path acquired a panic) → stop and escalate.
//!
//! The proof is a POSITIVE receipt: after a burst of pathological messages, a
//! valid message still routes end-to-end to the live capture cell ⟺ the colony
//! task never died.
//!
//! Test class: topology/interleaving (`docs/development-rules.md` § 11) — four
//! workers, invariant and event assertions only, no time window. Until GH #1514
//! the test synchronised by two 100 ms sleeps followed by a `try_recv` drain;
//! under CI load the forwarder's echo had not reached the receiver within the
//! window (`left: 0, right: 1`). Now every wait is an event:
//! - the dead letters are drained after `ColonyHandle::quiesce()`, the barrier
//!   that returns once no inbox, mailbox or output of the colony holds work, so
//!   the `TtlExpired` letter of (1) is in the queue by construction;
//! - the receipt is awaited with `recv()` under the hang mark
//!   `meclaw_testing::deadline::HANG` (never a verdict on speed — it only names
//!   a receipt that never came);
//! - "exactly one receipt" is proven by quiescence: after `quiesce()` nothing is
//!   left in flight that could still reach `/a/receiver`, so an empty
//!   `try_recv` is the absence of a second receipt, not an elapsed window.

use meclaw_colony::DeadLetterReason;
use meclaw_core::{MessageBuilder, Path};
use meclaw_testing::MessageBuilder as TestMessageBuilder;
use meclaw_testing::deadline;
use meclaw_testing::topologies::phase_3a::build_phase_3a_topology;
use tokio::sync::mpsc::error::TryRecvError;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn colony_survives_pathological_routing_input() {
    let mut topo = build_phase_3a_topology().await;

    // (1) TTL boundary: ttl == 0 → TtlExpired DLQ, short-circuits before delivery.
    topo.colony
        .send_from(
            Path::new("/"),
            TestMessageBuilder::new("/a/receiver").with_ttl(0).build(),
        )
        .await;

    // (2) Unresolvable target, no reply_to → registry-lookup miss, no-route path.
    topo.colony
        .send_from(
            Path::new("/"),
            TestMessageBuilder::new("/does/not/exist").build(),
        )
        .await;

    // (3) Structurally pathological (valid syntax, deeply nested, unregistered) →
    //     lookup miss, no panic.
    topo.colony
        .send_from(
            Path::new("/"),
            TestMessageBuilder::new("/a/b/c/d/e/f/g/h/i/j/k").build(),
        )
        .await;

    // Event, not a window: the colony has routed all three and holds no work.
    topo.colony.quiesce().await;

    // The pathological input was absorbed (handled into the DLQ / no-route), not
    // panicked through. TtlExpired must be present from (1).
    let dls = topo.colony.drain_dead_letters().await;
    assert!(
        dls.iter().any(|d| d.reason == DeadLetterReason::TtlExpired),
        "ttl==0 must land as TtlExpired (handled, not panicked); got {:?}",
        dls.iter().map(|d| d.reason.as_code()).collect::<Vec<_>>()
    );

    // POSITIVE RECEIPT — the decisive panic-free proof: the colony is STILL ALIVE
    // and routes a valid message end-to-end through the forwarder to the receiver.
    // If any pathological input above had panicked the colony task, this delivery
    // could not happen.
    topo.colony
        .send_from(
            Path::new("/"),
            MessageBuilder::new(Path::new("/a/forwarder")).build(),
        )
        .await;
    let first = deadline::hang(
        "the valid message reaches /a/receiver after the pathological input — proof \
         the hot path never panicked",
        topo.receiver_rx.recv(),
    )
    .await
    .expect("receiver channel closed: the colony task is gone");
    assert_eq!(first.target.as_str(), "/a/receiver");

    // Exactly one receipt: once the colony is quiescent nothing can still reach
    // the receiver, so the channel must be empty now (absence by quiescence).
    topo.colony.quiesce().await;
    match topo.receiver_rx.try_recv() {
        Err(TryRecvError::Empty) => {}
        other => panic!(
            "exactly one message must reach /a/receiver (the forwarder's echo); \
             after quiescence the receiver still had {other:?}"
        ),
    }

    topo.colony.shutdown().await;
}
