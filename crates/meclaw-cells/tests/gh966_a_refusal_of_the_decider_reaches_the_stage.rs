//! GH #966 (N1.3) -- a refusal of the presenter's decider that is no turn's verdict
//! reaches `stage` and leaves the presenter as `error` `decide_refused`.
//!
//! Measured in the review of the presenter (strand P): the `in_model` door passed the
//! context unfiltered onto `./decide`, so a push that brought `show_origin = 'decide'`
//! and a `show_id` came back from a refusal as that turn's verdict; and a refusal
//! without `hop.refused_subscriber` (a push addressed to another cell, or one without
//! `subscriber`) matched no edge and ended inside the hive as `no_route`. Now the door
//! deletes the inner context, and the one edge for "neither a turn's verdict nor the
//! registry's refusal" carries the refusal to `stage`, which says it out loud.
//!
//! Both pushes are read at the egress: the error that leaves `/alex` is the receipt.

#[path = "support/display_colony.rs"]
mod display_colony;
#[path = "support/presenter_colony.rs"]
mod presenter_colony;

use meclaw_core::serde_json::json;
use presenter_colony::{Dials, PRESENTER, boot, guard, to_path};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_push_leaves_as_an_error_not_as_a_verdict() {
    if !guard("a_refused_push_leaves_as_an_error_not_as_a_verdict") {
        return;
    }
    let mut s = boot(Dials::default()).await;
    s.install_sample().await;

    // (a) A push for another cell that brings the inner markers of a turn: without the
    // door's `delete_context` the refusal posed as the verdict of `ghost`, and `stage`
    // dropped it as a verdict for no pending turn -- nothing left.
    for (n, context) in [
        json!({"show_origin": "decide", "show_id": "ghost"}),
        // (b) The same push without any context: without the edge to `stage` it was a
        // `no_route` inside the hive.
        json!({}),
    ]
    .into_iter()
    .enumerate()
    {
        s.c.h
            .send(to_path(
                PRESENTER,
                json!({"route": "in_model", "subscriber": "/elsewhere/brain"}),
                context,
                json!({"system": {}, "params": {"model": "vendor/other"}}),
            ))
            .await;
        let refused = s.out("error").await;
        assert_eq!(
            refused.headers.hop.get("error_code"),
            Some(&json!("decide_refused")),
            "push {n}: {:?}",
            refused.headers.hop
        );
        let detail = refused
            .headers
            .hop
            .get("detail")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        assert!(
            detail.contains("invalid_input"),
            "push {n}: the error names what the decider said: {detail}"
        );
    }
    s.c.shutdown().await;
}
