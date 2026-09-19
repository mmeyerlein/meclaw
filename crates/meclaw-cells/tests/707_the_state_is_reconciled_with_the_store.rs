//! OR-H0.9: the reference model assumes that passes run one after the other, and the cell
//! makes that promise for it.
//!
//! A write and its read pass are four messages apart. When a second write lands before the
//! first pass's state row is back in the store, the second read pass is handed the SAME
//! state row the first one got -- and computes a state in which the first view was never
//! written. Measured on the minimal colony (H5): the ambient app sends three views in one
//! tick and the first two are gone, for ever, because no later event ever writes them
//! again.
//!
//! So `pass_read` reconciles first: the store's rows are the truth about what the apps have
//! said, and every row the state does not know yet is replayed as the `app_write` it was,
//! at the moment the store wrote it. Every view whose row is gone is marked `withdrawn`,
//! which leaves the leaving pass of § 4.35 to this pass -- the sheet keeps its one frame to
//! fade the window out.

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
    let mut screen = Screen::new(params("off"));
    screen.write(ambient("clock", 1000), 1000);
    assert!(screen.holds(&window_of("clock")), "the clock is drawn");

    // THE RACE. The second write lands before the first pass's state row reached the
    // store, so this pass is handed the state row of the pass BEFORE the clock -- here,
    // none at all. Without the reconciliation the clock is simply gone from here on.
    screen.state = Value::Null;
    screen.write(ambient("weather", 1100), 1100);

    let held = screen.screen_state();
    assert!(
        held["views"][window_id("ambient", "clock")].is_object(),
        "the clock is back in the state row: {held}"
    );
    assert!(
        held["views"][window_id("ambient", "weather")].is_object(),
        "and so is the weather: {held}"
    );
    assert!(
        screen.holds(&window_of("clock")) && screen.holds(&window_of("weather")),
        "both windows are drawn"
    );
    assert!(
        screen.holds(&tile_of("clock")) && screen.holds(&tile_of("weather")),
        "and both tiles"
    );
    // The replay is a repair of the pass that never ran, and it runs at the moment the
    // store recorded -- so `since` is the write's own moment, and `ttl_ms` would count
    // from there too.
    assert_eq!(
        screen.curator(&window_id("ambient", "clock"), "since"),
        json!(1000)
    );
    assert_eq!(
        screen.curator(&window_id("ambient", "weather"), "since"),
        json!(1100)
    );
    // What a replay cannot give back is the fly-in: the pass it repairs is the pass in
    // which the clock was `fresh`, and that pass is over.
    assert_eq!(age(&screen, "clock"), "settled");
    assert_eq!(age(&screen, "weather"), "fresh");
}

#[test]
fn a_view_whose_row_is_gone_leaves_before_it_goes() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // The mirror case: the state holds a view the store no longer has -- a withdrawal
    // whose own read pass was lost the same way. It is marked `withdrawn` rather than
    // replayed as a pass of its own, so § 4.35 still gives the sheet one frame to fade the
    // window out instead of taking it away between two renders.
    let mut screen = Screen::new(params("off"));
    screen.write(ambient("clock", 1000), 1000);
    screen.write(ambient("weather", 1100), 1100);
    screen.withdraw("ambient", "clock");

    screen.pass(json!({"kind": "stroke"}), 2000);
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
    // A verdict travels the same road and takes the same time, so it can be handed a stale
    // state row too -- and a verdict that dropped a window would drop it for ever, because
    // the judge decides what is OPEN and never what exists (§ 4.11, Leitlinie).
    let mut screen = Screen::new(params("on"));
    screen.write(ambient("clock", 1000), 1000);
    screen.write(ambient("weather", 1100), 1100);
    let before = screen.state.clone();
    screen.write(ambient("timer", 1200), 1200);

    // The verdict was asked one pass ago and answers now, carrying the state row of then.
    screen.state = before;
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
