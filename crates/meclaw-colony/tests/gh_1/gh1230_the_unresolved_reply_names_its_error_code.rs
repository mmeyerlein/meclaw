//! GH #1230 — the routing-error reply to `reply_to` names its error code.
//!
//! When a resolved path is not in the registry and the message carries a
//! `reply_to`, the cascade's error reply goes to that anchor. Before #1230 the
//! reply was an empty envelope: the anchor saw *that* something failed, never
//! *what*. The reply now carries the canonical substrate error shape
//! (`hop.finish_reason: "error"` + `hop.error_code: "unresolved_path"`), the
//! same shape `consumes_violation`, `contract_violation`, `message_timeout`
//! and the `ttl_expired` notice already use.
//!
//! Topology class (AGENTS.md rule 11): multi-thread runtime, the reply is
//! awaited as the event itself under the hang mark; no time window is a verdict.

use meclaw_core::Path;
use meclaw_testing::MessageBuilder as TestMessageBuilder;
use meclaw_testing::deadline::hang;
use meclaw_testing::topologies::phase_3a::build_phase_3a_topology;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_unresolved_reply_carries_error_code_unresolved_path() {
    let mut topo = build_phase_3a_topology().await;

    let msg = TestMessageBuilder::new("/missing")
        .with_reply_to("/a/receiver")
        .build();
    topo.colony.send_from(Path::new("/"), msg).await;

    let reply = hang(
        "the routing-error reply reaches /a/receiver",
        topo.receiver_rx.recv(),
    )
    .await
    .expect("the /a/receiver tap stays open while the colony runs");

    assert_eq!(reply.target.as_str(), "/a/receiver");
    assert_eq!(
        reply
            .headers
            .hop
            .get("finish_reason")
            .and_then(|v| v.as_str()),
        Some("error"),
        "the reply is a substrate error reply: {reply:#?}"
    );
    assert_eq!(
        reply.headers.hop.get("error_code").and_then(|v| v.as_str()),
        Some("unresolved_path"),
        "the reply names why the route failed: {reply:#?}"
    );

    topo.colony.shutdown().await;
}
