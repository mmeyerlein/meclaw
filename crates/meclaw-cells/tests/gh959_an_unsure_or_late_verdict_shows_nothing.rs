//! GH #959 -- unsure, `none`, too late or no topic at all: nothing on the screen.
//!
//! (a) confidence under the threshold -> no view, no data request, journal `unsure`;
//! (b) `none` -> journal `no_topic`; (c) the decider is held past `budget_ms` until the
//! journal says `timeout`, then answers sure -> still no view, journal `late`; (d) a
//! presenter without topics asks the decider nothing (sentinel: the first request it ever
//! sees is the turn after the topic arrived). Absence is read at the emitter after the
//! journal row of the same output stands, never after a wait.

#[path = "support/display_colony.rs"]
mod display_colony;
#[path = "support/presenter_colony.rs"]
mod presenter_colony;

use meclaw_core::serde_json::json;
use presenter_colony::{Dials, boot, decision, guard, stage_routes};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unsure_or_none_verdict_shows_nothing() {
    if !guard("an_unsure_or_none_verdict_shows_nothing") {
        return;
    }
    let mut s = boot(Dials::default()).await;
    s.install_sample().await;

    s.turn("u1", "maybe a sample").await;
    s.ask().await.release(decision(&[("topic", "sample", 0.4)]));
    assert_eq!(s.journal_of("u1").await["fallback"], json!("unsure"));
    s.turn("u2", "thanks").await;
    s.ask().await.release(decision(&[("topic", "none", 0.95)]));
    assert_eq!(s.journal_of("u2").await["fallback"], json!("no_topic"));

    let routes = stage_routes(&s).await;
    assert!(
        !routes.iter().any(|r| r == "view" || r == "in_show"),
        "nothing may leave towards the screen or the app: {routes:?}"
    );
    s.c.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_late_verdict_changes_nothing() {
    if !guard("a_late_verdict_changes_nothing") {
        return;
    }
    let mut s = boot(Dials {
        budget_ms: 200,
        ..Dials::default()
    })
    .await;
    s.install_sample().await;
    s.turn("l1", "show me a sample").await;
    let held = s.ask().await;
    let row = s.journal_of("l1").await;
    assert_eq!(row["fallback"], json!("timeout"), "{row}");
    held.release(decision(&[
        ("topic", "sample", 0.9),
        ("sample.lead", "rows", 0.9),
    ]));
    s.c.wait_until("the late mark", || async {
        s.journal()
            .await
            .iter()
            .any(|r| r["turn_id"] == "l1" && r["late"] == json!(1))
    })
    .await;
    let routes = stage_routes(&s).await;
    assert!(
        !routes.iter().any(|r| r == "view" || r == "in_show"),
        "{routes:?}"
    );
    s.c.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_presenter_without_topics_asks_nothing() {
    if !guard("a_presenter_without_topics_asks_nothing") {
        return;
    }
    let mut s = boot(Dials::default()).await;
    s.turn("n1", "before any topic").await;
    s.install_sample().await;
    s.turn("n2", "after the topic").await;
    let first = s.ask().await;
    assert_eq!(
        first.json()["state"],
        json!("after the topic"),
        "the turn before any topic reached the decider"
    );
    first.release(decision(&[("topic", "none", 0.9)]));
    s.c.shutdown().await;
}
