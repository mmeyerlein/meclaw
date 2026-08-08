//! Phase-13.5-A6 T6 Regressions-Test (T6 requirement 2):
//!
//! Pinnt das `send_eda_reject`-Verhalten, wenn eine rejected Mutation
//! `reply_to = /colony/<endpoint>` hat. Der Reply-Pfad ist die 4. callsite
//! des `RouteAction::ColonyDispatch`-handlings (außerhalb von outputs-arm
//! und den 2 inbox-routing-stellen), und wird in T3 DURCH INLINE DLQ-PUSH
//! gelöst — KEIN dispatch_colony_endpoint-Reentry (würde infinite-recursion
//! bei /colony/mutations riskieren, plus send_eda_reject hat keinen vollen
//! colony_task-State).
//!
//! Beweis:
//!
//! - Mutation wird via ColonyMsg::Mutation mit reply_to=Some(/colony/mutations)
//!   eingereicht und scheitert deterministisch (template_missing).
//! - send_eda_reject baut error-reply mit target=/colony/mutations und routet
//!   sie via route_with_log.
//! - route() returnt RouteAction::ColonyDispatch mit endpoint=/colony/mutations,
//!   sender=/colony.
//! - send_eda_reject's loop fängt ColonyDispatch und macht inline DLQ-Push
//!   (reason=ColonyEndpointUnimplemented, resolved_target=/colony/mutations,
//!   sender_path=/colony — = sender aus RouteAction, von route()'s initialem
//!   sender_path="/colony").
//! - KEINE infinite-recursion: Test terminiert in deutlich unter 3s.

#![allow(clippy::expect_fun_call)]

use meclaw_colony::{ColonyMsg, MutationOutcome};
use meclaw_core::Path;
use meclaw_core::Uuid;
use meclaw_testing::ColonyHandle;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_eda_reject_to_colony_endpoint_lands_in_dlq_no_recursion() {
    let h = ColonyHandle::new();

    // Invalide Mutation: nicht-existierendes Template → template_missing reject.
    // reply_to = Some(/colony/mutations) — der pathologische Cell→/colony-loop-fall.
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

    // Outcome muss Rejected sein — sonst kein send_eda_reject-Pfad.
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

    // Poll DLQ deterministisch (kein fixed sleep — analog T5-Hygiene-Pattern).
    // Test terminiert in <3s, KEINE infinite-recursion durch /colony/mutations→
    // /colony/mutations-Dispatch-loop.
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
