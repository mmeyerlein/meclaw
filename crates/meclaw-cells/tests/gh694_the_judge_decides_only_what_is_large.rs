//! display-hive.md § 4.4 / § 4.5 -- the judge decides only what is LARGE.
//!
//! Three sentences, one door. What the judge is shown is the window's own words and its
//! own last verdict, never the dock it stands in (R-23-6). What it is told is that a dock
//! exists and that hiding a window costs nothing but the space. And what it may weigh to
//! nothing is the score, not the ORDER: a context weighed to zero still leaves a readable
//! dock, because the rank keeps a floor of 0.05 that the score does not (§ 4.27).
//!
//! The script runs through the shared harness (`support`), the way a `code` cell runs it.

mod support;

use meclaw_core::serde_json::{Value, json};
use support::{COMPOSE, Screen, component_view, library_ships, pane, repo, window_id};

fn params() -> Value {
    json!({"judge": "on", "judge_min_interval_ms": 3000,
           "screens": {"monitor": {"display_type": "monitor", "inputs": ["pointer"]}},
           "default_screen": "monitor"})
}

fn tile_of(oid: &str) -> String {
    format!("display.dock/tile.{oid}")
}

/// What the judge was shown in the last pass, or none when it was not asked.
fn question(screen: &Screen) -> Option<Value> {
    let asked = screen.lane("judge");
    assert!(asked.len() <= 1, "at most one question a pass");
    asked.first().map(|e| {
        meclaw_core::serde_json::from_str(e["messages"][0]["text"].as_str().expect("the payload"))
            .expect("the payload is JSON")
    })
}

/// The judge is shown a WINDOW, named by the window id of § 2 -- not the node inside it
/// that happens to carry the props, and not a tile. What it gets are the words the
/// application said about itself, the rung the pass reached and the window's own last
/// verdict; the dock it stands in is not among them since 2.4.0 (R-23-6).
#[test]
fn the_situation_carries_the_windows_own_words_and_not_its_dock() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(params());
    screen.write(
        component_view(
            "a",
            "main",
            pane(
                "a",
                json!({"title": "Chat", "context": "conversation", "relevance": "0.8",
                       "topic": "chat", "class": "note"}),
            ),
        ),
        100_000,
    );
    let seen = question(&screen).expect("an app touch asks the judge");
    let oid = window_id("alex", "a");
    let window = seen["windows"]
        .as_array()
        .expect("a list")
        .iter()
        .find(|w| w["id"] == oid.as_str())
        .unwrap_or_else(|| panic!("the window is named by its window id: {seen}"));

    // The application's own words, as the door took them.
    assert_eq!(window["owner"], "alex", "{window}");
    assert_eq!(window["topic"], "chat", "{window}");
    assert_eq!(window["context"], "conversation", "{window}");
    assert_eq!(window["class"], "note", "{window}");
    assert_eq!(window["relevance"], 0.8, "{window}");
    // A glimpse of the text, so the judge weighs a window by what it says (§ 4.4).
    assert_eq!(window["text"], "Chat", "{window}");
    // And the pass's own answer: the rung, the age and whether the window leads its
    // ladder -- `leads` is `rung == focus`, nothing about a canvas.
    assert_eq!(window["rung"], "relevant", "{window}");
    assert_eq!(window["age"], "fresh", "{window}");
    assert_eq!(window["age_ms"], 0, "{window}");
    assert_eq!(window["leads"], false, "{window}");

    // The dock, in every shape it used to arrive in. It really does stand in one -- the
    // tile below proves the absence is an absence and not an empty screen.
    for gone in ["tile", "rank", "topic_dupe", "on_canvas"] {
        assert!(
            window.get(gone).is_none(),
            "`{gone}` was the dock talking to the judge (R-23-6): {window}"
        );
    }
    assert!(
        screen.props(&tile_of(&oid)).is_some(),
        "and the window does have a tile: {:?}",
        screen.held
    );
}

/// The prompt says what the verdict may and may not do.
#[test]
fn the_prompt_says_the_dock_is_not_the_verdicts_business() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let source = std::fs::read_to_string(repo(COMPOSE)).expect("the script ships");
    for sentence in [
        // Since 2.4.0 the sentence names the dock without measuring it (R-23-6): the judge
        // is told that a dock exists and that its own verdict is about something else,
        // never how much fits in it.
        "The dock shows everything that is present; your verdict decides only what stands LARGE.",
        "A window you hide keeps its tile",
        "topic_dupe",
        "below 0.05 is read as 0.05",
        // § 4.5: the next run replaces the whole verdict, and the judge is told so -- a
        // window it stops naming loses what it said about it last time.
        "Your verdict REPLACES the one before it",
    ] {
        assert!(source.contains(sentence), "the judge is told: {sentence:?}");
    }
}

/// A weight of zero does not take a window out of the dock (OR-D10). The score follows the
/// weight to nothing, so the window closes; the rank raises the weight to the floor of
/// 0.05, so the tile keeps a place in the order (§ 4.14 against § 4.27).
#[test]
fn a_weight_of_zero_still_leaves_an_order() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(params());
    let oid = window_id("alex", "a");
    screen.write(
        component_view(
            "a",
            "main",
            pane(
                "a",
                json!({"title": "A", "context": "ambient", "relevance": "0.5"}),
            ),
        ),
        100_000,
    );
    assert_eq!(
        screen.curator(&oid, "score"),
        0.25,
        "0.5 x 0.5 x 1 while no verdict stands"
    );

    // The judge answers, weighing `ambient` to nothing.
    screen.pass(
        json!({"kind": "verdict", "bar": 0.5, "weights": {"ambient": 0.0}, "windows": {}}),
        101_000,
    );
    assert_eq!(screen.screen_state()["weights"]["ambient"], 0.0);
    assert_eq!(
        screen.curator(&oid, "score"),
        0.0,
        "a weight of nothing scores nothing"
    );
    assert_eq!(screen.curator(&oid, "open"), false, "so the window closes");

    // But the dock keeps an order, and the tile keeps standing in it.
    assert_eq!(
        screen.curator(&oid, "rank"),
        0.025,
        "the rank raises the weight to its floor: 0.05 x 0.5 x 1"
    );
    let tile = screen
        .props(&tile_of(&oid))
        .unwrap_or_else(|| panic!("the tile stands: {:?}", screen.held));
    assert_eq!(tile["rank"], "0.025", "{tile}");
    assert_eq!(tile["open"], "", "and it says the window is closed: {tile}");
}
