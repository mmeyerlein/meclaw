//! How long a window keeps its full weight is the window's own word (display-hive.md
//! § 4.15).
//!
//! `linger_ms` is a dial of the screen, and the chat should stand longer than the
//! weather: that is a statement an application makes about itself. `linger_of()` is the
//! one place a window's decay gets its duration -- the hint if it said one, the dial
//! otherwise, capped at `linger_ms + fade_ms` so the clock still has a moment to strike
//! at (§ 7.5: a request with a cap, and the app does not learn whether it was capped).
//!
//! The hint travels as TEXT like every number an app sends (§ 3.3), and `0` reads as
//! empty -- as no word at all.
//!
//! The script runs the way a `code` cell runs it: as a subprocess, with the pass's
//! document on stdin.

mod support;

use meclaw_core::serde_json::{Value, json};
use support::{Screen, component_view, library_ships, pane, window_id};

/// The dock child of a window, keyed by the window's own id.
fn tile_of(window: &str) -> String {
    format!("display.dock/tile.{}", window.replace('/', "~"))
}

/// The `at` of the one order this pass placed.
fn ordered_at(screen: &Screen) -> String {
    screen
        .lane("due")
        .into_iter()
        .find(|e| e["op"] == "add")
        .and_then(|e| e["at"].as_str().map(str::to_string))
        .unwrap_or_else(|| panic!("the pass orders a strike: {:?}", screen.lane("due")))
}

fn window(view_id: &str, linger: Value) -> Value {
    let mut props = json!({"context": "ambient", "relevance": "0.5"});
    if !linger.is_null() {
        props["linger"] = linger;
    }
    component_view(view_id, "main", pane(view_id, props))
}

/// Two windows, one context, one touch each: the one that said `linger 90000` still
/// stands at full weight a minute later, the one that said nothing has started to fade.
#[test]
fn a_linger_of_its_own_holds_a_window_through_the_dial() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(json!({}));
    screen.write(window("chat", json!("90000")), 1000);
    screen.write(window("sky", Value::Null), 1000);
    // A minute on: the dial's linger (20 s) is long gone, the chat's is not.
    screen.pass(json!({"kind": "stroke"}), 61_000);
    assert_eq!(
        screen.curator(&window_id("alex", "chat"), "decay"),
        json!(1.0),
        "full weight is full weight: {}",
        screen.screen_state()
    );
    let sky = screen.curator(&window_id("alex", "sky"), "decay");
    assert!(
        sky.as_f64().unwrap_or(1.0) < 0.7,
        "the one without a word of its own is fading: {sky}"
    );
    // And the two numbers reach the sheet the same way: as text on the window (§ 3.3).
    assert_eq!(
        screen.props("tv.view.alex.chat/c.chat").expect("the chat")["score"],
        "0.25",
        "the chat scores whole"
    );
}

/// `0` is not a duration: a window that says `linger: "0"` said nothing, and the screen's
/// dial answers for it (§ 3.3).
#[test]
fn a_linger_of_zero_reads_as_no_word_at_all() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(json!({}));
    screen.write(window("z", json!("0")), 1000);
    // The pass after the arrival: a fresh window orders one second out whatever it says
    // (§ 4.34), so the window's own crossing is only visible from the second pass on.
    screen.pass(json!({"kind": "stroke"}), 2000);
    assert_eq!(
        ordered_at(&screen),
        "1970-01-01T00:00:21Z",
        "the dial's 20 s after the touch, not a linger of nothing"
    );
}

/// A linger longer than `linger_ms + fade_ms` would put the window's zero crossing beyond
/// any moment the clock orders, and the tile would never go. It is capped (§ 4.15).
#[test]
fn a_linger_is_capped_at_the_dial_plus_the_fade() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(json!({}));
    screen.write(window("forever", json!("999999999")), 1000);
    screen.pass(json!({"kind": "stroke"}), 2000);
    // Capped at 20 s + 120 s, so the linger ends at 141 s. Uncapped the order would be
    // eleven days out, which is what tells the two apart.
    assert_eq!(
        ordered_at(&screen),
        "1970-01-01T00:02:21Z",
        "the clock strikes at the capped crossing"
    );

    let tile = tile_of(&window_id("alex", "forever"));
    assert!(screen.holds(&tile), "the tile stands while the window does");
    // The cap holds the LINGER; the fade then runs its own 120 s, so the score reaches
    // zero at 1000 + 140 s + 120 s. The pass after that is the window's leaving pass
    // (§ 4.35), and the one after that is where it is gone.
    screen.pass(json!({"kind": "stroke"}), 1000 + 140_000 + 120_000 + 1);
    assert_eq!(
        screen.curator(&window_id("alex", "forever"), "age"),
        json!("leaving"),
        "one last pass so the sheet can fade it out"
    );
    screen.pass(json!({"kind": "stroke"}), 1000 + 140_000 + 122_000);
    assert!(
        !screen.holds(&tile),
        "and then the tile is gone: {:?}",
        screen.held
    );
}

/// The clock strikes at the crossings the SCORE will make, so a window with its own
/// linger orders its own moment -- the dial's one would be too early and the window would
/// stand there with nothing to change.
#[test]
fn the_clock_orders_the_moment_the_window_will_actually_reach() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(json!({}));
    screen.write(window("chat", json!("90000")), 1000);
    // The arrival orders one second out, because a fresh window is settled in the next
    // pass and competes only from then on (§ 4.34, § 4.35).
    assert_eq!(ordered_at(&screen), "1970-01-01T00:00:02Z");
    screen.pass(json!({"kind": "stroke"}), 2000);
    // The moment is what carries the claim: with its own 90 s linger the decay leaves 1
    // at 91 s, with the screen's 20 s dial it would be 21 s.
    assert_eq!(
        ordered_at(&screen),
        "1970-01-01T00:01:31Z",
        "the clock strikes at the window's own crossing, not the dial's"
    );
    // And at 81 s the dial's window would long have started to fade; this one has not.
    screen.pass(json!({"kind": "stroke"}), 81_000);
    assert_eq!(
        screen.curator(&window_id("alex", "chat"), "decay"),
        json!(1.0),
        "still inside its own linger"
    );
}
