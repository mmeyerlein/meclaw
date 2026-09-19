//! display-hive.md § 4.11 and § 4.22: the dock shows what is PRESENT, the rung decides
//! only what is OPEN. Two axes, and they are independent -- a window a zero rule pushes
//! off the canvas keeps its tile, and a window that has faded keeps nothing.
//!
//! Presence ends by decay, by `ttl_ms`, by `topic_dupe` or by the app withdrawing the
//! view -- never by a verdict (§ 4.11; Leitlinie; Ruling 14.09.). `pinned` and
//! `relevant_until` are the two ways an app keeps a tile standing through the fade.
//!
//! The tile ids are derived from the WINDOW id of § 2 (`view.<owner>.<view_id>`), and
//! every output carries its own copy of the whole tree under its name (§ 6.2).

mod support;

use meclaw_core::serde_json::{Value, json};
use support::{Screen, component_view, library_ships, pane, window_id};

/// One exit, so what an output draws and what the state holds differ in nothing but the
/// prefix. `dock_max` is high enough that no cut is in the way (§ 4.30 is another lock).
fn params() -> Value {
    json!({"screens": {"monitor": {"display_type": "monitor",
                                   "inputs": ["pointer", "keyboard"], "dock_max": 8}},
           "default_screen": "monitor"})
}

fn note(view_id: &str, props: Value) -> Value {
    component_view(view_id, "main", pane(view_id, props))
}

/// The id of a window's tile on one output. A window id carries a slash, and the dock
/// writes it as a tilde so the tile is a child of the dock and not of the window.
fn tile_of(exit: &str, oid: &str) -> String {
    format!("{exit}.display.dock/tile.{}", oid.replace('/', "~"))
}

/// The tiles one output draws, bottom -> top. The dock is anchored at the bottom edge,
/// so the HIGHEST `ord` stands lowest on the screen (§ 4.26, § 4.28).
fn tiles(screen: &Screen, exit: &str) -> Vec<(String, i64)> {
    let prefix = format!("{exit}.display.dock/");
    let mut out: Vec<(String, i64)> = screen
        .held
        .as_array()
        .expect("the display holds a list")
        .iter()
        .filter(|o| {
            o["component"] == "display-tile" && o["id"].as_str().unwrap_or("").starts_with(&prefix)
        })
        .map(|o| {
            (
                o["props"]["oid"].as_str().unwrap_or("").to_string(),
                o["ord"].as_i64().unwrap_or(0),
            )
        })
        .collect();
    out.sort_by_key(|t| -t.1);
    out
}

/// Presence alone puts a tile in the dock, and the rank orders the column (§ 4.27).
#[test]
fn the_dock_carries_one_tile_per_present_window() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(params());
    screen.write(
        note("a", json!({"context": "conversation", "relevance": "0.9"})),
        1000,
    );
    screen.write(
        note("b", json!({"context": "ambient", "relevance": "0.3"})),
        1000,
    );

    let drawn = tiles(&screen, "monitor");
    assert_eq!(
        drawn.iter().map(|t| t.0.clone()).collect::<Vec<_>>(),
        vec![window_id("alex", "b"), window_id("alex", "a")],
        "bottom -> top: the highest rank stands on top (§ 4.28): {drawn:?}"
    );
    assert_eq!(
        screen
            .props("monitor.display.dock")
            .expect("the dock stands")["count"],
        2,
        "the dock counts the tiles it draws"
    );

    let tile = screen
        .props(&tile_of("monitor", &window_id("alex", "a")))
        .expect("a present window has a tile");
    // Two names, two jobs (§ 5.6, § 7.2): `oid` is the window id a tap carries back,
    // `for` is the DOM id of the window element -- its `pane_id` -- which is what the
    // client matches the two halves of one object on.
    assert_eq!(tile["oid"], json!(window_id("alex", "a")));
    assert_eq!(tile["for"], json!("a"));
    assert!(
        tile["glyph"].as_str().is_some_and(|g| !g.is_empty()),
        "and it carries a glyph, its own or the fallback (§ 7.2): {tile}"
    );
    let rank = |oid: &str| -> f64 { screen.curator(oid, "rank").as_f64().unwrap_or(0.0) };
    assert!(
        rank(&window_id("alex", "a")) > rank(&window_id("alex", "b")),
        "the rank is what the order reads (§ 4.27)"
    );
}

