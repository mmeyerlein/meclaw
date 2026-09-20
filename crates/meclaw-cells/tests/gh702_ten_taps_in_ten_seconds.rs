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
//! The script runs the way a `code` cell runs it: as a subprocess, with the pass's
//! document on stdin.

mod support;

use meclaw_core::serde_json::{Value, json};
use support::{
    Screen, apply, component_view, drawn, library_ships, pane, put_state, raw, window_id,
};

/// One pass whose plan also carries `struck`: the id of the order this pass arrives as
/// the strike of. `Screen::pass` writes a plan without it, and one pass of the run needs
/// it -- an order that has already fired is gone from the timer, and asking for it to be
/// removed is an error the hive logs.
fn struck_pass(screen: &mut Screen, event: Value, now: u64, struck: &str) -> Vec<Value> {
    let plan = json!({
        "views": screen.rows, "state": screen.state, "define": [], "now": now,
        "event": event, "struck": struck,
        // The mark of the pass, the way `pass_views` hands it on (GH #744).
        "mark": {"tick": true, "tap": event["for"], "struck": struck},
    });
    let doc = json!({
        "params": screen.params,
        "body": {"messages": [{
            "origin": "tool", "type": "tool_result", "id": "d-query",
            "text": json!({"objects": screen.held}).to_string(),
        }]},
        "envelope": {"header": {
            "hop": {"operation": "query"},
            "context": {"display_origin": "read", "display_views": plan.to_string()},
        }},
    });
    let emissions = raw(&doc);
    // GH #765 (way A): the patch rides the state write and is drawn from its reply.
    let patch = drawn(&emissions);
    apply(&mut screen.held, &patch);
    for em in &emissions {
        let request = em["header"]["display_request"].as_str().unwrap_or("");
        if em["header"]["route"] == "views" && request.contains("\"state\"") {
            for leg in em["messages"].as_array().unwrap_or(&Vec::new()) {
                let call: Value =
                    meclaw_core::serde_json::from_str(leg["text"].as_str().unwrap_or("{}"))
                        .expect("a call is JSON");
                put_state(&mut screen.state, &call);
            }
        }
    }
    screen.last = emissions;
    patch
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
        // Pass 2 is the one that arrives as the strike of the order the root is holding.
        // It is chosen because pass 2 is a pass that WOULD cancel at every cadence here,
        // so the suppression is measured and not assumed.
        let struck = if i == 2 {
            before.clone()
        } else {
            String::new()
        };
        let event = json!({"kind": "tap", "for": window.as_str()});
        if struck.is_empty() {
            screen.pass(event, now);
        } else {
            struck_pass(&mut screen, event, now, &struck);
        }

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
        if !struck.is_empty() {
            assert!(
                removes.is_empty(),
                "the order that struck is gone from the timer already: {orders:?}"
            );
        }
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
