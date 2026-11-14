//! One question, one window, and it shows what was asked (display-hive.md § 9b).
//!
//! A turn that asked for the weather of three places left three things on the screen: the
//! presenter's window with one place (the app's own set, the home town), a free card of the
//! speaker's sidecar with all three, and the pinned weather tile lifted beside them. The
//! three carried three topic keys, so the topic rule of § 4.12 compared nothing.
//!
//! These locks say what stands instead:
//!
//! 1. The result of a listed tool fills the window of its turn, whichever tool it is and
//!    whichever app owns the topic; several results of one turn stand together, in the
//!    order of the calls; the app's own set fills in until a result is there.
//! 2. A free card of a turn whose presenter window stands is a dupe -- the `turn_id`
//!    decides, not the topic key -- in either order of arrival. Without such a window the
//!    card opens as before.
//! 3. A pinned tile stays a tile while the window of the turn stands.
//! 4. An observed tool gives data, never a window (§ 9b.9, unchanged).
//!
//! The presenter runs on the stamp way (`verdict_source: stamp`), the way a colony draws
//! it: only there a window is rebuilt until it is done (§ 9b.5, § 9b.14). The screen has no
//! round of its own, so every set here is for everyone (`*`).

#[path = "support/display_colony.rs"]
mod display_colony;
use crate::display_colony::presenter_colony;

use display_colony::{attr, calls_of, present, tile};
use meclaw_core::serde_json::{Value, json};
use presenter_colony::{Dials, PRESENTER, Stage, boot, decision, guard, stage_routes, warm};

/// The app of the free card, the app of the pinned tile, and a bystander whose window is
/// the mark that the screen took every write before it.
const CARD_APP: &str = "/alex/apps/sidecar";
const TILE_APP: &str = "/alex/apps/ambient";
const MARK_APP: &str = "/alex/apps/note";

const TURN: &str = "keeper:01a18cc2:2026-10-30T12:22:23.516841Z";
const OTHER_TURN: &str = "keeper:01a18cc2:2026-10-30T12:22:31.002113Z";

/// A topic an app owns, with one candidate over one set. The set carries the name of the
/// tool that answers the topic, so "the set the results fill" has one reading.
fn places_topic() -> Value {
    json!({
        "topic": "sample", "title": "Sample", "glyph": "S", "standard": "weather",
        "describe": "the weather of the places the request names",
        "candidates": [
            {"key": "weather", "block": "display-list", "set": "weather",
             "describe": "the places as a list: place and one sentence",
             "bind": {"title": "=Places"},
             "children": [{"each": "weather", "block": "display-item",
                           "bind": {"k": "$.place.name", "v": "$.summary"}}]}
        ]
    })
}

/// The stamp of a turn the talky is sure about: topic `sample`, lead `lead`.
fn stamp(turn_id: &str, lead: &str) -> Value {
    json!({"turn_id": turn_id, "session_id": "s1", "channel": "chat", "path": "simple",
           "intent": "show", "choices": {},
           "screen": {"topic": "sample", "confidence": 0.95, "lead": lead,
                      "further": "none"}})
}

/// The context a generation's own tool hop carries: the round of the turn and its id.
fn generation(turn_id: Option<&str>) -> Value {
    let mut c = json!({"audience_set": r#"["*"]"#, "tool_caller": "talky", "assistant": "g1",
                       "session_id": "s1"});
    if let Some(t) = turn_id {
        c["turn_id"] = json!(t);
    }
    c
}

/// One row in the form the tool `weather` answers (templates/tools/README.md, `weather/`):
/// `place` is an object, `current` the values of the hour.
fn forecast_row(place: &str, summary: &str) -> Value {
    json!({"summary": summary,
           "place": {"name": place, "country": "Exampleland", "admin1": "North",
                     "latitude": 51.5, "longitude": 7.25},
           "timezone": "UTC",
           "current": {"time": "2026-11-12T09:00", "temp": 12.4, "weather_code": 1,
                       "condition": "mainly clear"},
           "days": [], "fetched_at": "2026-11-12T09:00:00Z", "source": "open-meteo"})
}

/// What the tool `weather` answers for one place: one JSON object as text.
fn forecast(place: &str) -> String {
    forecast_row(place, &format!("{place}: mild and dry")).to_string()
}

/// A presenter on the stamp way whose observed topics name the app's topic `sample`.
async fn presenter_with_observed_sample() -> Stage {
    boot(Dials {
        stamp_way: true,
        observed_topics: json!(["search", "work", "sample"]),
        ..Dials::default()
    })
    .await
}

