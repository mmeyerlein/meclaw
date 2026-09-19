//! GH #705 -- a tap lands, however many windows already stand open.
//!
//! Measured on a monitor during acceptance: after a reload exactly two tiles could be
//! brought onto the canvas. The third tap put its window up for one frame and the server
//! took it straight back into its tile. The screen had a fixed number of canvas places
//! and handed them out by score, so the tapped window landed behind the ones already
//! standing -- outside the places, back in its tile.
//!
//! There are no places any more. The state holds one level per window (§ 4.24) and an
//! output draws every open window (§ 6.2: "never fewer windows"); how they are arranged
//! is the profile's business and nobody's decision about existence. So the lock is the
//! sentence itself: nine windows open, a tap on the tenth tile, and afterwards TEN are
//! open -- the finger leads its ladder while `led_until` runs (§ 4.19, § 5.1), whatever
//! the score says.
//!
//! The second tap on the same tile is the put-away of § 5.2: the window closes, the
//! others keep standing, and the tile keeps its rank (R-24-1).

mod support;

use meclaw_core::serde_json::{Value, json};
use support::{Screen, component_view, library_ships, pane, window_id};

/// One monitor with a dock big enough for every tile: the cut of § 4.30 is another
/// lock's business and would only hide what this one measures.
fn monitor() -> Value {
    json!({"screens": {"monitor": {"display_type": "monitor", "viewing_distance_m": 0.7,
                                   "inputs": ["audio", "pointer", "keyboard"],
                                   "dock_max": 20, "dock_default": "shown"}},
           "default_screen": "monitor"})
}

fn window(name: &str, relevance: &str) -> Value {
    component_view(
        name,
        "main",
        pane(name, json!({"context": "work", "relevance": relevance})),
    )
}

/// Nine loud windows open on the canvas and one quiet one down in its tile.
fn crowded() -> Screen {
    let mut screen = Screen::new(monitor());
    for i in 0..9 {
        screen.write(window(&format!("w{i}"), "0.9"), 1000);
    }
    screen.write(window("quiet", "0.05"), 1000);
    // § 4.35: a window takes no focus in the pass it appears in, so one more pass before
    // anything is measured.
    screen.pass(json!({"kind": "stroke"}), 2000);
    screen
}

fn open_windows(screen: &Screen) -> Vec<String> {
    let state = screen.screen_state();
    let mut out: Vec<String> = state["views"]
        .as_object()
        .expect("the state keys its views")
        .iter()
        .filter(|(_, v)| v["curator"]["open"] == json!(true))
        .map(|(oid, _)| oid.clone())
        .collect();
    out.sort();
    out
}

/// The finger decides, and nothing steps aside for it (§ 5.1, § 6.2).
#[test]
fn a_tap_opens_a_tenth_window_beside_nine_open_ones() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = crowded();
    let quiet = window_id("alex", "quiet");
    assert_eq!(
        open_windows(&screen).len(),
        9,
        "the fixture has to start with the quiet window OUT of the canvas, or it proves \
         nothing"
    );
    assert_eq!(screen.curator(&quiet, "rung"), json!("ambient"));

    screen.pass(json!({"kind": "tap", "for": &quiet}), 3000);

    assert_eq!(
        open_windows(&screen).len(),
        10,
        "the tapped window opens and not one of the nine closes (§ 6.2)"
    );
    assert_eq!(
        screen.curator(&quiet, "rung"),
        json!("focus"),
        "the finger leads its ladder while `led_until` runs (§ 4.19)"
    );
    assert_eq!(screen.curator(&quiet, "level"), json!(1));
    assert_eq!(
        screen.curator(&quiet, "since"),
        json!(3000),
        "`since` = now"
    );
    assert_eq!(
        screen.curator(&quiet, "led_until"),
        json!(3000 + 20_000),
        "and the lead lasts one linger (§ 4.9, § 4.15)"
    );

    // The lowest score on the screen, and it leads anyway: that is the whole sentence.
    let score = screen.curator(&quiet, "score").as_f64().unwrap_or(1.0);
    for i in 0..9 {
        let other = window_id("alex", &format!("w{i}"));
        assert!(
            screen.curator(&other, "score").as_f64().unwrap_or(0.0) > score,
            "{other} scores higher than the tapped window and stays open all the same"
        );
        assert_ne!(
            screen.curator(&other, "rung"),
            json!("focus"),
            "one focus per ladder (§ 4.20): {other} steps back without closing"
        );
    }
    // Every output draws it large now (§ 6.2): the window object wears level 1.
    let drawn = screen
        .props(&format!("monitor.{quiet}/c.quiet"))
        .expect("the monitor draws the tapped window");
    assert_eq!(drawn["level"], "1", "{drawn}");
    assert_eq!(drawn["led"], "1", "and says the finger is on it: {drawn}");
}

/// The second tap on the same tile is a put-away (§ 5.2): the window closes, the nine
/// keep standing, and the tile keeps its rank (R-24-1).
#[test]
fn a_second_tap_puts_it_away_and_leaves_the_rest_open() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = crowded();
    let quiet = window_id("alex", "quiet");
    screen.pass(json!({"kind": "tap", "for": &quiet}), 3000);
    let rank = screen.curator(&quiet, "rank");
    assert_eq!(open_windows(&screen).len(), 10);

    screen.pass(json!({"kind": "tap", "for": &quiet}), 4000);

    assert_eq!(
        open_windows(&screen).len(),
        9,
        "only the tapped window goes back into its tile"
    );
    assert_eq!(screen.curator(&quiet, "level"), json!(0));
    assert_eq!(
        screen.curator(&quiet, "rung"),
        json!("hidden"),
        "a put-away window scores 0 (§ 4.14, § 4.22)"
    );
    assert_eq!(screen.curator(&quiet, "dismissed_at"), json!(4000));
    assert_eq!(
        screen.curator(&quiet, "since"),
        json!(3000),
        "`since` stays: a put-away is no touch (§ 5.2)"
    );
    assert_eq!(
        screen.curator(&quiet, "rank"),
        rank,
        "and the tile keeps its place in the dock (R-24-1)"
    );
    let tile = screen
        .props(&format!(
            "monitor.display.dock/tile.{}",
            quiet.replace('/', "~")
        ))
        .expect("the tile stands");
    assert_eq!(
        tile["open"], "",
        "§ 6.10: the tile says it is closed: {tile}"
    );
}
