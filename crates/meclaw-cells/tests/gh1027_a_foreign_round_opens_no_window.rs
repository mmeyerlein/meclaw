//! GH #1027 -- a turn of a foreign round opens no window on the member's screen.
//!
//! Measured (display before-run, scene "a guest asks"): a turn whose
//! round was `["agent:…", "person:guest"]` -- the member not in the conversation -- went
//! to the decider, and its sure verdict opened `show-calendar` with the working hint on
//! all three outputs of the member; only the data sets were gated afterwards (`no_data`).
//! The open window alone told the screen that a guest asked about the member's calendar.
//!
//! The rule now: the turn's round is checked against the screen's round BEFORE anything
//! happens. A round that does not cover the screen's asks no decider and opens nothing;
//! its journal row says `foreign_round`. Proven at the receiver with a sentinel: `stage`
//! handles one turn after the other, so when the decider's FIRST request is the member's
//! later turn, the guest's turn was handled completely -- and nothing of it reached the
//! screen.

#[path = "support/display_colony.rs"]
mod display_colony;
#[path = "support/presenter_colony.rs"]
mod presenter_colony;

use meclaw_core::serde_json::json;
use presenter_colony::{Dials, boot, decision, guard, stage_routes, warm};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_of_a_foreign_round_opens_no_window() {
    if !guard("a_turn_of_a_foreign_round_opens_no_window") {
        return;
    }
    let mut s = boot(Dials {
        screen_audience: json!(["member:alex"]),
        ..Dials::default()
    })
    .await;
    warm(&s).await;
    s.install_sample().await;

    // The guest's turn: the member is not part of the round.
    s.turn_in(
        "g1",
        "show me her sample",
        json!(["agent:g1", "person:guest"]),
    )
    .await;
    // The sentinel: the member's own turn on the same screen.
    s.turn_in(
        "m1",
        "show me my sample",
        json!(["agent:g1", "member:alex"]),
    )
    .await;

    let first = s.ask().await;
    assert_eq!(
        first.json()["state"],
        json!("show me my sample"),
        "the decider was asked about the guest's turn"
    );
    let row = s.journal_of("g1").await;
    assert_eq!(row["fallback"], json!("foreign_round"), "{row}");
    assert!(
        !row.to_string().contains("her sample"),
        "the journal carries no text: {row}"
    );
    assert!(
        !stage_routes(&s).await.iter().any(|r| r == "view"),
        "a view left for the guest's turn"
    );
    assert!(
        !s.drawn("show-sample").await,
        "the guest's turn reached the screen as a window"
    );

    // And the member's turn on the same screen still opens its window.
    first.release(decision(&[
        ("topic", "sample", 0.9),
        ("sample.lead", "rows", 0.9),
        ("sample.also", "none", 0.9),
    ]));
    s.wait_drawn("show-sample-hint").await;
    s.c.shutdown().await;
}

/// A turn without any round on a screen with one is no member turn either: fail-closed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_without_a_round_opens_no_window_on_a_screen_with_one() {
    if !guard("a_turn_without_a_round_opens_no_window_on_a_screen_with_one") {
        return;
    }
    let mut s = boot(Dials {
        screen_audience: json!(["member:alex"]),
        ..Dials::default()
    })
    .await;
    warm(&s).await;
    s.install_sample().await;

    s.turn_in("n1", "show me a sample", json!(null)).await;
    s.turn_in("m1", "show me my sample", json!(["member:alex"]))
        .await;
    let first = s.ask().await;
    assert_eq!(first.json()["state"], json!("show me my sample"));
    let row = s.journal_of("n1").await;
    assert_eq!(row["fallback"], json!("foreign_round"), "{row}");
    assert!(!s.drawn("show-sample").await);
    first.release(decision(&[("topic", "none", 0.9)]));
    s.c.shutdown().await;
}
