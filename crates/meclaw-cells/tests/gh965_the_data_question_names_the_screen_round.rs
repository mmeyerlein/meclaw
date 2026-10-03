//! GH #965 (ruling OR-DP-82) -- the data question names the screen's round.
//!
//! An app that CHOOSES between rows of different rounds (one list out of several, the
//! newest one as a fallback) would reveal through its visible choice that rows of a
//! foreign round exist, as long as it does not know the screen's round. So `stage`
//! sends `screen_audience` with every `in_show {op: "data"}`: the sorted canonical list
//! of its param, empty when the screen has no round (the app then keeps to its member's
//! own rows, fail-closed). Read at the receiver: the `in_show` that leaves the presenter's
//! level is what the `shows` edge carries to the app.

#[path = "support/display_colony.rs"]
mod display_colony;
#[path = "support/presenter_colony.rs"]
mod presenter_colony;

use meclaw_core::serde_json::{Value, json};
use presenter_colony::{Dials, body, boot, decision, guard, warm};

async fn data_question(screen_audience: Value) -> Value {
    let mut s = boot(Dials {
        screen_audience,
        ..Dials::default()
    })
    .await;
    warm(&s).await;
    s.install_sample().await;
    s.turn("r1", "show me a sample").await;
    let ask = s.ask().await;
    ask.release(decision(&[
        ("topic", "sample", 0.9),
        ("sample.lead", "rows", 0.9),
    ]));
    let req = s.out("in_show").await;
    let b = body(&req);
    assert_eq!(b["op"], json!("data"), "the data question: {b}");
    b
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_data_question_carries_the_screen_round_sorted() {
    if !guard("the_data_question_carries_the_screen_round_sorted") {
        return;
    }
    let b = data_question(json!(["peer:x", "member:alex"])).await;
    assert_eq!(
        b["screen_audience"],
        json!(["member:alex", "peer:x"]),
        "the app reads the screen's round, canonical: {b}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_screen_without_a_round_sends_an_empty_one() {
    if !guard("a_screen_without_a_round_sends_an_empty_one") {
        return;
    }
    let b = data_question(json!([])).await;
    assert_eq!(
        b["screen_audience"],
        json!([]),
        "no round on the screen: the field stands, empty (fail-closed at the app): {b}"
    );
}
