//! What a hand at full speed does to one tile and to the clock (display-hive.md § 5.8).
//!
//! Ten taps on a tile produce no error, and each one applies to the state the one before
//! it left: a tap on the tile of a window that is not open is a touch that opens it
//! (§ 5.1, § 4.19), a tap on the tile of an open one puts it away (§ 5.2). So a finger
//! drumming on one tile makes it open, closed, open, closed -- and the tile itself never
//! goes, because a put-away changes neither `since` nor the decay (R-24-1).
//!
//! The other half is the CLOCK. A tap is absorbed into a full read pass, and every read
//! pass orders the next stroke (§ 4.34), so a finger drives the timer as hard as it
//! drives the curator. Ordering and cancelling the same id in one pass is what made a
//! window flash and vanish once (GH #681, GH #690); this is the measurement that says it
//! no longer does (OR-F7).
//!
//! The script runs the way a `resident` code cell runs it: one living cell over every
//! message (`support::Screen`, the curator driver, GH #809).

mod support;

use meclaw_core::serde_json::{Value, json};
use support::{Screen, component_view, library_ships, pane, window_id};

/// The strike of the order the root is holding, as the clock sends it (`in_tick`, its
/// `schedule_id`): the pass that arrives as the strike. An order that has already fired
/// is gone from the timer, and asking for it to be removed is an error the hive logs.
///
/// Since GH #809 a tap is a web event and a strike an `in_tick` -- one message never
/// carries both, as the old hand-built plan could -- so the strike is its own pass,
/// right before the tap it used to ride on.
fn strike(screen: &mut Screen, now: u64, struck: &str) {
    screen.pass(json!({"kind": "stroke", "struck": struck}), now);
    let orders = screen.lane("due");
    assert!(
        !orders
            .iter()
            .any(|o| o["op"] == "remove" && o["schedule_id"] == struck),
        "the order that struck is gone from the timer already: {orders:?}"
    );
}

