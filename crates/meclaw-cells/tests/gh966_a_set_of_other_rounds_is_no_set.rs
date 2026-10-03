//! GH #966 (N1.7, N review I-1) -- a set whose own round covers the screen but whose rows
//! all belong to other rounds, with no `value`, is no set: it is EXACTLY a set that never
//! came. Nothing of it is kept, the lead waits for the clock as it would for a missing set,
//! and at `data_wait_ms` the window is withdrawn with `no_data` -- the standard does NOT
//! stand at once, because that difference would tell the screen that rows of other rounds
//! exist (an app that leaves an empty set out would otherwise betray them).
//!
//! The screen's round is `["member", "X"]`; the set's round is the union of its rows'
//! rounds `["X", "Y", "Z", "member"]` and covers it, but the rows `["member", "Y"]` and
//! `["X", "Z"]` do not. Read at the receiver: the withdrawal reaches the screen (the
//! sentinel -- the screen takes the presenter's lane in order), no block of the topic and
//! no row ever stood at `web`, and the journal says `no_data`.
//!
//! Second case (N review side note, Z2): a set whose OWN round `["Y"]` does not cover the
//! screen is the same set that never came -- it used to be kept as empty, so the standard
//! stood at once and told the screen that a set of another round exists.
#[path = "support/display_colony.rs"]
mod display_colony;
#[path = "support/presenter_colony.rs"]
mod presenter_colony;

use display_colony::{SCREEN, body_of, hop_of};
use meclaw_core::serde_json::json;
use presenter_colony::{Dials, boot, decision, guard};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_set_of_other_rounds_is_a_set_that_never_came() {
    if !guard("a_set_of_other_rounds_is_a_set_that_never_came") {
        return;
    }
    never_came(
        json!({"rows": {"audience_set": ["X", "Y", "Z", "member"], "rows": [
            {"name": "row-of-member-y", "audience_set": ["member", "Y"]},
            {"name": "row-of-x-z", "audience_set": ["X", "Z"]}
        ]}}),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_set_whose_round_does_not_cover_the_screen_never_came() {
    if !guard("a_set_whose_round_does_not_cover_the_screen_never_came") {
        return;
    }
    never_came(json!({"rows": {"audience_set": ["Y"], "rows": [
        {"name": "row-of-member-y", "audience_set": ["Y"]},
        {"name": "row-of-x-z"}
    ]}}))
    .await;
}

async fn never_came(hidden: meclaw_core::serde_json::Value) {
    let mut s = boot(Dials {
        screen_audience: json!(["member", "X"]),
        // The clock is what a missing set waits for; short, so the withdrawal comes soon.
        data_wait_ms: 3_000,
        ..Dials::default()
    })
    .await;
    s.install_sample().await;
    s.turn("o1", "show me a sample").await;
    s.ask().await.release(decision(&[
        ("topic", "sample", 0.9),
        ("sample.lead", "rows", 0.9),
        ("sample.also", "none", 0.9),
    ]));
    s.out("in_show").await;
    s.data("o1", hidden).await;
    s.data(
        "o1",
        json!({"brief": {"audience_set": ["*"], "value": {"title": "the brief"}}}),
    )
    .await;
    // The sentinel: the window's withdrawal reaches the screen at the deadline.
    s.c.wait_until("the sample window is withdrawn at the screen", || async {
        s.c.log(Some(SCREEN))
            .await
            .iter()
            .any(|r| hop_of(r)["route"] == "in_withdraw" && body_of(r)["view_id"] == "show-sample")
    })
    .await;
    assert_eq!(s.journal_of("o1").await["fallback"], json!("no_data"));
    assert!(
        !s.drawn("show-sample-brief").await,
        "the standard stood at once: a set of other rounds read as a set that came empty"
    );
    let tree = s.c.tree().await;
    let text = tree.to_string();
    assert!(
        !text.contains("show-sample-rows"),
        "a block of a set the screen may not see stands: {text}"
    );
    assert!(
        !text.contains("row-of-member-y") && !text.contains("row-of-x-z"),
        "a row of another round reached the screen"
    );
    s.c.shutdown().await;
}
