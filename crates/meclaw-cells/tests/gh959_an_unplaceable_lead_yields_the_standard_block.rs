//! GH #959 -- an unplaceable lead yields the standard block, and a row of a foreign round
//! never reaches the screen.
//!
//! The screen's round is `["a"]`. The lead `rows` came, visible, with a `value` and no
//! rows: nothing to place, so the standard `brief` stands (journal `invalid`). (A lead
//! set whose rows ALL belong to other rounds is not this case since GH #966 N review I-1:
//! it is a set that never came -- `gh966_a_set_of_other_rounds_is_no_set`.) A second turn's
//! rows `["a"]`, `["a","b"]`, `["b"]`, none, `["*"]` in a set of `["*"]` -> all but the
//! third at web: a row without its own round inherits the set's (OR-DP-56), a row with one
//! is gated alone. The decider never sees a data value: its requests carry `state` and
//! `questions` only.

#[path = "support/display_colony.rs"]
mod display_colony;
#[path = "support/presenter_colony.rs"]
mod presenter_colony;

use meclaw_core::serde_json::json;
use presenter_colony::{Dials, boot, decision, guard};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unplaceable_lead_yields_the_standard_block() {
    if !guard("an_unplaceable_lead_yields_the_standard_block") {
        return;
    }
    let mut s = boot(Dials {
        screen_audience: json!(["a"]),
        ..Dials::default()
    })
    .await;
    s.install_sample().await;
    let sure = || {
        decision(&[
            ("topic", "sample", 0.9),
            ("sample.lead", "rows", 0.9),
            ("sample.also", "none", 0.9),
        ])
    };

    s.turn("p1", "show me a sample").await;
    let ask1 = s.ask().await;
    let body1 = ask1.json();
    ask1.release(sure());
    s.out("in_show").await;
    s.data(
        "p1",
        json!({
            "rows": {"audience_set": ["*"], "value": {"n": 1}},
            "brief": {"audience_set": ["*"], "value": {"title": "the brief"}}
        }),
    )
    .await;
    s.wait_drawn("show-sample-brief").await;
    assert_eq!(s.journal_of("p1").await["fallback"], json!("invalid"));

    s.turn("p2", "the sample rows again").await;
    let ask2 = s.ask().await;
    let body2 = ask2.json();
    ask2.release(sure());
    s.out("in_show").await;
    s.data(
        "p2",
        json!({"rows": {"audience_set": ["*"], "rows": [
            {"name": "r1", "audience_set": ["a"]},
            {"name": "r2", "audience_set": ["a", "b"]},
            {"name": "r3", "audience_set": ["b"]},
            {"name": "r4"},
            {"name": "r5", "audience_set": ["*"]}
        ]}}),
    )
    .await;
    s.wait_drawn("show-sample-rows").await;
    let tree = s.c.tree().await;
    let mut shown: Vec<String> = tree
        .as_object()
        .expect("a tree")
        .iter()
        // The view itself, not its copies per surface (`monitor.`, `phone.`, `tv.`): a
        // colony run counted each row four times.
        .filter(|(id, _)| id.starts_with("view.") && id.contains("show-sample-rows-"))
        .filter_map(|(_, n)| n["props"]["k"].as_str().map(str::to_string))
        .collect();
    shown.sort();
    let ids: Vec<&String> = tree
        .as_object()
        .expect("a tree")
        .keys()
        .filter(|id| id.contains("show-sample-rows-"))
        .collect();
    assert_eq!(shown, ["r1", "r2", "r4", "r5"], "{ids:?}");
    assert!(
        !tree.to_string().contains("foreign-row"),
        "a foreign row reached the screen"
    );

    for asked in [body1, body2] {
        let keys: Vec<&String> = asked.as_object().expect("an object").keys().collect();
        assert_eq!(keys, ["model", "questions", "state"]);
        let text = asked.to_string();
        assert!(
            !text.contains("the brief") && !text.contains("r1"),
            "a data value: {text}"
        );
    }
    s.c.shutdown().await;
}
