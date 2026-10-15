//! GH #159 — the return path out of a colony, driven through a real colony.
//!
//! The unit tests inside `colony.rs` pin the policy decision itself (marked out,
//! unmarked to the DLQ, `All` unchanged, no sink unchanged). This file answers the
//! one question they cannot: does the **marker survive the journey**?
//!
//! The whole return path rests on that. The HTTP layer stamps a request id into
//! `context` when it injects a message, the message travels cell → cell → root
//! hive, and the reply is only routable back to the browser that asked if the
//! stamp is still on it. The mechanism is `carry_context_with_hop` — a cell's
//! emission inherits the inbound `context` with a fresh `hop` — but a mechanism
//! read in the source is not a mechanism proven end to end, and this is the test
//! that would have caught it before anything was built on top.
//!
//! Deterministic by construction: the assertion is on what arrives on the egress
//! channel and on the dead-letter rows, never on timing.

use meclaw_core::{Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mocks::EchoMockCell;
use std::collections::HashSet;

const MARK: &str = "surface_reply";

/// One `context` compartment with a marker and a request id in it.
fn context_with(entries: &[(&str, &str)]) -> meclaw_core::Headers {
    let mut m = meclaw_core::serde_json::Map::new();
    for (k, v) in entries {
        m.insert((*k).to_string(), meclaw_core::serde_json::json!(v));
    }
    meclaw_core::Headers::from_parts(m, Default::default())
}

/// A colony with two echo cells wired `a -> b -> /`, and a marked egress sink.
///
/// `b -> /` is what a Direct-Mode topology writes to reach the outside; here it is
/// what makes the root hive the last stop, which is where the policy is applied.
async fn colony_with_two_cells() -> (
    tempfile::TempDir,
    ColonyHandle,
    tokio::sync::mpsc::Receiver<Message>,
) {
    let td = tempfile::TempDir::new().unwrap();
    let (h, rx) = ColonyHandle::new_with_marked_egress_at(&td, vec![], MARK);
    // `emitted_target` is not optional garnish: an EchoMockCell without it emits
    // nothing at all, so a cascade test built on the bare constructor proves
    // only that silence stays silent.
    h.spawn(Path::new("/a"), || {
        EchoMockCell::new(Path::new("/a")).emitted_target(Path::new("/b"))
    })
    .await;
    h.spawn(Path::new("/b"), || {
        EchoMockCell::new(Path::new("/b")).emitted_target(Path::new("/"))
    })
    .await;
    h.add_hive_scope(Path::new("/")).await;
    h.add_edge(
        meclaw_core::Uuid::now_v7(),
        Path::new("/a"),
        Path::new("/b"),
    )
    .await;
    h.add_edge(meclaw_core::Uuid::now_v7(), Path::new("/b"), Path::new("/"))
        .await;
    (td, h, rx)
}

/// **The load-bearing test.** A marker stamped once, at injection, must still be
/// there after two cells have handled the message and the root hive has handed it
/// out. If it is not, a reply cannot be matched to the browser that asked, and the
/// entire return path of #159 is broken.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_marker_and_the_request_id_survive_the_cascade() {
    let (_td, h, mut rx) = colony_with_two_cells().await;

    h.send(
        MessageBuilder::new(Path::new("/a"))
            .headers(context_with(&[(MARK, "1"), ("surface_request", "req-42")]))
            .build(),
    )
    .await;

    let out = tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv())
        .await
        .expect("a marked message must leave the colony")
        .expect("egress channel closed");

    assert_eq!(
        out.headers.context.get(MARK).and_then(|v| v.as_str()),
        Some("1"),
        "the egress marker did not survive the cascade"
    );
    assert_eq!(
        out.headers
            .context
            .get("surface_request")
            .and_then(|v| v.as_str()),
        Some("req-42"),
        "the request id did not survive the cascade — replies cannot be correlated"
    );
    h.shutdown().await;
}

/// The other half of the policy, in a live colony: an **unmarked** message on the
/// same wiring keeps dead-lettering. Without this, wiring a door in `--api` mode
/// would silently swallow every correct dead letter.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unmarked_message_on_the_same_wiring_dead_letters() {
    let (_td, h, mut rx) = colony_with_two_cells().await;

    h.send(MessageBuilder::new(Path::new("/a")).build()).await;

    // The receipt is positive on the DLQ side rather than negative on the channel:
    // wait until the dead letter is there, then assert the channel stayed empty.
    let mut dlq = Vec::new();
    for _ in 0..300 {
        dlq = h.drain_dead_letters().await;
        if !dlq.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(
        dlq.len(),
        1,
        "an unmarked root-hive dead end must dead-letter"
    );
    assert!(
        matches!(dlq[0].reason, meclaw_colony::DeadLetterReason::HiveNoRoute),
        "and with the unchanged reason, got {:?}",
        dlq[0].reason
    );
    assert!(
        rx.try_recv().is_err(),
        "an unmarked message must not reach the egress channel"
    );
    h.shutdown().await;
}

/// Two marked messages in flight at once must arrive as two distinguishable
/// answers. A return path that collapsed them would serve one browser another
/// browser's page, which is the failure a request id exists to prevent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_marked_messages_keep_their_own_request_ids() {
    let (_td, h, mut rx) = colony_with_two_cells().await;

    for id in ["req-1", "req-2"] {
        h.send(
            MessageBuilder::new(Path::new("/a"))
                .headers(context_with(&[(MARK, "1"), ("surface_request", id)]))
                .build(),
        )
        .await;
    }

    let mut seen: HashSet<String> = HashSet::new();
    for _ in 0..2 {
        let out = tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv())
            .await
            .expect("both marked messages must leave the colony")
            .expect("egress channel closed");
        seen.insert(
            out.headers
                .context
                .get("surface_request")
                .and_then(|v| v.as_str())
                .unwrap_or("<missing>")
                .to_string(),
        );
    }
    assert_eq!(
        seen,
        HashSet::from(["req-1".to_string(), "req-2".to_string()]),
        "two concurrent requests must arrive as two distinct ids"
    );
    h.shutdown().await;
}
