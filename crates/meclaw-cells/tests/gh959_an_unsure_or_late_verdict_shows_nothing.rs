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
use crate::display_colony::presenter_colony;

use display_colony::Why;
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
    // The budget ran out while the decider is held: the row is read under that hold.
    let row = s.journal_of_held(&held, "l1").await.because(Why::Held);
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
    // GH #963: the shipped presenter offers its own observed topics on every turn; a
    // presenter WITHOUT topics is one with none of its own either.
    let mut s = boot(Dials {
        observed_topics: json!([]),
        ..Dials::default()
    })
    .await;
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

/// Re-check tv-bn M7b5 B-M2: a colony that does not settle within the hang mark fails with
/// `HANG: the colony settles` AND its last state -- the dead letters and the newest hops.
/// The barrier of the kit said only how long it waited, and the fixture's own account of a
/// hang settled again inside the failure (2 x HANG, then no account at all).
///
/// The held decider is what keeps the colony from rest: its own timeout is dialled past
/// twice the scaled hang mark, so the settle runs into the mark while `decide` still waits.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_colony_that_does_not_settle_names_its_last_state() {
    use futures_util::FutureExt;

    if !guard("a_colony_that_does_not_settle_names_its_last_state") {
        return;
    }
    let past = meclaw_testing::deadline::scaled(meclaw_testing::deadline::HANG) * 2;
    let mut s = boot(Dials {
        decide_timeout_ms: u64::try_from(past.as_millis()).expect("a timeout in ms"),
        ..Dials::default()
    })
    .await;
    s.install_sample().await;
    s.turn("h1", "show me a sample").await;
    let held = s.ask().await;
    let hang = std::panic::AssertUnwindSafe(s.c.settle())
        .catch_unwind()
        .await
        .expect_err("a colony whose decider is held does not settle within the hang mark");
    let text = hang
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_else(|| format!("{:?}", hang.downcast_ref::<&str>()));
    assert!(
        text.starts_with("HANG: the colony settles (waited "),
        "the hang is the kit's HANG line: {text}"
    );
    assert!(
        text.contains("; last state: DLQ ["),
        "the hang names the dead letters as its last state: {text}"
    );
    assert!(
        text.contains("log rows: [") && text.contains(" -> ") && text.contains(" route="),
        "the hang names the newest hops of the log: {text}"
    );
    held.release(decision(&[("topic", "none", 0.9)]));
    s.c.shutdown().await;
}
