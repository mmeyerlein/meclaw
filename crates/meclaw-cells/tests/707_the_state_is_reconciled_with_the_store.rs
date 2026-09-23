//! OR-H0.9: the reference model assumes that passes run one after the other, and the cell
//! makes that promise for it.
//!
//! Until display 2.7.0 a write and its read pass were four messages apart, and a second
//! write that landed before the first pass's state row was back in the store was handed
//! the SAME state row -- measured on the minimal colony (H5): the ambient app sent three
//! views in one tick and the first two were gone for ever. The cure was `reconcile`: the
//! store's rows are the truth about what the apps have said, and every row the state does
//! not know is replayed as the `app_write` it was, at the moment the store wrote it.
//!
//! Since GH #809 the curator runs `resident` and every event is a pass over the ONE state
//! in its memory, so that race is gone: three writes in one breath are three passes in
//! order. `reconcile` still does the same work, now at the BOOT: a new cell reads the
//! store once and replays every row it finds. This file holds both halves -- the order of
//! a living cell, and the replay of a new one.

mod support;

use meclaw_core::serde_json::{Value, json};
use support::{Screen, library_ships, pane, view_of, window_id};

fn params(judge: &str) -> Value {
    json!({"judge": judge, "judge_min_interval_ms": 3000,
           "screens": {"monitor": {"display_type": "monitor", "inputs": ["pointer"]}},
           "default_screen": "monitor"})
}

/// One view of the ambient app, as it writes three of them in one tick.
fn ambient(view_id: &str, at: u64) -> Value {
    let mut row = view_of(
        "ambient",
        view_id,
        "main",
        pane(
            view_id,
            json!({"title": view_id, "context": "ambient", "relevance": "0.9",
                   "topic": format!("t:{view_id}")}),
        ),
    );
    row["updated_at"] = json!(at);
    row
}

fn window_of(view_id: &str) -> String {
    format!("{}/c.{view_id}", window_id("ambient", view_id))
}

fn tile_of(view_id: &str) -> String {
    format!(
        "monitor.display.dock/tile.{}",
        window_id("ambient", view_id)
    )
}

fn age(screen: &Screen, view_id: &str) -> Value {
    screen.curator(&window_id("ambient", view_id), "age")
}

#[test]
fn two_writes_in_one_breath_lose_no_window() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // THE OLD RACE, as it arrives now: the second write lands while the store's answer to
    // the first is still in the air. The replies are held back, so both writes are taken
    // before either bundle is acknowledged -- and neither pass waits for the store.
    let mut screen = Screen::new(params("off"));
    screen.write(ambient("clock", 1000), 1000);
    screen.hold(true);
    screen.write(ambient("weather", 1100), 1100);
    screen.hold(false);
    screen.flush();

    let held = screen.screen_state();
    assert!(
        held["views"][window_id("ambient", "clock")].is_object(),
        "the clock stands in the state: {held}"
    );
    assert!(
        held["views"][window_id("ambient", "weather")].is_object(),
        "and so does the weather: {held}"
    );
    assert!(
        screen.holds(&window_of("clock")) && screen.holds(&window_of("weather")),
        "both windows are drawn"
    );
    assert!(
        screen.holds(&tile_of("clock")) && screen.holds(&tile_of("weather")),
        "and both tiles"
    );
}

#[test]
fn a_new_cell_replays_every_row_the_store_holds() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // Two rows stand in the store before the cell's first message -- written while it was
    // down. The boot reads them ONCE and replays each as the `app_write` it was, at the
    // moment the store wrote it: `since` is the write's own moment, not the boot's.
    let mut screen = Screen::new(params("off"));
    screen.put(ambient("clock", 1000));
    screen.put(ambient("weather", 1100));
    screen.pass(json!({"kind": "stroke"}), 2000);

    let routes: Vec<&str> = screen
        .hops()
        .iter()
        .map(|h| h["route"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        routes.iter().filter(|r| **r == "read").count(),
        1,
        "one read of the tree: {routes:?}"
    );
    assert_eq!(
        routes.first(),
        Some(&"views"),
        "the store is read first: {routes:?}"
    );
    assert_eq!(
        screen.curator(&window_id("ambient", "clock"), "since"),
        json!(1000)
    );
    assert_eq!(
        screen.curator(&window_id("ambient", "weather"), "since"),
        json!(1100)
    );
    assert!(
        screen.holds(&window_of("clock")) && screen.holds(&window_of("weather")),
        "both windows are drawn"
    );
    // What a replay cannot give back is the fly-in: the pass in which a window was
    // `fresh` is over (OR-D19).
    assert_eq!(age(&screen, "clock"), "settled");
    assert_eq!(age(&screen, "weather"), "settled");
}

#[test]
fn a_withdrawn_view_leaves_before_it_goes() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // A withdrawal is marked `withdrawn` rather than taken out at once, so § 4.35 still
    // gives the sheet one frame to fade the window out instead of taking it away between
    // two renders.
    let mut screen = Screen::new(params("off"));
    screen.write(ambient("clock", 1000), 1000);
    screen.write(ambient("weather", 1100), 1100);
    screen.take_down("ambient", "clock", 2000);

    assert_eq!(age(&screen, "clock"), "leaving", "one last frame (§ 4.35)");
    assert!(
        screen.holds(&window_of("clock")),
        "and it is still drawn while it leaves"
    );

    screen.pass(json!({"kind": "stroke"}), 3000);
    assert!(
        screen.screen_state()["views"][window_id("ambient", "clock")].is_null(),
        "then it is gone from the state"
    );
    assert!(!screen.holds(&window_of("clock")), "and from the screen");
    assert!(
        screen.holds(&window_of("weather")),
        "the weather is untouched by any of it"
    );
}

#[test]
fn a_verdict_that_comes_back_between_two_writes_loses_no_window() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // A verdict travels the same road and takes the same time as a write, and a verdict
    // that dropped a window would drop it for ever, because the judge decides what is OPEN
    // and never what exists (§ 4.11, Leitlinie). It was asked two writes ago and answers
    // after the third: its pass runs on the state that holds all three.
    let mut screen = Screen::new(params("on"));
    screen.write(ambient("clock", 1000), 1000);
    screen.write(ambient("weather", 1100), 1100);
    screen.write(ambient("timer", 1200), 1200);
    screen.pass(
        json!({"kind": "verdict", "bar": 0.4, "weights": {"ambient": 0.8},
               "windows": {window_id("ambient", "clock"): {"judged_hidden": true}}}),
        1300,
    );

    let held = screen.screen_state();
    for view_id in ["clock", "weather", "timer"] {
        assert!(
            held["views"][window_id("ambient", view_id)].is_object(),
            "{view_id} survived the verdict: {held}"
        );
        assert!(
            screen.holds(&tile_of(view_id)),
            "{view_id} keeps its tile -- the judge decides what is open, never what exists"
        );
    }
    assert_eq!(held["bar"], 0.4, "and the verdict itself was taken");
    assert_eq!(
        screen.curator(&window_id("ambient", "clock"), "rung"),
        "hidden"
    );
}
