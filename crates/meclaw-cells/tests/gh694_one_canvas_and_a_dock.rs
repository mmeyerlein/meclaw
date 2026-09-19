//! One canvas and a dock (`display-hive.md` § 4.17-4.32): `aside` is drawn as the canvas,
//! an urgent window stands ABOVE the focus instead of replacing it, and what is not open
//! stands in the dock as a tile.
//!
//! Two sentences this file used to pin are struck. The canvas has no SLOTS any more
//! (`canvas_slots`): every canvas window whose score reaches the bar is `relevant` and
//! open, so losing the focus is not leaving the canvas -- what takes a window off the
//! canvas is its score falling under the bar (§ 4.21, § 4.22). And the curator's word is
//! `rung` with a `level` beside it; `state` is only what an application may say about its
//! own window (`urgent`/`hidden`, § 4.6), and the tile says `open` rather than
//! `on_canvas`.
//!
//! The script runs the way a `code` cell runs it: as a subprocess, one pass at a time,
//! with the state row and the held tree carried between the passes (`support::Screen`).

mod support;

use meclaw_core::serde_json::{Value, json};
use support::{Screen, component_view, library_ships, pane, pane_id, view_of, window_id};

/// One television, named and complete: `display_type` and `default_screen` are both
/// mandatory since § 4.7.
fn params() -> Value {
    json!({"screens": {"tv": {"display_type": "tv", "viewing_distance_m": 3.0}},
           "default_screen": "tv"})
}

/// The tile of a window, which the dock keys by the WINDOW's id (§ 2 Id).
fn tile_of(window: &str) -> String {
    format!("display.dock/tile.{window}")
}

fn note(view_id: &str, region: &str, props: Value) -> Value {
    component_view(view_id, region, pane(view_id, props))
}

/// A window with one keyed child, so a later pass can change the CHILD and nothing else.
fn clock(view_id: &str, face: &str, touched: Option<&str>) -> Value {
    let mut props = json!({"context": "ambient", "relevance": "0.3", "pinned": true,
                           "pane_id": view_id});
    if let Some(t) = touched {
        props["touched"] = json!(t);
    }
    view_of(
        "alex",
        view_id,
        "main",
        json!({"component": "display-pane", "props": props, "key": format!("c.{view_id}"),
               "children": [{"component": "display-clock", "key": "face",
                             "props": {"time": face}}]}),
    )
}

/// `aside` is accepted and drawn as the canvas (OR-D3): a window that named it
/// may take the focus like any other.
#[test]
fn a_window_in_aside_may_take_the_focus() {
    if !library_ships() {
        return;
    }
    let mut screen = Screen::new(params());
    screen.write(
        note(
            "p",
            "aside",
            json!({"context": "conversation", "relevance": "0.9"}),
        ),
        100_000,
    );
    // A window that is `fresh` does not take the focus in the pass it arrived in (§ 4.20).
    screen.pass(json!({"kind": "stroke"}), 101_000);
    let window = screen.props(&pane_id("p", "p")).expect("the window stands");
    assert_eq!(window["region"], "aside", "it named the second region");
    assert_eq!(window["rung"], "focus", "aside is canvas: {window}");
    assert_eq!(window["level"], "1", "and it is open on it: {window}");
}

/// An urgent window does not lock the focus (OR-D2): it stands ABOVE the focus on its
/// own level, and the focus keeps both its rung and its level (§ 4.23 -- there is no
/// stack, so nothing is taken away to be restored).
#[test]
fn urgent_stands_above_the_focus_instead_of_replacing_it() {
    if !library_ships() {
        return;
    }
    let mut screen = Screen::new(params());
    screen.write(
        note(
            "a",
            "main",
            json!({"context": "conversation", "relevance": "0.9"}),
        ),
        100_000,
    );
    screen.pass(json!({"kind": "stroke"}), 101_000);
    assert_eq!(screen.props(&pane_id("a", "a")).unwrap()["rung"], "focus");

    screen.write(
        note(
            "t",
            "main",
            json!({"context": "ambient", "state": "urgent"}),
        ),
        102_000,
    );
    let ringing = screen.props(&pane_id("t", "t")).expect("the timer rings");
    let answer = screen.props(&pane_id("a", "a")).expect("the answer stands");
    assert_eq!(ringing["rung"], "urgent", "the timer rings: {ringing}");
    assert_eq!(
        ringing["front"], "1",
        "and it is the front urgent: {ringing}"
    );
    assert_eq!(ringing["level"], "3", "on the level above: {ringing}");
    assert_eq!(
        answer["rung"], "focus",
        "and the answer keeps the canvas: {answer}"
    );
    assert_eq!(answer["level"], "1", "on the level it had: {answer}");
}

