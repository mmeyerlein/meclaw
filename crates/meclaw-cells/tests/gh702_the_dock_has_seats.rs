//! display-hive.md § 4.28–§ 4.30: the seats of the dock, and what a cut does to them.
//!
//! The dock is a column filled from the BOTTOM: seat tiles first, by `seat_ord`
//! ascending (the clock's 0 at the very bottom, the weather's 10 above it), the other
//! tiles above them by rank. The object tree is anchored the same way, so a HIGH `ord`
//! stands LOW on the screen -- the one thing a sketch gets backwards.
//!
//! § 4.29: a seat guarantees nothing. When its tile is missing its place stays visibly
//! empty -- empty space, a `display-seat` with no content, so nothing slides into the
//! gap -- and an empty seat does not count against `dock_max` (Q-19). The seat is known
//! as long as the view stands in the store, present or not.
//!
//! § 4.30: `dock_max` is the maximum number of drawn tiles per output, WITHOUT
//! exception. Seats fall last of all -- after the other tiles, after the urgent ones,
//! after the pinned ones -- but they do fall.

mod support;

use meclaw_core::serde_json::{Value, json};
use support::{Screen, component_view, library_ships, pane, window_id};

fn params(dock_max: u64) -> Value {
    json!({"screens": {"monitor": {"display_type": "monitor", "inputs": ["pointer"],
                                   "dock_max": dock_max}},
           "default_screen": "monitor"})
}

fn seat(view_id: &str, seat_ord: &str, props: Value) -> Value {
    let mut props = props;
    props["seat"] = json!("bottom");
    props["seat_ord"] = json!(seat_ord);
    component_view(view_id, "main", pane(view_id, props))
}

fn tile_of(exit: &str, oid: &str) -> String {
    format!("{exit}.display.dock/tile.{}", oid.replace('/', "~"))
}

/// Everything one output hangs in its dock, bottom -> top: `(component, oid, ord)`.
/// A `display-seat` carries no window, so its own id stands in for one.
fn column(screen: &Screen, exit: &str) -> Vec<(String, String, i64)> {
    let prefix = format!("{exit}.display.dock/");
    let mut out: Vec<(String, String, i64)> = screen
        .held
        .as_array()
        .expect("the display holds a list")
        .iter()
        .filter(|o| o["id"].as_str().unwrap_or("").starts_with(&prefix))
        .map(|o| {
            let id = o["id"].as_str().unwrap_or("");
            (
                o["component"].as_str().unwrap_or("").to_string(),
                o["props"]["oid"].as_str().unwrap_or(id).to_string(),
                o["ord"].as_i64().unwrap_or(0),
            )
        })
        .collect();
    out.sort_by_key(|t| -t.2);
    out
}

/// The clock sits at the bottom, the weather above it, every ranked tile above both
/// (§ 4.28).
#[test]
fn the_seats_are_at_the_bottom_in_their_own_order() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(params(8));
    screen.write(
        seat(
            "clock",
            "0",
            json!({"context": "ambient", "relevance": "0.2", "pinned": true}),
        ),
        1000,
    );
    screen.write(
        seat(
            "sky",
            "10",
            json!({"context": "ambient", "relevance": "0.2", "pinned": true}),
        ),
        1000,
    );
    screen.write(
        component_view(
            "talk",
            "main",
            pane(
                "talk",
                json!({"context": "conversation", "relevance": "0.9"}),
            ),
        ),
        1000,
    );

    let seen = column(&screen, "monitor");
    assert_eq!(
        seen.iter().map(|t| t.1.clone()).collect::<Vec<_>>(),
        vec![
            window_id("alex", "clock"),
            window_id("alex", "sky"),
            window_id("alex", "talk"),
        ],
        "bottom -> top: clock, weather, then what is ranked: {seen:?}"
    );
    let clock = screen
        .props(&tile_of("monitor", &window_id("alex", "clock")))
        .expect("the clock has a tile");
    assert_eq!(clock["seat"], "1", "the tile says it is seated: {clock}");
    assert_eq!(
        clock["oid"],
        json!(window_id("alex", "clock")),
        "and names the window a tap would carry: {clock}"
    );
}

