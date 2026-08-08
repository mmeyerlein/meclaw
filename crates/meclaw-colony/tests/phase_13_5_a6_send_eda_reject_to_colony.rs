//! Phase-13.5-A6 T6 Regressions-Test (T6 requirement 2):
//!
//! Pins the `send_eda_reject` behaviour when a rejected mutation has
//! `reply_to = /colony/<endpoint>`. The reply path is the 4th call site of the
//! `RouteAction::ColonyDispatch` handling (outside the outputs arm and the 2
//! inbox routing sites), and is solved in T3 BY AN INLINE DLQ PUSH — NO
//! dispatch_colony_endpoint re-entry (that would risk infinite recursion on
//! /colony/mutations, plus send_eda_reject has no full colony_task state).
//!
//! Proof:
//!
//! - The mutation is submitted via ColonyMsg::Mutation with
//!   reply_to=Some(/colony/mutations) and fails deterministically
//!   (template_missing).
//! - send_eda_reject builds an error reply with target=/colony/mutations and
//!   routes it via route_with_log.
//! - route() returns RouteAction::ColonyDispatch with endpoint=/colony/mutations,
//!   sender=/colony.
//! - send_eda_reject's loop catches ColonyDispatch and does an inline DLQ push
//!   (reason=ColonyEndpointUnimplemented, resolved_target=/colony/mutations,
//!   sender_path=/colony — = the sender from RouteAction, from route()'s initial
//!   sender_path="/colony").
//! - NO infinite recursion: the test terminates well under 3s.

#![allow(clippy::expect_fun_call)]

use meclaw_colony::{ColonyMsg, MutationOutcome};
use meclaw_core::Path;
use meclaw_core::Uuid;
use meclaw_testing::ColonyHandle;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_eda_reject_to_colony_endpoint_lands_in_dlq_no_recursion() {
    let h = ColonyHandle::new();

    // Invalid mutation: non-existent template → template_missing reject.
    // reply_to = Some(/colony/mutations) — the pathological cell→/colony loop case.
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: meclaw_core::serde_json::json!({
                "scope": "/",
                "diff": {"add_nodes": [{"name": "x", "template": "doesnotexist"}]}
            }),
            reply_to: Some(Path::new("/colony/mutations")),
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();

    // The outcome must be Rejected — otherwise there is no send_eda_reject path.
    let outcome = ack_rx.await.unwrap();
    match outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(
                error_code, "template_missing",
                "expected template_missing reject"
            );
        }
        _ => panic!("expected Rejected, got {outcome:?}"),
    }

    // Poll the DLQ deterministically (no fixed sleep — same as the T5 hygiene
    // pattern). The test terminates in <3s, NO infinite recursion through a
    // /colony/mutations→/colony/mutations dispatch loop.
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let matched: Vec<meclaw_colony::DeadLetter> = loop {
        let snapshot = h.drain_dead_letters().await;
        let m: Vec<meclaw_colony::DeadLetter> = snapshot
            .into_iter()
            .filter(|d| d.resolved_target.as_str() == "/colony/mutations")
            .collect();
        if !m.is_empty() || std::time::Instant::now() >= deadline {
            break m;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(
        matched.len(),
        1,
        "send_eda_reject with reply_to=/colony/mutations must produce EXACTLY 1 DLQ entry \
         (no dispatch, no recursion); got: {} entries",
        matched.len()
    );

    let entry = &matched[0];
    assert!(
        matches!(
            entry.reason,
            meclaw_colony::DeadLetterReason::ColonyEndpointUnimplemented
        ),
        "DLQ reason must be ColonyEndpointUnimplemented (sender_path-pass-through inline DLQ); \
         got: {:?}",
        entry.reason
    );
    assert_eq!(
        entry.resolved_target.as_str(),
        "/colony/mutations",
        "DLQ resolved_target is the original colony-endpoint"
    );
    assert_eq!(
        entry.sender_path.as_str(),
        "/colony",
        "DLQ sender_path is /colony (= sender from RouteAction::ColonyDispatch, \
         passed from send_eda_reject's route_with_log call with sender=Path::new(\"/colony\"))"
    );

    h.shutdown().await;
}