/// The empty screen: a clock and a weather window, both pinned, both under the bar. The
/// canvas is EMPTY -- pinned keeps a window PRESENT, it does not open it -- and the dock
/// holds their two tiles, each wearing its rung and saying that it is not open.
#[test]
fn the_empty_screen_is_an_empty_canvas_and_a_dock() {
    if !library_ships() {
        return;
    }
    let mut screen = Screen::new(params());
    screen.write(
        note(
            "clock",
            "main",
            json!({"context": "ambient", "relevance": "0.2", "pinned": true}),
        ),
        100_000,
    );
    screen.write(
        note(
            "weather",
            "main",
            json!({"context": "ambient", "relevance": "0.25", "pinned": true}),
        ),
        100_000,
    );
    screen.pass(json!({"kind": "stroke"}), 101_000);

    for view in ["clock", "weather"] {
        let window = screen.props(&pane_id(view, view)).expect("the window");
        assert_eq!(
            window["rung"], "ambient",
            "on the ladder, not open: {window}"
        );
        assert_eq!(window["level"], "0", "so not on the canvas: {window}");
        assert_eq!(window["pinned"], "1", "{window}");
        let tile = screen
            .props(&tile_of(&window_id("alex", view)))
            .expect("the tile");
        assert_eq!(tile["rung"], "ambient", "the tile wears the rung: {tile}");
        assert_eq!(tile["open"], "", "and says the window is not open: {tile}");
        assert_eq!(tile["pinned"], "1", "{tile}");
    }
    assert_eq!(
        screen.props("display.dock").expect("the dock")["count"],
        2,
        "two tiles"
    );
    // The bar is the screen state's, not the root's (§ 3.1): one number per member, in
    // the store, where two outputs cannot hold two of them.
    assert_eq!(screen.screen_state()["bar"], 0.3);
    assert!(
        screen.props("display.root").expect("the root")["focus"].is_null(),
        "the root carries no bar any more"
    );
}

/// Losing the focus is not leaving the canvas (§ 4.21): the window that was large stays
/// open as `relevant`. What takes it off the canvas is its SCORE falling under the bar
/// (§ 4.22) -- and then its tile stands and says it is not open.
#[test]
fn a_window_leaves_the_canvas_when_its_score_falls_under_the_bar() {
    if !library_ships() {
        return;
    }
    let mut screen = Screen::new(params());
    let a = json!({"context": "conversation", "relevance": "0.9"});
    let b = json!({"context": "conversation", "relevance": "0.95"});
    screen.write(note("a", "main", a), 100_000);
    screen.pass(json!({"kind": "stroke"}), 101_000);
    assert_eq!(
        screen.props(&pane_id("a", "a")).unwrap()["rung"],
        "focus",
        "a stands large first"
    );

    screen.write(note("b", "main", b), 102_000);
    screen.pass(json!({"kind": "stroke"}), 103_000);
    let a_now = screen.props(&pane_id("a", "a")).expect("a stands");
    assert_eq!(a_now["rung"], "relevant", "b took the focus: {a_now}");
    assert_eq!(
        a_now["level"], "1",
        "and a is STILL on the canvas -- there are no slots to be pushed out of: {a_now}"
    );
    assert_eq!(screen.props(&pane_id("b", "b")).unwrap()["rung"], "focus");
    assert_eq!(
        screen
            .props(&tile_of(&window_id("alex", "a")))
            .expect("a's tile")["open"],
        "1",
        "an open window says so on its tile too (§ 6.10)"
    );

    // Two minutes on: the decay of § 4.15 has eaten both scores, and the canvas empties
    // itself without anybody deciding anything.
    screen.pass(json!({"kind": "stroke"}), 230_000);
    for view in ["a", "b"] {
        let window = screen.props(&pane_id(view, view)).expect("the window");
        assert_eq!(
            window["rung"], "ambient",
            "{view} fell under the bar: {window}"
        );
        assert_eq!(window["level"], "0", "and off the canvas: {window}");
        let tile = screen
            .props(&tile_of(&window_id("alex", view)))
            .expect("the tile");
        assert_eq!(tile["rung"], "ambient", "the tile wears the rung: {tile}");
        assert_eq!(tile["open"], "", "{tile}");
    }
}

/// A pinned window that rewrites its CHILDREN is no touch (§ 4.8, § 9.3): the clock's
/// minute and a weather measurement redraw the tile and leave the screen where it was.
/// `since` is what says so -- it is the whole record of the last touch (§ 4.9).
#[test]
fn a_pinned_window_that_rewrites_its_children_does_not_wake_the_screen() {
    if !library_ships() {
        return;
    }
    let mut screen = Screen::new(params());
    screen.write(clock("clock", "12:00", None), 100_000);
    screen.pass(json!({"kind": "stroke"}), 101_000);
    let oid = pane_id("clock", "clock");
    assert_eq!(screen.props(&oid).unwrap()["since"], "100000");

    // Thirty seconds on: the clock ticked.
    screen.write(clock("clock", "12:01", None), 132_000);
    let window = screen.props(&oid).expect("the window");
    assert_eq!(
        window["since"], "100000",
        "the minute is a child change and no touch: {window}"
    );
    assert_eq!(window["rung"], "ambient", "so nothing opened: {window}");
    assert_eq!(window["level"], "0", "{window}");
}

/// The `touched` hint still wakes the same window (§ 4.8 c): an application that says so
/// is asking for attention even though no own prop of the window changed, and the touch
/// restarts the decay from `now`.
#[test]
fn the_touched_hint_still_wakes_a_pinned_window() {
    if !library_ships() {
        return;
    }
    let mut screen = Screen::new(params());
    screen.write(clock("clock", "12:00", None), 100_000);
    screen.pass(json!({"kind": "stroke"}), 101_000);
    let oid = pane_id("clock", "clock");
    assert_eq!(
        screen.props(&oid).unwrap()["since"],
        "100000",
        "arrival alone is the only touch so far"
    );

    // The same face, the same own props -- only the hint is younger than the last one.
    screen.write(clock("clock", "12:00", Some("133000")), 133_000);
    let window = screen.props(&oid).expect("the window");
    assert_eq!(
        window["since"], "133000",
        "the hint is the touch (§ 4.8 c): {window}"
    );
    // § 4.9: an app touch writes `since` and NOTHING else -- it does not lead the window
    // above its score the way a finger does (§ 4.19).
    assert_eq!(
        window["led"], "",
        "and no app touch leads a window: {window}"
    );
}
