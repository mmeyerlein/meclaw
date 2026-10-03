//! GH #959 -- the data request follows the verdict, and the window builds up block by block.
//!
//! What `stage` emits on the turn is the call to the decider and nothing towards the app
//! (R-29-6, OR-DP-29); what it emits on the sure verdict is the view AND the data request,
//! caused by the same message (one output); an `on_choice` set travels only when its
//! candidate was chosen. Lead `rows` and also `steps` with the `steps` set coming
//! later: two patches, each carrying one whole block, rows first. With every chosen block
//! placed the turn closes: its pending row keeps neither the words nor the data (OR-DP.P.4).
//! Data from an app that does not own the topic is refused out loud and draws nothing; a
//! newer turn of the topic replaces the older one, whose late data never reaches the new
//! window (sentinel: the newer turn's block).

#[path = "support/display_colony.rs"]
mod display_colony;
#[path = "support/presenter_colony.rs"]
mod presenter_colony;

use meclaw_core::serde_json::{Value, json};
use presenter_colony::{Dials, body, boot, decision, guard, stage_routes, warm};

fn creates(patch: &[Value], needle: &str) -> bool {
    patch.iter().any(|c| {
        c["op"] == "object.create" && c["id"].as_str().is_some_and(|id| id.contains(needle))
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn data_follows_the_verdict_and_builds_up() {
    if !guard("data_follows_the_verdict_and_builds_up") {
        return;
    }
    let mut s = boot(Dials::default()).await;
    warm(&s).await;
    s.install_sample().await;

    s.turn("d1", "show me a sample").await;
    let ask = s.ask().await;
    assert!(
        !stage_routes(&s)
            .await
            .iter()
            .any(|r| r == "in_show" || r == "view"),
        "the turn's output reaches past the decider"
    );
    ask.release(decision(&[
        ("topic", "sample", 0.9),
        ("sample.lead", "rows", 0.9),
        ("sample.also", "steps", 0.6),
    ]));
    let req = s.out("in_show").await;
    assert_eq!(
        body(&req)["sets"],
        json!(["brief", "rows", "steps"]),
        "no on_choice set"
    );

    // One output: the view and the request share the message that caused them.
    let rows = s.c.log(None).await;
    let from_stage: Vec<_> = rows
        .iter()
        .filter(|r| r.from_path == format!("{}/stage", presenter_colony::PRESENTER))
        .collect();
    let parent = |route: &str| {
        from_stage
            .iter()
            .find(|r| display_colony::hop_of(r)["route"] == route)
            .and_then(|r| r.parent_message_id.clone())
    };
    assert!(parent("view").is_some());
    assert_eq!(parent("view"), parent("in_show"), "two outputs, not one");

    s.data(
        "d1",
        json!({"rows": {"audience_set": ["*"],
                                 "rows": [{"name": "alpha", "audience_set": ["*"]}]}}),
    )
    .await;
    s.wait_drawn("show-sample-rows").await;
    assert!(!s.drawn("show-sample-steps").await);
    s.data(
        "d1",
        json!({"steps": {"audience_set": ["*"], "rows": [
        {"label": "one", "audience_set": ["*"]}, {"label": "two", "audience_set": ["*"]}]}}),
    )
    .await;
    s.wait_drawn("show-sample-steps").await;
    let patches = s.c.patches().await;
    let rows_at = patches
        .iter()
        .position(|p| creates(p, "show-sample-rows"))
        .expect("rows");
    let steps_at = patches
        .iter()
        .position(|p| creates(p, "show-sample-steps"))
        .expect("steps");
    assert!(rows_at < steps_at, "rows first");
    assert!(
        creates(&patches[steps_at], "show-sample-steps-0")
            && creates(&patches[steps_at], "show-sample-steps-1"),
        "the steps block arrived in pieces"
    );

    // Every chosen block stands: the turn is done, the words and the data are gone.
    let closed = s.pending_done("d1").await;
    assert_eq!(
        closed["text"],
        json!(""),
        "the words stayed in the pending row"
    );
    assert_eq!(
        closed["sets"],
        json!({}),
        "the data stayed in the pending row"
    );
    assert!(
        !closed.to_string().contains("alpha"),
        "a bound block stayed in the pending row"
    );

    // An on_choice set travels when its candidate is the lead.
    s.turn("d2", "the slow number").await;
    s.ask().await.release(decision(&[
        ("topic", "sample", 0.9),
        ("sample.lead", "slow", 0.9),
        ("sample.also", "none", 0.9),
    ]));
    let req2 = s.out("in_show").await;
    assert_eq!(
        body(&req2)["sets"],
        json!(["brief", "rows", "steps", "slow"])
    );

    // Data from an app that does not own the topic: refused out loud, nothing drawn.
    s.data_from(
        "/alex/apps/stranger",
        "d2",
        json!({"slow": {"audience_set": ["*"],
                                                    "value": {"n": 7}}}),
    )
    .await;
    let refused = s.out("error").await;
    assert_eq!(
        refused.headers.hop.get("error_code"),
        Some(&json!("foreign_data"))
    );

    // A newer turn of the topic replaces d2; d2's late data never reaches the new window.
    s.turn("d3", "a sample brief").await;
    s.ask().await.release(decision(&[
        ("topic", "sample", 0.9),
        ("sample.lead", "brief", 0.9),
        ("sample.also", "none", 0.9),
    ]));
    s.out("in_show").await;
    assert_eq!(s.journal_of("d2").await["fallback"], json!("no_data"));
    s.data(
        "d2",
        json!({"slow": {"audience_set": ["*"], "value": {"n": 7}}}),
    )
    .await;
    s.data(
        "d3",
        json!({"brief": {"audience_set": ["*"], "value": {"title": "sentinel"}}}),
    )
    .await;
    s.wait_drawn("show-sample-brief").await;
    assert!(
        !s.drawn("show-sample-slow").await,
        "the older turn's data reached the newer window"
    );
    let undeclared = s.undeclared_emissions().await;
    assert!(
        undeclared.is_empty(),
        "emissions outside contract.emits: {undeclared:?}"
    );
    s.c.shutdown().await;
}
