//! display-hive.md § 4.30: `dock_max` per output is the maximum number of DRAWN tiles,
//! without exception. The cut takes, in this order, the other tiles by rank (lowest first),
//! then urgent tiles, then pinned tiles, then seat tiles (highest `seat_ord` first); a tile
//! falls in the LATEST stage that applies to it, so a pinned urgent falls in the pinned
//! stage. What does not fit is missing on this output (R-23-6) -- the app stays present and
//! its window stays open (R-24-2).
//!
//! This is the ONE place an output decides anything. S-055 and S-031 as objects; it replaces
//! `gh702_each_exit_shows_what_it_can_carry.rs`, which pinned the struck `canvas_slots`.

mod support;

use meclaw_core::serde_json::{Value, json};
use support::{Screen, library_ships, pane, view_of};

fn params() -> Value {
    json!({"linger_ms": 20000, "fade_ms": 120000,
           "screens": {"monitor": {"display_type": "monitor", "inputs": ["pointer"]},
                       "three": {"display_type": "phone", "dock_max": 3}},
           "default_screen": "monitor"})
}

/// The tiles this output draws, bottom to top (`ord` runs the other way: the column is
/// anchored at the bottom edge).
fn drawn(screen: &Screen, exit: &str) -> Vec<String> {
    let prefix = format!("{exit}.display.dock/tile.");
    let mut tiles: Vec<(i64, String)> = screen
        .held
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| o["component"] == "display-tile")
        .filter_map(|o| {
            let id = o["id"].as_str()?;
            let rest = id.strip_prefix(&prefix)?;
            Some((o["ord"].as_i64().unwrap_or(0), rest.to_string()))
        })
        .collect();
    tiles.sort_by_key(|(ord, _)| -ord);
    tiles.into_iter().map(|(_, id)| id).collect()
}

#[test]
fn the_cut_reaches_the_seats_last_and_a_pinned_urgent_falls_as_pinned() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(params());
    screen.write(
        view_of(
            "ambient",
            "clock",
            "main",
            pane(
                "clock",
                json!({"seat": "bottom", "seat_ord": "0", "pinned": true,
                                     "topic": "clock", "context": "ambient", "title": "Clock"}),
            ),
        ),
        1_000,
    );
    screen.write(
        view_of(
            "ambient",
            "weather",
            "main",
            pane(
                "weather",
                json!({"seat": "bottom", "seat_ord": "10", "pinned": true,
                                       "topic": "weather:berlin", "context": "ambient",
                                       "title": "Weather"}),
            ),
        ),
        1_000,
    );
    for (view_id, props) in [
        (
            "other",
            json!({"relevance": "0.9", "context": "system", "topic": "x:o", "title": "Other"}),
        ),
        (
            "urgent",
            json!({"state": "urgent", "touched": "100000", "relevance": "0.1",
                          "context": "system", "topic": "x:u", "title": "Urgent"}),
        ),
        (
            "pinned",
            json!({"pinned": true, "relevance": "0.1", "context": "system",
                          "topic": "x:p", "title": "Pinned"}),
        ),
        (
            "pinurg",
            json!({"pinned": true, "state": "urgent", "touched": "100000",
                          "relevance": "0.1", "context": "system", "topic": "x:pu",
                          "title": "PinUrg"}),
        ),
    ] {
        screen.write(view_of("x", view_id, "main", pane(view_id, props)), 100_000);
    }

    // The systemwide order (§ 4.28): seats at the bottom, the rest by rank.
    assert_eq!(
        drawn(&screen, "monitor"),
        vec![
            "view.ambient.clock",
            "view.ambient.weather",
            "view.x.pinned",
            "view.x.other",
            "view.x.urgent",
            "view.x.pinurg",
        ],
        "the whole dock, uncut"
    );
    // Three tiles: the other falls first, then the urgent, then the pinned -- and the
    // pinned urgent falls in the PINNED stage, so it is the last of the three to go.
    assert_eq!(
        drawn(&screen, "three"),
        vec![
            "view.ambient.clock",
            "view.ambient.weather",
            "view.x.pinurg"
        ],
        "the latest stage that applies decides"
    );
    // The apps that lost their tile here are still present, and the window is untouched:
    // only the DOCK of this output is shorter (R-23-6).
    assert!(
        screen.holds("three.view.x.other/c.other"),
        "a cut tile does not close its window"
    );
}