/// Install `topic` as the app's one topic and prove `stage` wrote it down.
async fn install(s: &Stage, topic: Value) {
    s.topics(json!([topic])).await;
    s.c.settle().await;
    let rows = s.c.log(Some(&format!("{PRESENTER}/store"))).await;
    let accepted = rows.iter().filter_map(calls_of).flatten().any(|c| {
        c["operation"] == "insert"
            && c["table"] == "shows"
            && c["rows"]
                .as_array()
                .is_some_and(|rows| rows.iter().any(|r| r["topic"] == "sample"))
    });
    assert!(accepted, "the presenter took the topic `sample`");
}

/// One call of `weather` for `place` and nothing else: the call is at `stage` on return.
async fn call(s: &Stage, id: &str, place: &str, turn_id: Option<&str>) {
    s.tool_call("weather", id, json!({"place": place}), generation(turn_id))
        .await;
}

/// The result of the call `id`, as the generation's own tools hive stamps it.
async fn result(s: &Stage, id: &str, place: &str, turn_id: Option<&str>) {
    s.tool_result(id, &forecast(place), json!({}), generation(turn_id))
        .await;
}

/// The places the list `show-sample-weather` shows, in the order of the screen.
async fn places(s: &Stage) -> Vec<String> {
    let tree = s.c.tree().await;
    let mut rows: Vec<(f64, String, String)> = tree
        .as_object()
        .into_iter()
        .flatten()
        // The view itself, not its copies per surface (`monitor.`, `phone.`, `tv.`): the
        // first colony run counted each place four times.
        .filter(|(id, _)| id.starts_with("view.") && id.contains("show-sample-weather-"))
        .filter_map(|(id, node)| {
            let place = node["props"]["k"].as_str()?;
            Some((
                node["ord"].as_f64().unwrap_or(0.0),
                id.clone(),
                place.to_string(),
            ))
        })
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    rows.into_iter().map(|(_, _, place)| place).collect()
}

/// The turn `turn_id` gets its window: the words, the stamp, the data question to the app.
async fn open_window(s: &mut Stage, turn_id: &str, lead: &str, words: &str) {
    s.fanned_turn(words).await;
    s.keeper_stamp(stamp(turn_id, lead)).await;
    let _ask = s.out("in_show").await;
    s.wait_drawn("show-sample").await;
}

/// The window of `turn_id` over the shipped sample topic, filled and done: its lead stands.
async fn standing_window(s: &mut Stage, turn_id: &str, words: &str) {
    open_window(s, turn_id, "rows", words).await;
    s.data(
        turn_id,
        json!({"rows": {"audience_set": ["*"],
                        "rows": [{"name": "alpha", "audience_set": ["*"]}]}}),
    )
    .await;
    s.wait_drawn("show-sample-rows").await;
}

/// The props of a free card as the sidecar's app writes it: a canvas window of the
/// conversation with a topic of its own and the turn it answers.
fn card(turn_id: &str) -> Value {
    json!({"title": "Weather in three places", "context": "conversation", "layer": "canvas",
           "pinned": false, "relevance": "0.7", "topic": "weather:north-south-east",
           "turn_id": turn_id})
}