/// The weather stops being present: its seat stays as an object of its own, the clock
/// does not move up into it, and the empty seat is not a drawn tile (Q-19, § 4.29).
#[test]
fn an_empty_seat_is_an_object_of_its_own() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(params(8));
    screen.write(
        seat(
            "clock",
            "0",
            json!({"context": "ambient", "relevance": "0.2", "pinned": true}),
        ),
        1000,
    );
    screen.write(
        seat(
            "sky",
            "10",
            json!({"context": "ambient", "relevance": "0.2"}),
        ),
        1000,
    );
    let sky = window_id("alex", "sky");
    assert!(
        screen.holds(&tile_of("monitor", &sky)),
        "both stand at first"
    );

    // The weather is not pinned: its decay runs out, it plays its leaving pass, and the
    // pass after that it is present no more -- while its view stands in the store, so
    // its seat is still known (§ 4.29).
    screen.pass(json!({"kind": "stroke"}), 1000 + 141_000);
    screen.pass(json!({"kind": "stroke"}), 1000 + 142_000);
    assert!(
        !screen.curator(&sky, "present").as_bool().unwrap_or(true),
        "the weather is gone from the canvas and the dock"
    );
    assert!(
        !screen.holds(&tile_of("monitor", &sky)),
        "so its tile is gone"
    );

    let empty = format!("{}~seat", tile_of("monitor", &sky));
    let props = screen
        .props(&empty)
        .expect("the empty seat stands as a `display-seat`");
    assert_eq!(props["seat_ord"], "10", "at its own height: {props}");

    let seen = column(&screen, "monitor");
    assert_eq!(
        seen.iter()
            .map(|t| (t.0.clone(), t.1.clone()))
            .collect::<Vec<_>>(),
        vec![
            ("display-tile".to_string(), window_id("alex", "clock")),
            ("display-seat".to_string(), empty),
        ],
        "the clock keeps the bottom, the gap above it stays a gap: {seen:?}"
    );
    assert_eq!(
        screen.props("monitor.display.dock").expect("the dock")["count"],
        1,
        "an empty seat is no drawn tile and counts against no `dock_max` (§ 4.30)"
    );
}

/// § 4.30: the cut has no exception. The other tiles fall first, then the urgent ones,
/// then the pinned ones, and a seat only when nothing else is left.
#[test]
fn a_seat_is_the_last_tile_to_fall_from_a_full_dock() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(params(1));
    screen.write(
        seat(
            "clock",
            "0",
            json!({"context": "ambient", "relevance": "0.1"}),
        ),
        1000,
    );
    screen.write(
        component_view(
            "pin",
            "main",
            pane(
                "pin",
                json!({"context": "ambient", "relevance": "0.1", "pinned": true}),
            ),
        ),
        1000,
    );
    screen.write(
        component_view(
            "ring",
            "main",
            pane("ring", json!({"context": "system", "state": "urgent"})),
        ),
        1000,
    );
    screen.write(
        component_view(
            "talk",
            "main",
            pane(
                "talk",
                json!({"context": "conversation", "relevance": "0.9"}),
            ),
        ),
        1000,
    );
    let seen = column(&screen, "monitor");
    assert_eq!(
        seen.iter().map(|t| t.1.clone()).collect::<Vec<_>>(),
        vec![window_id("alex", "clock")],
        "one tile fits, and the seat is the one that stays: {seen:?}"
    );
    // The windows themselves are untouched by the cut (§ 4.30, R-24-2): the urgent one
    // stands in front on every output, with or without a tile here.
    assert_eq!(
        screen.curator(&window_id("alex", "ring"), "level"),
        json!(3)
    );

    // And the seat is no exception either: with two seats and room for one, the higher
    // `seat_ord` falls.
    let mut tight = Screen::new(params(1));
    tight.write(
        seat(
            "clock",
            "0",
            json!({"context": "ambient", "relevance": "0.1"}),
        ),
        1000,
    );
    tight.write(
        seat(
            "sky",
            "10",
            json!({"context": "ambient", "relevance": "0.1"}),
        ),
        1000,
    );
    let seen = column(&tight, "monitor");
    // `dock_max` counts DRAWN tiles (§ 4.30): the seat of the window that fell leaves an
    // empty seat behind (§ 4.29), and an empty seat is not a drawn tile.
    assert_eq!(
        seen.iter()
            .filter(|t| t.0 == "display-tile")
            .map(|t| t.1.clone())
            .collect::<Vec<_>>(),
        vec![window_id("alex", "clock")],
        "`dock_max` holds without exception (§ 4.30): {seen:?}"
    );
    assert!(
        seen.iter().any(|t| t.0 == "display-seat"),
        "the cut seat left its place empty (§ 4.30 -> § 4.29): {seen:?}"
    );
    assert!(
        tight
            .curator(&window_id("alex", "sky"), "present")
            .as_bool()
            .unwrap_or(false),
        "what the cut takes is the tile on this output, never the presence"
    );
}
