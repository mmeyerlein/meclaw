//! Phase-3b demo: UBF body validation + content.header extraction
//! + 3-hop direct-parent regression (closes the 3a gap from fixup 6-fix).

use meclaw_core::Uuid;
use meclaw_core::serde_json::json;
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::MessageBuilder as TestMessageBuilder;
use meclaw_testing::topologies::phase_3b::build_phase_3b_topology;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_3b_ubf_echo_propagates_header_through_chain() {
    let mut topo = build_phase_3b_topology().await;

    let src = TestMessageBuilder::new("/a")
        .with_inline_messages(vec![
            json!({"origin": "user", "type": "text", "text": "hi"}),
        ])
        .with_header("session_id", json!("test-session"))
        .build();
    topo.colony.send_from(Path::new("/"), src).await;

    tokio::time::sleep(Duration::from_millis(150)).await;

    let mut received = Vec::new();
    while let Ok(m) = topo.c_rx.try_recv() {
        received.push(m);
    }
    assert_eq!(received.len(), 1, "exactly one message must reach /c");
    let msg = &received[0];
    assert_eq!(
        msg.headers.context.get("session_id"),
        Some(&json!("test-session")),
        "session_id from source propagates through chain via input_headers"
    );
    assert_eq!(
        msg.headers.hop.get("forwarded_by"),
        Some(&json!("/b")),
        "last-write-wins: /b's forwarded_by overrides /a's"
    );

    let body = match &msg.body {
        Body::Inline(v) => v,
        _ => panic!("expected inline body"),
    };
    assert!(
        body.get("header").is_none(),
        "content.header must be extracted into envelope.headers, not remain in body"
    );
    let msgs = body["messages"].as_array().expect("messages array");
    assert_eq!(msgs.len(), 3, "user-turn + 2 assistant-turns (a, b)");
    assert_eq!(msgs[0]["origin"], json!("user"));
    assert_eq!(msgs[1]["origin"], json!("assistant"));
    assert_eq!(msgs[2]["origin"], json!("assistant"));
    drop(received);

    topo.colony.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_3b_three_hop_parent_message_id_is_direct_not_grandparent() {
    let mut topo = build_phase_3b_topology().await;

    // Source carries a pre-existing parent (simulating a prior hop in the trace).
    let fake_grandparent = Uuid::now_v7();
    let trace = Uuid::now_v7();
    let src = MessageBuilder::new(Path::new("/a"))
        .trace_id(trace)
        .parent_message_id(fake_grandparent)
        .body(Body::Inline(json!({
            "messages": [{"origin": "user", "type": "text", "text": "hi"}]
        })))
        .build();
    let src_id = src.id;
    topo.colony.send_from(Path::new("/"), src).await;

    tokio::time::sleep(Duration::from_millis(150)).await;

    let mut received = Vec::new();
    while let Ok(m) = topo.c_rx.try_recv() {
        received.push(m);
    }
    assert_eq!(received.len(), 1);
    let msg = &received[0];
    assert_eq!(msg.trace_id, trace, "trace_id constant across chain");
    // The 3a fixup 6-fix regression: parent_message_id at /c must be the
    // ID of the message /b emitted — NOT src_id (grandparent at /c) and
    // NOT fake_grandparent (great-grandparent).
    assert!(
        msg.parent_message_id.is_some(),
        "follow-up must have parent_message_id"
    );
    assert_ne!(
        msg.parent_message_id,
        Some(src_id),
        "parent at /c must NOT be the source-id (grandparent)"
    );
    assert_ne!(
        msg.parent_message_id,
        Some(fake_grandparent),
        "parent at /c must NOT be the fake grandparent (great-grandparent)"
    );
    drop(received);

    topo.colony.shutdown().await;
}
