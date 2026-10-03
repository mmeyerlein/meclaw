//! GH #977 -- one omitted answer no longer tips the whole verdict.
//!
//! The decider may leave single questions unanswered. The `decisions` translate
//! then emits ONE decision with the answers it got and `missing` beside them;
//! `stage` keeps the sure topic and journals the omitted keys. Without the
//! `topic` question nothing opens -- proven by a sentinel turn: the decide edge
//! is ordered and `stage` is resident with one child, so the sentinel's data
//! request means the partial verdict before it was handled completely.

#[path = "support/display_colony.rs"]
mod display_colony;
#[path = "support/presenter_colony.rs"]
mod presenter_colony;

use meclaw_core::serde_json::json;
use presenter_colony::{Dials, body, boot, decision, decision_without, guard, stage_routes, warm};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh977_an_omitted_question_keeps_the_sure_topic() {
    if !guard("gh977_an_omitted_question_keeps_the_sure_topic") {
        return;
    }
    // The presenter's observed topics (`search`, `work`) stand beside `sample`:
    // their questions are the other topics whose omitted answers must not tip it.
    let mut s = boot(Dials::default()).await;
    warm(&s).await;
    s.install_sample().await;

    s.turn("t1", "show me a sample").await;
    s.ask().await.release(decision_without(
        &[
            ("topic", "sample", 0.9),
            ("sample.lead", "rows", 0.9),
            ("sample.also", "none", 0.9),
        ],
        &["search.lead"],
    ));
    let ask = s.out("in_show").await;
    assert_eq!(body(&ask)["turn_id"], json!("t1"));
    s.wait_drawn("show-sample-hint").await;
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
    // The omitted key is named, sorted, and none of `sample`'s is missing.
    let missing: Vec<String> =
        meclaw_core::serde_json::from_str(row["missing"].as_str().expect("a list")).unwrap();
    assert!(missing.iter().any(|k| k == "search.lead"), "{row}");
    assert!(!missing.iter().any(|k| k.starts_with("sample.")), "{row}");
    assert!(missing.windows(2).all(|w| w[0] < w[1]), "sorted: {row}");
    s.c.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh977_without_the_topic_question_nothing_shows() {
    if !guard("gh977_without_the_topic_question_nothing_shows") {
        return;
    }
    let mut s = boot(Dials::default()).await;
    warm(&s).await;
    s.install_sample().await;

    s.turn("t1", "show me a sample").await;
    s.ask().await.release(decision_without(
        &[("sample.lead", "rows", 0.9), ("sample.also", "none", 0.9)],
        &["topic"],
    ));
    // Sentinel: a sure turn behind it opens its window and asks for data.
    s.turn("t2", "show me a sample again").await;
    s.ask().await.release(decision(&[
        ("topic", "sample", 0.9),
        ("sample.lead", "rows", 0.9),
    ]));
    let ask = s.out("in_show").await;
    assert_eq!(body(&ask)["turn_id"], json!("t2"), "the sentinel's request");

    let views: Vec<_> = s
        .outputs_of_stage()
        .await
        .into_iter()
        .filter(|(r, _)| r == "view")
        .map(|(_, b)| b)
        .collect();
    assert!(
        views
            .iter()
            .all(|b| b.to_string().contains("\"t2\"") && !b.to_string().contains("\"t1\"")),
        "a view for the turn without a topic answer: {views:?}"
    );
    assert!(stage_routes(&s).await.iter().any(|r| r == "view"));
    let row = s.journal_of("t1").await;
    assert_eq!(row["fallback"], json!("error"), "{row}");
    assert_eq!(row["missing"], json!("[\"topic\"]"), "{row}");
    s.c.shutdown().await;
}