/// A window of a bystander, drawn: the screen took every write sent before it.
async fn mark(s: &Stage) -> Value {
    s.c.put(
        MARK_APP,
        "note",
        json!({"title": "Note", "context": "work", "relevance": "0.9", "topic": "note:1"}),
    )
    .await
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. The window shows the data of the turn
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_onewindow_a_tool_result_of_the_turn_fills_the_topic_window() {
    if !guard("gh_d54g_onewindow_a_tool_result_of_the_turn_fills_the_topic_window") {
        return;
    }
    let mut s = presenter_with_observed_sample().await;
    warm(&s).await;
    install(&s, places_topic()).await;

    // The app is asked and stays silent: the one source of this window is the turn's tool.
    open_window(&mut s, TURN, "weather", "how is the weather in Southport").await;
    call(&s, "w1", "Southport", Some(TURN)).await;
    result(&s, "w1", "Southport", Some(TURN)).await;

    assert!(
        s.drawn("show-sample-weather").await,
        "the result of the turn's tool placed the lead of the topic window"
    );
    assert_eq!(
        places(&s).await,
        vec!["Southport".to_string()],
        "the window shows the place the turn asked the tool for"
    );
    s.c.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_onewindow_three_results_of_one_tool_stand_in_one_window() {
    if !guard("gh_d54g_onewindow_three_results_of_one_tool_stand_in_one_window") {
        return;
    }
    let mut s = presenter_with_observed_sample().await;
    warm(&s).await;
    install(&s, places_topic()).await;

    open_window(
        &mut s,
        TURN,
        "weather",
        "the weather in Northfield, Southport and Eastham",
    )
    .await;
    // Three calls in one order, their results in another: the set keeps the order of the
    // calls, because that is the order the request named the places in.
    call(&s, "w1", "Northfield", Some(TURN)).await;
    call(&s, "w2", "Southport", Some(TURN)).await;
    call(&s, "w3", "Eastham", Some(TURN)).await;
    result(&s, "w2", "Southport", Some(TURN)).await;
    result(&s, "w3", "Eastham", Some(TURN)).await;
    result(&s, "w1", "Northfield", Some(TURN)).await;

    assert_eq!(
        places(&s).await,
        vec![
            "Northfield".to_string(),
            "Southport".to_string(),
            "Eastham".to_string()
        ],
        "every result of the turn stands in the window, in the order of the calls"
    );
    let views: Vec<Value> = s
        .outputs_of_stage()
        .await
        .into_iter()
        .filter(|(route, _)| route == "view")
        .map(|(_, body)| body["view_id"].clone())
        .collect();
    assert!(
        !views.is_empty() && views.iter().all(|v| v == "show-sample"),
        "three results are one window, not three: {views:?}"
    );
    s.c.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_onewindow_the_apps_own_set_fills_in_until_a_result_comes() {
    if !guard("gh_d54g_onewindow_the_apps_own_set_fills_in_until_a_result_comes") {
        return;
    }
    let mut s = presenter_with_observed_sample().await;
    warm(&s).await;
    install(&s, places_topic()).await;

    open_window(&mut s, TURN, "weather", "how is the weather in Southport").await;
    // The app answers first, as it does in a colony: it knows the home town, not the turn.
    let mut home = forecast_row("Hometown", "Hometown: grey");
    home["audience_set"] = json!(["*"]);
    s.data(
        TURN,
        json!({"weather": {"audience_set": ["*"], "rows": [home]}}),
    )
    .await;
    s.wait_drawn("show-sample-weather").await;
    assert_eq!(
        places(&s).await,
        vec!["Hometown".to_string()],
        "the app's own set fills the window while no result of the turn is there"
    );

    // The turn's tool answers seconds later: its result takes the place of the app's set.
    call(&s, "w1", "Southport", Some(TURN)).await;
    result(&s, "w1", "Southport", Some(TURN)).await;
    assert_eq!(
        places(&s).await,
        vec!["Southport".to_string()],
        "the result of the turn replaced the app's own set"
    );
    s.c.shutdown().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. and 3. One turn, one window -- in either order
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_onewindow_a_free_card_of_a_turn_with_a_window_is_a_dupe() {
    if !guard("gh_d54g_onewindow_a_free_card_of_a_turn_with_a_window_is_a_dupe") {
        return;
    }
    let mut s = boot(Dials {
        stamp_way: true,
        ..Dials::default()
    })
    .await;
    warm(&s).await;
    s.install_sample().await;
    standing_window(&mut s, TURN, "show me a sample").await;

    // The sidecar of the same turn asks for a card. It never stands, so its absence is read
    // behind a mark: the bystander's window is drawn by a pass that came after the card.
    let fresh = s.c.oid(CARD_APP, "card-1");
    s.c.write_view(CARD_APP, "card-1", card(TURN)).await;
    let tree = mark(&s).await;
    assert!(
        !present(&tree, &fresh),
        "a free card of a turn whose window stands opens no window"
    );
    assert!(
        tile(&tree, &fresh).is_null(),
        "and it has no tile: {}",
        tile(&tree, &fresh)
    );
    assert!(
        s.drawn("show-sample-rows").await,
        "the window of the turn stands as it stood"
    );
    s.c.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_onewindow_a_free_card_that_came_first_is_withdrawn() {
    if !guard("gh_d54g_onewindow_a_free_card_that_came_first_is_withdrawn") {
        return;
    }
    let mut s = boot(Dials {
        stamp_way: true,
        ..Dials::default()
    })
    .await;
    warm(&s).await;
    s.install_sample().await;

    // The card is first: the sidecar wrote it before the stamp of its turn arrived.
    let first = s.c.oid(CARD_APP, "card-1");
    let tree = s.c.put(CARD_APP, "card-1", card(TURN)).await;
    assert!(present(&tree, &first), "the card stands while it is alone");

    standing_window(&mut s, TURN, "show me a sample").await;
    let tree =
        s.c.wait_tree(
            "the free card of the turn leaves once the turn's window stands",
            |t| !present(t, &first),
        )
        .await;
    assert!(
        tile(&tree, &first).is_null(),
        "and its tile leaves with it: {}",
        tile(&tree, &first)
    );
    assert!(
        s.drawn("show-sample-rows").await,
        "the window of the turn stands"
    );
    s.c.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_onewindow_a_free_card_without_a_window_opens_as_before() {
    if !guard("gh_d54g_onewindow_a_free_card_without_a_window_opens_as_before") {
        return;
    }
    let mut s = boot(Dials {
        stamp_way: true,
        ..Dials::default()
    })
    .await;
    warm(&s).await;
    s.install_sample().await;

    // (a) No presenter window at all: the sidecar stays the way for free cards (§ 10).
    let alone = s.c.oid(CARD_APP, "card-0");
    let tree = s.c.put(CARD_APP, "card-0", card(OTHER_TURN)).await;
    assert!(present(&tree, &alone), "a free card alone opens");
    assert!(
        attr(&tree, &alone, "rung").is_string(),
        "and carries a rung: {}",
        attr(&tree, &alone, "rung")
    );

    // (b) A presenter window stands, but of ANOTHER turn: it is no rival of this card.
    standing_window(&mut s, TURN, "show me a sample").await;
    let beside = s.c.oid(CARD_APP, "card-2");
    let tree = s.c.put(CARD_APP, "card-2", card(OTHER_TURN)).await;
    assert!(
        present(&tree, &beside),
        "a free card of another turn opens beside the window"
    );
    assert!(
        attr(&tree, &beside, "rung").is_string(),
        "and carries a rung: {}",
        attr(&tree, &beside, "rung")
    );
    assert!(
        s.drawn("show-sample-rows").await,
        "the window of the other turn stands"
    );
    s.c.shutdown().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. A pinned tile stays a tile
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_onewindow_a_pinned_tile_is_not_lifted_by_the_turn() {
    if !guard("gh_d54g_onewindow_a_pinned_tile_is_not_lifted_by_the_turn") {
        return;
    }
    let mut s = boot(Dials {
        stamp_way: true,
        ..Dials::default()
    })
    .await;
    warm(&s).await;
    s.install_sample().await;

    // The standing tile of an app on the same subject, pinned, below the bar.
    let pinned = s.c.oid(TILE_APP, "weather");
    let quiet = json!({"title": "Weather", "context": "ambient", "layer": "canvas",
                       "pinned": true, "relevance": "0.3", "topic": "weather:hometown"});
    s.c.put(TILE_APP, "weather", quiet).await;
    standing_window(&mut s, TURN, "show me a sample").await;
    let before = s.c.tree().await;
    assert_eq!(
        attr(&before, &pinned, "rung"),
        json!("ambient"),
        "the pinned tile is a tile before the turn touches it"
    );

    // What the tile's app wrote in the measured turn: its tile again, with the turn's id,
    // a raised relevance and a touch.
    let lifted = json!({"title": "Weather", "context": "ambient", "layer": "canvas",
                        "pinned": true, "relevance": "0.8", "topic": "weather:hometown",
                        "touched": "2", "turn_id": TURN});
    let after = s.c.put(TILE_APP, "weather", lifted).await;
    assert_eq!(
        attr(&after, &pinned, "rung"),
        json!("ambient"),
        "the turn has its window: the pinned tile stays a tile"
    );
    assert!(
        s.drawn("show-sample-rows").await,
        "the window of the turn stands"
    );
    s.c.shutdown().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// § 9b.9, unchanged: an observation is data, never a window
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_onewindow_an_observed_tool_never_opens_a_window_by_itself() {
    if !guard("gh_d54g_onewindow_an_observed_tool_never_opens_a_window_by_itself") {
        return;
    }
    // The selector's way: its decide call is the sentinel that orders `stage`.
    let mut s = boot(Dials {
        observed_topics: json!(["search", "work", "sample"]),
        ..Dials::default()
    })
    .await;
    warm(&s).await;
    install(&s, places_topic()).await;

    // A call and its result for the listed topic's tool, and no verdict for any turn.
    call(&s, "w0", "Southport", None).await;
    result(&s, "w0", "Southport", None).await;
    // `stage` takes its inbox in order: the decide call of a later turn means both
    // observations were handled before it.
    s.turn("t0", "hello there").await;
    s.decided_on("t0").await;
    s.ask().await.release(decision(&[("topic", "none", 0.9)]));
    assert!(
        !stage_routes(&s).await.iter().any(|r| r == "view"),
        "an observation opened a window"
    );
    assert!(
        !s.drawn("show-sample").await,
        "the listed topic has a window without a verdict"
    );
    s.c.shutdown().await;
}
