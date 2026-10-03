//! GH #963 -- a web search shows only after a sure verdict, and only in the screen's round.
//!
//! Tool results are data (R-29-6, OR-DP-30): an observed `web_search` call and its result
//! open no window and ask the decider nothing. Only a turn the decider is sure about as
//! `search` opens the window with its working hint; the hits the generation's search then
//! returns fill it. Hits of a result stamped with another round never reach the screen.
//!
//! The taps the builder draws (`observes_tool_calls`, string-form `observes_tool_results`)
//! end on `./stage`, so the test delivers there what they would. Absence is proven by a
//! sentinel: `stage` is resident with one child, so the decide call of a LATER turn means
//! every earlier observation was handled -- and the log shows it emitted no view.

#[path = "support/display_colony.rs"]
mod display_colony;
#[path = "support/presenter_colony.rs"]
mod presenter_colony;

use meclaw_core::serde_json::{Value, json};
use presenter_colony::{Dials, boot, decision, guard, stage_routes, warm};

const ROUND: &str = r#"["agent:g1","member:alex"]"#;
const OTHER: &str = r#"["agent:g1","member:sam"]"#;

fn ctx(round: &str, turn: Option<&str>) -> Value {
    let mut c = json!({"audience_set": round, "tool_caller": "talky", "assistant": "g1",
                       "session_id": "s1"});
    if let Some(t) = turn {
        c["turn_id"] = json!(t);
    }
    c
}

fn hits(titles: &[&str]) -> String {
    let list: Vec<Value> = titles
        .iter()
        .enumerate()
        .map(|(i, t)| {
            json!({"title": t, "url": format!("https://site{i}.example/p"),
                             "snippet": format!("about {t}")})
        })
        .collect();
    Value::Array(list).to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_search_fills_a_window_only_after_a_sure_verdict() {
    if !guard("a_search_fills_a_window_only_after_a_sure_verdict") {
        return;
    }
    let mut s = boot(Dials {
        screen_audience: json!(["member:alex"]),
        ..Dials::default()
    })
    .await;
    warm(&s).await;

    // (a) A search observed without a sure verdict: no window. The sentinel turn's decide
    // call proves both observations were handled before it.
    s.tool_call(
        "web_search",
        "c0",
        json!({"query": "unasked"}),
        ctx(ROUND, None),
    )
    .await;
    s.tool_result(
        "c0",
        &hits(&["Unasked"]),
        json!({"operation": "web_search", "result_count": 1}),
        ctx(ROUND, None),
    )
    .await;
    s.turn_in("t0", "hello there", json!(["agent:g1", "member:alex"]))
        .await;
    s.decided_on("t0").await;
    assert!(
        !stage_routes(&s).await.iter().any(|r| r == "view"),
        "an observation opened a window"
    );
    s.ask().await.release(decision(&[("topic", "none", 0.9)]));

    // (b) A sure `search`: the hint first, while no result is there yet.
    s.turn_in(
        "t1",
        "search the web for actors",
        json!(["agent:g1", "member:alex"]),
    )
    .await;
    s.ask().await.release(decision(&[
        ("topic", "search", 0.9),
        ("search.lead", "results", 0.9),
        ("search.also", "none", 0.9),
    ]));
    s.wait_drawn("show-search-hint").await;
    assert!(
        !s.drawn("show-search-results").await,
        "a block before its result"
    );

    // A result of another round for the same turn: nothing placed. The sentinel turn's
    // decide call proves it was handled.
    s.tool_call(
        "web_search",
        "f1",
        json!({"query": "x"}),
        ctx(ROUND, Some("t1")),
    )
    .await;
    s.tool_result(
        "f1",
        &hits(&["Foreign one", "Foreign two"]),
        json!({"operation": "web_search"}),
        ctx(OTHER, Some("t1")),
    )
    .await;
    s.turn_in(
        "t2",
        "and something else",
        json!(["agent:g1", "member:alex"]),
    )
    .await;
    s.decided_on("t2").await;
    assert!(
        !s.drawn("show-search-results").await,
        "hits of another round were placed"
    );

    // The generation's own result in the turn's round fills the window.
    s.tool_call(
        "web_search",
        "c1",
        json!({"query": "actors"}),
        ctx(ROUND, Some("t1")),
    )
    .await;
    s.tool_result(
        "c1",
        &hits(&["Alpha", "Beta"]),
        json!({"operation": "web_search", "result_count": 2}),
        ctx(ROUND, Some("t1")),
    )
    .await;
    s.wait_drawn("show-search-results").await;
    let items = s.ids("show-search-results-").await;
    assert_eq!(items.len(), 2, "{items:?}");
    let tree = s.c.tree().await.to_string();
    assert!(
        !tree.contains("Foreign") && !tree.contains("Unasked"),
        "a hit of another round or turn reached the screen"
    );
    let row = s.journal_of("t1").await;
    assert_eq!(row["fallback"], json!("none"), "{row}");
    assert_eq!(row["topic"], json!("search"), "{row}");
    s.ask().await.release(decision(&[("topic", "none", 0.9)]));
    s.c.shutdown().await;
}
