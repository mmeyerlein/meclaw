//! display-hive.md § 4.29: a seat guarantees nothing. When its tile is missing, its place
//! stays visibly EMPTY -- empty space, no placeholder object; no seat tile moves into the
//! gap and no ranked tile slides into a seat (R-23-3). A seat is known as long as the view
//! stands in the store, also when the app is not present; when the app withdraws the view,
//! the seat goes with it.
//!
//! So the screen draws a `display-seat` there: an object with no content and
//! `aria-hidden`, whose whole job is to hold the gap open. Q-19, S-031, S-054.

mod support;

use meclaw_core::serde_json::{Value, json};
use support::{Screen, library_ships, pane, view_of};

fn params() -> Value {
    json!({"linger_ms": 20000, "fade_ms": 120000,
           "screens": {"monitor": {"display_type": "monitor", "inputs": ["pointer"]},
                       "tiny": {"display_type": "phone", "dock_max": 1}},
           "default_screen": "monitor"})
}

fn ambient(view_id: &str, props: Value) -> Value {
    view_of("ambient", view_id, "main", pane(view_id, props))
}

fn seat_object(screen: &Screen, exit: &str, oid: &str) -> Option<Value> {
    screen.props(&format!("{exit}.display.dock/tile.{oid}~seat"))
}

fn tile(screen: &Screen, exit: &str, oid: &str) -> Option<Value> {
    screen.props(&format!("{exit}.display.dock/tile.{oid}"))
}

#[test]
fn a_seat_whose_app_is_gone_is_drawn_as_empty_space() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(params());
    // The clock is NOT pinned: its presence ends by decay (§ 4.11). The weather is.
    screen.write(
        ambient(
            "clock",
            json!({"seat": "bottom", "seat_ord": "0", "topic": "clock",
                                "context": "ambient", "title": "Clock"}),
        ),
        1_000,
    );
    screen.write(
        ambient(
            "weather",
            json!({"seat": "bottom", "seat_ord": "10", "pinned": true,
                                  "topic": "weather:berlin", "context": "ambient",
                                  "title": "Weather"}),
        ),
        1_000,
    );
    assert!(tile(&screen, "monitor", "view.ambient.clock").is_some());

    // 150000: the clock's decay has run out; it stands one more pass as `leaving` (§ 4.35).
    screen.pass(json!({"kind": "stroke"}), 150_000);
    assert_eq!(screen.curator("view.ambient.clock", "age"), "leaving");
    assert!(
        tile(&screen, "monitor", "view.ambient.clock").is_some(),
        "still drawn"
    );

    // 151000: gone from the state. Its seat stays -- as empty space.
    screen.pass(json!({"kind": "stroke"}), 151_000);
    assert!(
        tile(&screen, "monitor", "view.ambient.clock").is_none(),
        "a window that is not present has no tile (§ 4.11)"
    );
    let seat = seat_object(&screen, "monitor", "view.ambient.clock")
        .expect("the empty seat is an object of its own");
    assert_eq!(seat["seat_ord"], "0");
    assert_eq!(
        seat.as_object().unwrap().len(),
        1,
        "empty space carries nothing else: {seat}"
    );
    // Nothing slid into the gap: the weather still stands above it.
    let weather = tile(&screen, "monitor", "view.ambient.weather").expect("the weather");
    assert_eq!(weather["seat"], "1");

    // § 4.30: an empty seat does not count against `dock_max` -- drawn tiles are counted.
    // On an output that draws ONE tile, the weather is still drawn beside the empty seat.
    assert!(seat_object(&screen, "tiny", "view.ambient.clock").is_some());
    assert!(tile(&screen, "tiny", "view.ambient.weather").is_some());

    // The withdrawal takes the seat with it (§ 4.29, step 4).
    screen.withdraw("ambient", "clock");
    screen.pass(
        json!({"kind": "app_withdraw", "oid": "view.ambient.clock"}),
        152_000,
    );
    assert!(
        seat_object(&screen, "monitor", "view.ambient.clock").is_none(),
        "when the app withdraws the view, the seat is gone too"
    );
}