/// A zero rule of § 4.14 takes the window off the canvas and nothing else: rung `hidden`,
/// level 0, and the tile stands with a rank of its own (§ 4.22, § 4.27).
#[test]
fn a_hidden_window_keeps_its_tile() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(params());
    screen.write(
        note(
            "a",
            json!({"context": "ambient", "relevance": "0.9", "state": "hidden"}),
        ),
        1000,
    );
    let oid = window_id("alex", "a");
    assert_eq!(screen.curator(&oid, "rung"), json!("hidden"));
    assert_eq!(screen.curator(&oid, "score"), json!(0.0), "a zero rule");
    assert_eq!(screen.curator(&oid, "level"), json!(0), "and not open");
    assert!(screen.curator(&oid, "present").as_bool().unwrap_or(false));

    let tile = screen
        .props(&tile_of("monitor", &oid))
        .expect("the tile stands whatever the canvas says");
    assert_eq!(tile["rung"], "hidden", "the tile wears the rung: {tile}");
    assert_eq!(tile["open"], "", "§ 6.10: it is not open: {tile}");
    assert!(
        tile["rank"]
            .as_str()
            .unwrap_or("0")
            .parse::<f64>()
            .unwrap_or(0.0)
            > 0.0,
        "the rank knows no zero rule (§ 4.27): {tile}"
    );
}

/// Decay ends the presence -- but not in the same breath: the window stands one last
/// pass as `leaving` so the sheet can fade it, and is gone the pass after (§ 4.35).
#[test]
fn a_faded_window_leaves_and_then_loses_its_tile() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(params());
    screen.write(
        note("a", json!({"context": "ambient", "relevance": "0.5"})),
        1000,
    );
    let oid = window_id("alex", "a");
    let tile = tile_of("monitor", &oid);

    // linger 20 s + fade 120 s + one second: the decay is spent.
    screen.pass(json!({"kind": "stroke"}), 1000 + 141_000);
    assert_eq!(screen.curator(&oid, "age"), json!("leaving"));
    assert!(screen.holds(&tile), "the leaving pass still draws it");

    let calls = screen.pass(json!({"kind": "stroke"}), 1000 + 142_000);
    assert!(!screen.holds(&tile), "and then the tile is gone");
    assert!(
        calls
            .iter()
            .any(|c| c["op"] == "object.delete" && c["id"] == tile.as_str()),
        "swept, not merely overwritten: {calls:?}"
    );
    assert!(
        !screen.curator(&oid, "present").as_bool().unwrap_or(true),
        "presence ended by decay (§ 4.11)"
    );
}

/// `pinned` keeps the presence, so the tile stands through any fade (§ 4.11, § 7.6).
#[test]
fn a_pinned_window_stays_in_the_dock() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(params());
    screen.write(
        note(
            "a",
            json!({"context": "ambient", "relevance": "0.5", "pinned": true}),
        ),
        1000,
    );
    screen.pass(json!({"kind": "stroke"}), 1000 + 600_000);
    let tile = screen
        .props(&tile_of("monitor", &window_id("alex", "a")))
        .expect("a pinned window keeps its tile");
    assert_eq!(tile["pinned"], "1", "and says so: {tile}");
    // R-24-3: the pin holds the presence, never the decay -- for score and rank the
    // decay runs as for any window, so ten minutes later the rank is spent.
    assert_eq!(
        tile["rank"], "0.0",
        "the decay ran on under the pin: {tile}"
    );
}

/// A window that names its own deadline is present until then, faded or not (§ 4.11),
/// and the curator orders a stroke for that moment (§ 4.34).
#[test]
fn a_window_with_a_deadline_is_present_until_it() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let faded = 1000 + 141_000; // linger 20 s + fade 120 s + one second.
    let deadline = faded + 60_000;
    let mut screen = Screen::new(params());
    screen.write(
        note(
            "t",
            json!({"context": "ambient", "relevance": "0.5",
                   "relevant_until": deadline.to_string()}),
        ),
        1000,
    );
    screen.pass(json!({"kind": "stroke"}), faded);

    let oid = window_id("alex", "t");
    assert!(
        screen.curator(&oid, "present").as_bool().unwrap_or(false),
        "the decay is spent and the deadline carries the presence"
    );
    let tile = screen
        .props(&tile_of("monitor", &oid))
        .expect("the tile stands until the deadline");
    assert_eq!(tile["open"], "", "quiet, not open: {tile}");
    // § 4.27 read literally: rank = w x relevance x decay, and the decay is 0 here.
    assert_eq!(tile["rank"], "0.0", "{tile}");

    let due: Vec<String> = screen
        .lane("due")
        .iter()
        .filter(|e| e["op"] == "add")
        .map(|e| e["at"].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(
        due,
        vec!["1970-01-01T00:03:22Z".to_string()],
        "the deadline is a moment the curator orders (§ 4.34): {due:?}"
    );
}
