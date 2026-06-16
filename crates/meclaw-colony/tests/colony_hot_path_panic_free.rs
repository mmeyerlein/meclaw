//! Deep-Audit F3 — Colony-Hot-Path panic-free invariant lock (CLAUDE.md A1′).
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

use meclaw_colony::DeadLetterReason;
use meclaw_core::{MessageBuilder, Path};
use meclaw_testing::MessageBuilder as TestMessageBuilder;
use meclaw_testing::topologies::phase_3a::build_phase_3a_topology;
use std::time::Duration;

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

    tokio::time::sleep(Duration::from_millis(100)).await;

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
    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut received = Vec::new();
    while let Ok(m) = topo.receiver_rx.try_recv() {
        received.push(m);
    }
    assert_eq!(
        received.len(),
        1,
        "colony must still route a valid message after pathological input — proof \
         the hot path never panicked"
    );
    assert_eq!(received[0].target.as_str(), "/a/receiver");

    topo.colony.shutdown().await;
}
