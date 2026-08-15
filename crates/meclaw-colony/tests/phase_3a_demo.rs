//! Phase-3a demo: envelope mechanics — propagation + cascade + TTL.

use meclaw_colony::DeadLetterReason;
use meclaw_core::{MessageBuilder, Path};
use meclaw_testing::MessageBuilder as TestMessageBuilder;
use meclaw_testing::topologies::phase_3a::build_phase_3a_topology;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_3a_parent_and_trace_id_propagate_through_forwarder() {
    let mut topo = build_phase_3a_topology().await;

    // Source message to /a/forwarder. trace_id defaults to id (Builder convention).
    let src = MessageBuilder::new(Path::new("/a/forwarder")).build();
    let src_id = src.id;
    let src_trace = src.trace_id;
    topo.colony.send_from(Path::new("/"), src).await;

    // Wait for receiver to log.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut received = Vec::new();
    while let Ok(m) = topo.receiver_rx.try_recv() {
        received.push(m);
    }
    assert_eq!(
        received.len(),
        1,
        "exactly one message must reach /a/receiver"
    );
    let msg = &received[0];
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
    drop(received);

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

    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut received = Vec::new();
    while let Ok(m) = topo.receiver_rx.try_recv() {
        received.push(m);
    }
    assert_eq!(
        received.len(),
        1,
        "/a/receiver must receive exactly the error-reply"
    );
    let reply = &received[0];
    assert_eq!(reply.target.as_str(), "/a/receiver");
    assert_eq!(reply.reply_to, None, "error-reply is terminal");
    assert_eq!(reply.parent_message_id, Some(original_id));
    drop(received);

    // DLQ must be empty — reply was delivered.
    let dls = topo.colony.drain_dead_letters().await;
    assert!(dls.is_empty(), "successful reply must not also DLQ");

    topo.colony.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_3a_ttl_zero_lands_in_dead_letters_as_ttl_expired() {
    let mut topo = build_phase_3a_topology().await;

    let msg = TestMessageBuilder::new("/a/receiver").with_ttl(0).build();
    topo.colony.send_from(Path::new("/"), msg).await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    let dls = topo.colony.drain_dead_letters().await;
    assert_eq!(dls.len(), 1);
    assert_eq!(dls[0].reason, DeadLetterReason::TtlExpired);
    assert_eq!(dls[0].reason.as_code(), "ttl_expired");

    // /a/receiver must NOT have been touched.
    assert!(
        topo.receiver_rx.try_recv().is_err(),
        "ttl_expired must short-circuit before delivery"
    );

    topo.colony.shutdown().await;
}