/// The id of the order the root is holding: what the clock was last told.
fn due_of(screen: &Screen) -> String {
    screen
        .props("display.root")
        .and_then(|p| p["due"].as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Ten taps at one cadence on ONE tile.
///
/// `step_ms` is the whole point of the parameter. One tap a second puts most passes in a
/// different due second, so the order id differs from the standing one and the
/// `sid == old` case -- the one that made a window flash and vanish -- is rare. A hand at
/// ten taps a second stays inside one second, and then it is the rule.
fn ten_taps(step_ms: u64) {
    assert!(
        library_ships(),
        "the display library is part of this repository"
    );
    let mut screen = Screen::new(json!({}));
    // Relevance 0.5 against the default bar of 0.3 gives a score of 0.25: the window is
    // present with a tile and is NOT open, so the first tap has something to open.
    screen.write(
        component_view(
            "a",
            "main",
            pane("a", json!({"context": "work", "relevance": "0.5"})),
        ),
        1000,
    );
    let window = window_id("alex", "a");
    assert_eq!(
        screen.curator(&window, "open"),
        json!(false),
        "not yet open"
    );

    let (mut ordered, mut cancelled, mut same_second) = (0usize, 0usize, 0usize);
    for i in 0..10u64 {
        let before = due_of(&screen);
        let now = 2000 + i * step_ms;
        // Before tap 2 the order the root is holding strikes. Early -- the order is due
        // at 22 s -- so this pass computes the SAME order again and would not have taken
        // it back with or without `struck` (measured with the curator driver: `add` of the
        // same id either way). What it holds here is only that a strike in the middle of a
        // hand's taps takes nothing back; that `struck` is what stops the cancellation is
        // measured at the order's own moment, with a counter-probe, in
        // `a_strike_at_its_moment_takes_back_nothing` below (wave Display review m4).
        let before = if i == 2 {
            strike(&mut screen, now, &before);
            due_of(&screen)
        } else {
            before
        };
        screen.pass(json!({"kind": "tap", "for": window.as_str()}), now);

        let orders = screen.lane("due");
        let adds: Vec<&Value> = orders
            .iter()
            .copied()
            .filter(|o| o["op"] == "add")
            .collect();
        let removes: Vec<&Value> = orders
            .iter()
            .copied()
            .filter(|o| o["op"] == "remove")
            .collect();
        assert!(adds.len() <= 1, "at most one order per pass: {orders:?}");
        assert!(
            removes.len() <= 1,
            "and at most one cancellation: {orders:?}"
        );
        for r in &removes {
            assert_eq!(
                r["schedule_id"],
                before.as_str(),
                "a cancellation only ever names what the root was holding: {orders:?}"
            );
            if let Some(a) = adds.first() {
                assert_ne!(
                    r["schedule_id"], a["schedule_id"],
                    "and never the order of this very pass: {orders:?}"
                );
            }
        }
        if let Some(a) = adds.first() {
            if a["schedule_id"] == before.as_str() && !before.is_empty() {
                same_second += 1;
            }
            assert_eq!(
                due_of(&screen),
                a["schedule_id"].as_str().unwrap_or(""),
                "the root holds what was actually ordered: {orders:?}"
            );
        }
        ordered += adds.len();
        cancelled += removes.len();

        // § 5.8 -- and this is the state the NEXT tap acts on.
        if i % 2 == 0 {
            assert_eq!(
                screen.curator(&window, "open"),
                json!(true),
                "tap {i} found a closed window and opened it (§ 5.1)"
            );
            assert_eq!(screen.curator(&window, "since"), json!(now), "{i}");
            assert_eq!(
                screen.curator(&window, "led_until"),
                json!(now + 20_000),
                "and the finger holds it up for the linger (§ 4.19): {i}"
            );
            assert_eq!(screen.curator(&window, "dismissed_at"), json!(0), "{i}");
        } else {
            assert_eq!(
                screen.curator(&window, "open"),
                json!(false),
                "tap {i} found an open window and put it away (§ 5.2)"
            );
            assert_eq!(screen.curator(&window, "dismissed_at"), json!(now), "{i}");
            assert_eq!(screen.curator(&window, "led_until"), json!(0), "{i}");
            assert_eq!(
                screen.curator(&window, "since"),
                json!(now - step_ms),
                "a put-away leaves `since` where it was (§ 5.2): {i}"
            );
        }
        // Ten taps and the tile never goes: a put-away keeps the rank and the decay runs
        // on (§ 4.27, R-24-1), so the finger always has something to hit again.
        assert!(
            screen.holds("display.dock/tile.view.alex.a"),
            "the tile stands after tap {i}"
        );
    }
    assert_eq!(ordered, 10, "every one of the ten passes drives the clock");
    assert!(
        cancelled > 0,
        "and some of them replace the standing order, which is where the collision used \
         to be"
    );
    assert!(
        same_second > 0,
        "while a pass that computes the second the root already holds re-orders it \
         WITHOUT cancelling it first"
    );
}

/// Ten taps a second apart: the cadence of a person pointing at things.
#[test]
fn ten_taps_a_second_apart_open_and_put_away_by_turns() {
    ten_taps(1000);
}

/// Ten taps inside one second -- the cadence OR-F7 names, and the one that reaches the
/// case GH #681/#690 were about: every pass computes the second the root is already
/// holding, so the order id it places IS the standing one. A screen that cancelled it
/// first would remove the row it is about to write, and the moment would never strike.
#[test]
fn ten_taps_inside_one_second_never_cancel_the_order_they_place() {
    ten_taps(100);
    // And a cadence that straddles the boundary, where both cases occur in one run.
    ten_taps(200);
}

/// The strike at the moment the order was placed for, and its counter-probe.
///
/// A strike is the order firing: the timer has already dropped it, so the pass that
/// arrives as the strike places the next order and must NOT take the struck one back --
/// asking the timer to remove an order it no longer holds is an error the hive logs. The
/// counter-probe is the same pass WITHOUT `struck`: it takes the standing order back,
/// which is what proves the suppression is `struck`'s doing and not a pass that had
/// nothing to cancel.
#[test]
fn a_strike_at_its_moment_takes_back_nothing() {
    assert!(
        library_ships(),
        "the display library is part of this repository"
    );
    let run = |with_struck: bool| {
        let mut screen = Screen::new(json!({}));
        screen.write(
            component_view(
                "a",
                "main",
                pane("a", json!({"context": "work", "relevance": "0.5"})),
            ),
            1000,
        );
        let window = window_id("alex", "a");
        screen.pass(json!({"kind": "tap", "for": window.as_str()}), 2000);
        let at = screen
            .lane("due")
            .iter()
            .find(|o| o["op"] == "add")
            .and_then(|o| o["at"].as_str().map(str::to_string))
            .expect("the tap's pass orders the next stroke");
        let placed = due_of(&screen);
        // The order's own moment. `at` is ISO seconds on the first day of the epoch here
        // (the driver's clock starts at 1 s), so its time of day is the whole moment.
        let clock: Vec<u64> = at
            .split('T')
            .nth(1)
            .unwrap_or("")
            .trim_end_matches('Z')
            .split(':')
            .map(|x| x.parse().expect("an ISO moment"))
            .collect();
        assert!(at.starts_with("1970-01-01T") && clock.len() == 3, "{at}");
        let moment = (clock[0] * 3600 + clock[1] * 60 + clock[2]) * 1000;
        let struck = if with_struck {
            placed.clone()
        } else {
            String::new()
        };
        screen.pass(json!({"kind": "stroke", "struck": struck}), moment);
        let orders: Vec<Value> = screen.lane("due").into_iter().cloned().collect();
        (placed, orders)
    };

    let (placed, orders) = run(true);
    let adds: Vec<&Value> = orders.iter().filter(|o| o["op"] == "add").collect();
    assert_eq!(
        adds.len(),
        1,
        "the strike's pass orders the next stroke: {orders:?}"
    );
    assert_ne!(
        adds[0]["schedule_id"],
        placed.as_str(),
        "a NEW order, one the old would be cancelled for: {orders:?}"
    );
    assert!(
        !orders.iter().any(|o| o["op"] == "remove"),
        "and the struck order is not taken back -- the timer has dropped it: {orders:?}"
    );

    let (placed, orders) = run(false);
    assert!(
        orders
            .iter()
            .any(|o| o["op"] == "remove" && o["schedule_id"] == placed.as_str()),
        "counter-probe: the same pass without `struck` takes the standing order back, so \
         the suppression above is `struck`'s doing: {orders:?}"
    );
}
