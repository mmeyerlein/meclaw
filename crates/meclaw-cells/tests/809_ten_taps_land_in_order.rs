//! GH #809 -- ten taps land in order (display-hive.md § 4.1, § 5.8, S-024).
//!
//! § 5.8: every tap acts on the state the tap before it left. A tap on the tile of a
//! window that is not open opens it (§ 5.1), a tap on an open one puts it away (§ 5.2), so
//! an even count of taps on a closed window ends put away.
//!
//! Up to display 2.6.x that was the hard case: the curator kept its state in a store row
//! and had no memory between two messages, so two taps inside one round trip were handed
//! the SAME row and the second one opened the window a second time (measured on a twin,
//! GH #744: ten clicks, two of them 36 ms apart, and the window stood open). Since display
//! 2.7.0 the cell runs `resident` and the state lives in its memory: each message runs its
//! pass on what the message before it left, whatever the store and the display have
//! answered by then. There is no row to read back, no condition on a write and no repeat.
//!
//! So this file holds § 5.8 three ways: taps one after another, taps whose store and
//! display replies are all still in the air, and taps that reach a cell still waking up.
//!
//! The script runs the way a `resident` code cell runs it: one living cell over every
//! message (`support::Screen`, the curator driver).

mod support;

use meclaw_core::serde_json::{Value, json};
use support::{Screen, component_view, library_ships, pane, window_id};

/// Relevance 0.5 against the default bar of 0.3 is a score of 0.25: the window is present
/// with a tile and is NOT open, so the first tap has something to open.
fn scene() -> Value {
    component_view(
        "a",
        "main",
        pane("a", json!({"context": "work", "relevance": "0.5"})),
    )
}

/// The patch hops of the last message.
fn patches(screen: &Screen) -> usize {
    screen
        .hops()
        .iter()
        .filter(|h| h["route"] == "patch")
        .count()
}

/// Where tap `i` (from 0) at `now` leaves the window, `step_ms` after the tap before it.
fn assert_after_tap(screen: &Screen, window: &str, i: u64, now: u64, step_ms: u64) {
    if i.is_multiple_of(2) {
        assert_eq!(
            screen.curator(window, "open"),
            json!(true),
            "tap {i} found a closed window and opened it (§ 5.1)"
        );
        assert_eq!(screen.curator(window, "since"), json!(now), "{i}");
        assert_eq!(
            screen.curator(window, "led_until"),
            json!(now + 20_000),
            "and the finger holds it up for the linger (§ 4.19): {i}"
        );
        assert_eq!(screen.curator(window, "dismissed_at"), json!(0), "{i}");
    } else {
        assert_eq!(
            screen.curator(window, "open"),
            json!(false),
            "tap {i} found an open window and put it away (§ 5.2)"
        );
        assert_eq!(screen.curator(window, "dismissed_at"), json!(now), "{i}");
        assert_eq!(screen.curator(window, "led_until"), json!(0), "{i}");
        assert_eq!(
            screen.curator(window, "since"),
            json!(now - step_ms),
            "a put-away leaves `since` where it was (§ 5.2): {i}"
        );
    }
}

/// Ten taps a second apart, every reply delivered before the next tap.
#[test]
fn ten_taps_one_after_another_end_put_away() {
    if !library_ships() {
        return;
    }
    let mut screen = Screen::new(json!({}));
    screen.write(scene(), 1000);
    let window = window_id("alex", "a");
    assert_eq!(
        screen.curator(&window, "open"),
        json!(false),
        "not yet open"
    );

    for i in 0..10u64 {
        let now = 2000 + i * 1000;
        screen.pass(json!({"kind": "tap", "for": window.as_str()}), now);
        assert_after_tap(&screen, &window, i, now, 1000);
        assert_eq!(patches(&screen), 1, "one patch per pass: tap {i}");
        assert!(
            screen.holds("display.dock/tile.view.alex.a"),
            "the tile stands after tap {i}: a put-away keeps the rank (R-24-1)"
        );
    }
    assert_eq!(
        screen.curator(&window, "open"),
        json!(false),
        "ten taps on a closed window end put away (§ 5.8, S-024)"
    );
}

/// The shape GH #744 measured: the next tap arrives while the store and the display have
/// not answered the one before. The pass does not wait for them -- its state is in
/// memory -- so each tap still acts on the one before it.
#[test]
fn ten_taps_inside_one_round_trip_land_in_order() {
    if !library_ships() {
        return;
    }
    let mut screen = Screen::new(json!({}));
    screen.write(scene(), 1000);
    let window = window_id("alex", "a");

    screen.hold(true);
    for i in 0..10u64 {
        // 36 ms apart: the two clicks the twin measured, ten times over.
        let now = 2000 + i * 36;
        screen.pass(json!({"kind": "tap", "for": window.as_str()}), now);
        assert_after_tap(&screen, &window, i, now, 36);
        assert_eq!(
            patches(&screen),
            1,
            "and it draws at once, without waiting for any reply: tap {i}"
        );
    }
    let late = screen.flush();
    screen.hold(false);
    assert!(
        late.is_empty(),
        "the replies arriving late draw nothing: an acknowledgement is not answered \
         (GH #161): {late:?}"
    );
    assert_eq!(
        screen.curator(&window, "open"),
        json!(false),
        "an even count ends put away, whatever the replies did (§ 5.8, S-024)"
    );
}

/// Ten taps reaching a cell that has just started: it has no rows and no tree yet, so
/// every tap is queued behind its boot (one select, one read), and then each one runs as
/// its own pass, in the order it came, and ONE patch draws them all.
#[test]
fn ten_taps_on_a_waking_cell_run_in_the_order_they_came() {
    if !library_ships() {
        return;
    }
    let mut screen = Screen::new(json!({}));
    // The row the app wrote before the cell started: a prior state, not a write.
    let mut row = scene();
    row["updated_at"] = json!(1000);
    screen.put(row);
    let window = window_id("alex", "a");

    screen.hold(true);
    for i in 0..10u64 {
        screen.pass(
            json!({"kind": "tap", "for": window.as_str()}),
            2000 + i * 36,
        );
        assert!(
            screen.patch().is_empty(),
            "a cell that has not read its store draws nothing: tap {i}"
        );
    }
    screen.flush();
    assert_eq!(
        patches(&screen),
        1,
        "the boot draws the ten passes in ONE patch: {:?}",
        screen.hops()
    );
    screen.hold(false);
    assert_eq!(
        screen.curator(&window, "open"),
        json!(false),
        "and they ran in order: an even count ends put away (§ 5.8, S-024)"
    );
    assert_eq!(
        screen.curator(&window, "dismissed_at"),
        json!(2000 + 9 * 36),
        "put away by the tenth tap"
    );
    assert_eq!(
        screen.curator(&window, "since"),
        json!(2000 + 8 * 36),
        "opened by the ninth"
    );
}
