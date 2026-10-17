//! GH #1231 (T6-01): `/colony/trace` is not answered to an emission.
//!
//! The trace read hands out the raw message-log rows of every cell in the
//! colony. A cell that may only touch its own state must not be able to read
//! them by emitting to `/colony/trace` — the same class as `/colony/messages`
//! and `/colony/dead_letters`. The emission is hard-rejected with one
//! `colony_endpoint_unimplemented` dead letter that names the sender; the HTTP
//! read `GET /colony/trace` and `meclaw ask` stay the owner's way in.

use meclaw_core::Path;
use meclaw_core::serde_json::json;
use meclaw_testing::ColonyHandle;
use meclaw_testing::MessageBuilder;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_emission_to_colony_trace_is_hard_rejected() {
    let colony = ColonyHandle::new();
    // `/reader` is no node: a served read would answer it and that answer
    // would dead-letter under `/reader`, never under `/colony/trace`.
    let msg = MessageBuilder::new("/colony/trace")
        .with_inline_content(json!({"messages": [], "query": {"limit": 5}}))
        .with_reply_to("/reader")
        .build();
    colony.send_from(Path::new("/reader"), msg).await;
    colony.quiesce().await;

    let dls = colony.drain_dead_letters().await;
    let rejects: Vec<_> = dls
        .iter()
        .filter(|dl| dl.resolved_target.as_str() == "/colony/trace")
        .collect();
    assert_eq!(
        rejects.len(),
        1,
        "exactly one hard reject for the /colony/trace emission; got: {:?}",
        dls.iter()
            .map(|d| (d.resolved_target.as_str(), d.reason.as_code()))
            .collect::<Vec<_>>()
    );
    assert_eq!(rejects[0].reason.as_code(), "colony_endpoint_unimplemented");
    assert_eq!(rejects[0].sender_path.as_str(), "/reader");

    colony.shutdown().await;
}
