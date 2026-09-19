//! display-hive.md § 7.1–§ 7.2: an app sends ONE window with the child `tile`, and the
//! curator takes that child out of the window -- what an app says about itself in one
//! line belongs in the dock, not a second time inside the big window.
//!
//! § 7.2: when `tile` is missing the curator builds one itself: the glyph from
//! `context`, else from `class`, else a dot; the line from the window's title, else its
//! kicker, else the owner's name; at most 24 characters, cut with an ellipsis.
//!
//! § 4.30 and § 6.12: what the cut of one output takes is the TILE. The app stays
//! present and its window stays open if it is open (R-23-6, R-24-2) -- the cut is the
//! dock's business and never a statement about existence.

mod support;

use meclaw_core::serde_json::{Value, json};
use support::{Screen, component_view, library_ships, pane, window_id};

fn params(dock_max: u64) -> Value {
    json!({"screens": {"monitor": {"display_type": "monitor",
                                   "inputs": ["pointer", "keyboard"],
                                   "dock_max": dock_max}},
           "default_screen": "monitor"})
}

fn tile_of(exit: &str, oid: &str) -> String {
    format!("{exit}.display.dock/tile.{}", oid.replace('/', "~"))
}

/// The child keyed `tile` leaves the window's tree; every other child stays where the
/// app put it, and no `display-tile` stands anywhere but in the dock (§ 7.1).
#[test]
fn the_tile_is_taken_out_of_the_window() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let tree = json!({
        "component": "display-pane",
        "props": {"pane_id": "w", "context": "ambient", "relevance": "0.8"},
        "key": "c.w",
        "children": [
            {"component": "display-tile", "key": "tile",
             "props": {"glyph": "\u{2600}", "line": "Berlin", "value": "21",
                       "unit": "\u{00b0}"}},
            {"component": "display-value", "key": "v", "props": {"value": "21"}}
        ],
    });
    let mut screen = Screen::new(params(8));
    screen.write(component_view("w", "main", tree), 1000);
    let oid = window_id("alex", "w");

    assert!(
        screen.holds(&tile_of("monitor", &oid)),
        "the dock has a tile for the window"
    );
    assert!(
        !screen.holds(&format!("monitor.{oid}/c.w/0")),
        "and the window's tree does not: the tile was taken out (§ 7.1)"
    );
    // The sibling keeps its place. It is the tile that was taken out, nothing else.
    let sibling = screen
        .props(&format!("monitor.{oid}/c.w/v"))
        .expect("the sibling of the tile is drawn inside the window");
    assert_eq!(sibling["value"], "21", "{sibling}");

    let stray: Vec<String> = screen
        .held
        .as_array()
        .expect("a list")
        .iter()
        .filter(|o| {
            o["component"] == "display-tile"
                && !o["parent"].as_str().unwrap_or("").ends_with("display.dock")
        })
        .map(|o| o["id"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(
        stray.is_empty(),
        "no tile stands anywhere but in the dock: {stray:?}"
    );
}

/// A window that brought no tile gets one from the curator (§ 7.2).
#[test]
fn a_window_without_a_tile_falls_back() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(params(8));
    screen.write(
        component_view(
            "c",
            "main",
            pane(
                "c",
                json!({"context": "conversation", "relevance": "0.8",
                       "title": "A title that is very much longer than one tile line"}),
            ),
        ),
        1000,
    );
    let tile = screen
        .props(&tile_of("monitor", &window_id("alex", "c")))
        .expect("a tile");
    assert_eq!(
        tile["glyph"], "\u{1f4ac}",
        "the glyph comes from `context`: {tile}"
    );
    let line = tile["line"].as_str().expect("a line");
    assert_eq!(line.chars().count(), 24, "one line, cut: {line:?}");
    assert!(
        line.ends_with('\u{2026}'),
        "and the cut is visible: {line:?}"
    );
    assert_eq!(tile["value"], "", "without a value (§ 7.2): {tile}");
}

/// What the cut takes from an output is the tile, never the window (§ 4.30, § 6.12).
#[test]
fn a_cut_takes_the_tile_and_leaves_the_window() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(params(1));
    screen.write(
        component_view(
            "loud",
            "main",
            pane("loud", json!({"context": "work", "relevance": "0.9"})),
        ),
        1000,
    );
    screen.write(
        component_view(
            "quiet",
            "main",
            pane("quiet", json!({"context": "work", "relevance": "0.7"})),
        ),
        1000,
    );

    let loud = window_id("alex", "loud");
    let quiet = window_id("alex", "quiet");
    assert!(screen.holds(&tile_of("monitor", &loud)), "one tile fits");
    assert!(
        !screen.holds(&tile_of("monitor", &quiet)),
        "and the lower rank is the one that falls (§ 4.30)"
    );
    assert_eq!(
        screen.props("monitor.display.dock").expect("the dock")["count"],
        1,
        "`dock_max` is the maximum number of DRAWN tiles"
    );

    // And the window the cut dropped is untouched: present, open, drawn large.
    assert!(
        screen.curator(&quiet, "present").as_bool().unwrap_or(false),
        "the app stays present (R-23-6)"
    );
    assert_eq!(
        screen.curator(&quiet, "level"),
        json!(1),
        "and open (R-24-2)"
    );
    let window = screen
        .props(&format!("monitor.{quiet}/c.quiet"))
        .expect("the output draws the window whose tile it dropped");
    assert_eq!(window["level"], "1", "{window}");

    // The cut is the dock's business: nothing on the root counts what fell out.
    let root = screen.props("monitor.display.root").expect("the root");
    assert!(
        root.get("dock_overflow").is_none(),
        "no overflow number beside the dock (§ 4.30): {root:?}"
    );
}
