//! Phase-3b demo: UBF body validation + content.header extraction
//! + 3-hop direct-parent regression (closes the 3a gap from fixup 6-fix).
//!
//! Topology class (AGENTS.md rule 11): multi-thread runtime, event and
//! invariant assertions only, no time window as a verdict. The arrival at `/c`
//! is awaited as the event itself (the capture cell's channel) under the hang
//! mark; that nothing further reached `/c` is read after the colony's
//! quiescence barrier (`ColonyHandle::quiesce`), which every hop of the chain
//! follows from.

use meclaw_core::Uuid;
use meclaw_core::serde_json::json;
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::MessageBuilder as TestMessageBuilder;
use meclaw_testing::deadline::hang;
use meclaw_testing::topologies::phase_3b::{Phase3bTopology, build_phase_3b_topology};

/// The one message that reaches `/c`: its arrival is awaited as an event,
/// and after quiescence nothing more may have followed it.
async fn the_one_message_at_c(topo: &mut Phase3bTopology) -> meclaw_core::Message {
    let msg = hang("the chain's message reaches /c", topo.c_rx.recv())
        .await
        .expect("the /c capture channel stays open while the colony runs");
    topo.colony.quiesce().await;
    let extra = topo.c_rx.try_recv().ok();
    assert!(
        extra.is_none(),
        "exactly one message must reach /c; also got {extra:#?}"
    );
    msg
}

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

    let msg = the_one_message_at_c(&mut topo).await;
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

    let msg = the_one_message_at_c(&mut topo).await;
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

    topo.colony.shutdown().await;
}
