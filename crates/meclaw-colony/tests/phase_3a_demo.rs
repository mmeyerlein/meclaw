//! Phase-3a demo: envelope mechanics — propagation + cascade + TTL.
//!
//! Topology class (AGENTS.md rule 11): multi-thread runtime, event and
//! invariant assertions only, no time window as a verdict. A delivery is
//! awaited as the event itself (the receiver's tap) under the hang mark; the
//! absence of anything further is read after the colony's quiescence barrier
//! (`ColonyHandle::quiesce`), which every message here follows from — the
//! cascade's error reply is routed inside the routing loop, so it is covered.

use meclaw_colony::DeadLetterReason;
use meclaw_core::{MessageBuilder, Path};
use meclaw_testing::MessageBuilder as TestMessageBuilder;
use meclaw_testing::deadline::hang;
use meclaw_testing::topologies::phase_3a::{Phase3aTopology, build_phase_3a_topology};

/// The next message at `/a/receiver`, awaited as an event.
async fn next_at_receiver(topo: &mut Phase3aTopology, what: &str) -> meclaw_core::Message {
    hang(what, topo.receiver_rx.recv())
        .await
        .expect("the /a/receiver tap stays open while the colony runs")
}

/// After quiescence: nothing more reached `/a/receiver`.
async fn nothing_more_at_receiver(topo: &mut Phase3aTopology, why: &str) {
    topo.colony.quiesce().await;
    let extra = topo.receiver_rx.try_recv().ok();
    assert!(extra.is_none(), "{why}; also got {extra:#?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_3a_parent_and_trace_id_propagate_through_forwarder() {
    let mut topo = build_phase_3a_topology().await;

    // Source message to /a/forwarder. trace_id defaults to id (Builder convention).
    let src = MessageBuilder::new(Path::new("/a/forwarder")).build();
    let src_id = src.id;
    let src_trace = src.trace_id;
    topo.colony.send_from(Path::new("/"), src).await;

    let msg = next_at_receiver(&mut topo, "the forwarded message reaches /a/receiver").await;
    nothing_more_at_receiver(&mut topo, "exactly one message must reach /a/receiver").await;

    assert_eq!(msg.target.as_str(), "/a/receiver");
    assert_eq!(
        msg.trace_id, src_trace,
        "trace_id propagates across forwarder"
    );
    assert_eq!(
        msg.parent_message_id,
        Some(src_id),
        "parent = the message the forwarder consumed"
    );
    assert_eq!(
        msg.reply_to.as_ref().map(|p| p.as_str()),
        Some("/a/forwarder"),
        "Colony sets reply_to = sender_path of the emitting cell"
    );
    assert!(msg.ttl < 64, "ttl decremented by route() before lookup");

    topo.colony.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_3a_reply_to_cascade_delivers_error_reply() {
    let mut topo = build_phase_3a_topology().await;

    // Route a message to /missing with reply_to = /a/receiver.
    let msg = TestMessageBuilder::new("/missing")
        .with_reply_to("/a/receiver")
        .build();
    let original_id = msg.id;
    topo.colony.send_from(Path::new("/"), msg).await;

    let reply = next_at_receiver(&mut topo, "the error-reply reaches /a/receiver").await;
    nothing_more_at_receiver(
        &mut topo,
        "/a/receiver must receive exactly the error-reply",
    )
    .await;

    assert_eq!(reply.target.as_str(), "/a/receiver");
    assert_eq!(reply.reply_to, None, "error-reply is terminal");
    assert_eq!(reply.parent_message_id, Some(original_id));

    // DLQ must be empty — reply was delivered (read after quiescence).
    let dls = topo.colony.drain_dead_letters().await;
    assert!(
        dls.is_empty(),
        "successful reply must not also DLQ: {dls:#?}"
    );

    topo.colony.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_3a_ttl_zero_lands_in_dead_letters_as_ttl_expired() {
    let mut topo = build_phase_3a_topology().await;

    let msg = TestMessageBuilder::new("/a/receiver").with_ttl(0).build();
    topo.colony.send_from(Path::new("/"), msg).await;

    // /a/receiver must NOT have been touched; read after quiescence, so the
    // routing of the ttl=0 message has run before the DLQ is drained too.
    nothing_more_at_receiver(&mut topo, "ttl_expired must short-circuit before delivery").await;

    let dls = topo.colony.drain_dead_letters().await;
    assert_eq!(dls.len(), 1, "exactly the ttl=0 message: {dls:#?}");
    assert_eq!(dls[0].reason, DeadLetterReason::TtlExpired);
    assert_eq!(dls[0].reason.as_code(), "ttl_expired");

    topo.colony.shutdown().await;
}
