//! GH #959 -- a sure verdict opens the window with a working hint, and the data fills it.
//!
//! The two times of the plan, as a causal chain at the receiver (`web`), never as a
//! clock: the window with its hint reaches the screen WHILE the app still holds its data
//! (no app hop before the first reaction), and with the data the chosen block stands.
//! Before the decider is released nothing is shown -- proven by a sentinel turn: `stage`
//! is resident with one child, so the decider's second request means the first turn was
//! handled completely, and its handling emitted no view.

#[path = "support/display_colony.rs"]
mod display_colony;
#[path = "support/presenter_colony.rs"]
mod presenter_colony;

use meclaw_core::serde_json::json;
use presenter_colony::{Dials, body, boot, decision, guard, stage_routes, warm};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sure_verdict_opens_the_window_and_fills_it() {
    if !guard("a_sure_verdict_opens_the_window_and_fills_it") {
        return;
    }
    let mut s = boot(Dials::default()).await;
    warm(&s).await;
    s.install_sample().await;

    s.turn("t1", "show me a sample").await;
    let first = s.ask().await;
    let asked = first.json();
    assert_eq!(
        asked["state"],
        json!("show me a sample"),
        "the state is the turn's text"
    );
    let keys: Vec<&String> = asked.as_object().expect("an object").keys().collect();
    assert_eq!(
        keys,
        ["model", "questions", "state"],
        "the call carries nothing else"
    );

    // Sentinel: `stage` sends the decide call of a second turn, so the first was handled
    // -- and no view. The decider is serial, so the second call reaches the stub only
    // after the first is released.
    s.turn("t1b", "and another thing").await;
    s.decided_on("t1b").await;
    assert!(
        !stage_routes(&s).await.iter().any(|r| r == "view"),
        "a view left before the verdict"
    );

    first.release(decision(&[
        ("topic", "sample", 0.9),
        ("sample.lead", "rows", 0.9),
        ("sample.also", "none", 0.9),
    ]));
    s.ask().await.release(decision(&[("topic", "none", 0.9)]));
    let ask = s.out("in_show").await;
    assert_eq!(body(&ask)["op"], json!("data"));
    assert_eq!(body(&ask)["turn_id"], json!("t1"));
    // A-review M-2: without `show_app` the request would be the topics question to all.
    assert_eq!(
        ask.headers.hop.get("show_app"),
        Some(&json!(presenter_colony::SAMPLE_APP)),
        "the data request names its app"
    );
    // First reaction: the hint stands at web while the app holds its data.
    s.wait_drawn("show-sample-hint").await;
    assert!(
        !s.drawn("show-sample-rows").await,
        "a block before its data"
    );

    s.data(
        "t1",
        json!({"rows": {"audience_set": ["*"],
                        "rows": [{"name": "alpha", "audience_set": ["*"]}]}}),
    )
    .await;
    s.wait_drawn("show-sample-rows").await;

    let row = s.journal_of("t1").await;
    assert_eq!(row["fallback"], json!("none"), "{row}");
    assert_eq!(row["lead"], json!("rows"), "{row}");
    let t = |k: &str| row[k].as_i64().unwrap_or_else(|| panic!("{k} in {row}"));
    assert!(
        t("t_verdict_ms") <= t("t_window_ms") && t("t_window_ms") <= t("t_content_ms"),
        "{row}"
    );
    assert!(
        !row.to_string().contains("show me a sample"),
        "the journal carries no text"
    );
    s.c.shutdown().await;
}
