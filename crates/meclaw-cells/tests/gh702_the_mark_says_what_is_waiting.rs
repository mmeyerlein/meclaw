//! display-hive.md § 4.33: the light on the OS mark -- how much is waiting in the tiles.
//!
//! `unseen` counts the present apps whose window is not open and which either carry rung
//! `urgent` or whose linger since the last touch still runs. Put-away windows do not
//! count, a put-away urgent neither; a leaving window is not present and does not count.
//! There is no "seen" memory anywhere: both reasons fall away by themselves, and "seen"
//! would be a statement about ONE pair of eyes, which this hive does not have (R-23-4).
//!
//! The number is SYSTEMWIDE and independent of the dock's cut (S-044): one value in one
//! state, the same on every output, however many tiles each output draws. Whether an
//! output paints the dot is § 6.8 and is the client's business.
//!
//! § 4.34: the curator orders a stroke for every decay transition of every window in the
//! state, so the moment the light goes out is a moment the screen wakes for -- no
//! polling, and no dot left burning with nothing behind it.

mod support;

use meclaw_core::serde_json::{Value, json};
use support::{Screen, component_view, foreign_view, library_ships, pane, window_id};

/// One phone. Nothing here depends on the type: the count is the state's.
fn phone() -> Value {
    json!({"screens": {"handy": {"display_type": "phone", "viewing_distance_m": 0.35,
                                 "inputs": ["audio", "touch"], "dock_max": 8}},
           "default_screen": "handy"})
}

/// Two outputs of very different size, so a difference in the CUT cannot hide inside a
/// difference in the state (S-044).
fn tv_and_phone() -> Value {
    json!({"screens": {
               "tv": {"display_type": "tv", "viewing_distance_m": 3.0, "dock_max": 7},
               "handy": {"display_type": "phone", "viewing_distance_m": 0.35,
                         "inputs": ["audio", "touch"], "dock_max": 5}},
           "default_screen": "tv"})
}

fn note(view_id: &str, props: Value) -> Value {
    component_view(view_id, "main", pane(view_id, props))
}

/// The number on one output's mark. Its own function because the mark carries the whole
/// voice hook in `client_js`, and an assertion that printed every prop of it would print
/// thirty kilobytes of JavaScript on failure.
fn mark(screen: &Screen, exit: &str) -> String {
    screen
        .props(&format!("{exit}.display.os"))
        .expect("the mark stands")["unseen"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

/// Every moment this pass ordered from the clock (§ 4.34). A pass that orders nothing
/// answers an empty list, and that is a statement of its own.
fn ordered(screen: &Screen) -> Vec<String> {
    screen
        .lane("due")
        .iter()
        .filter(|e| e["op"] == "add")
        .map(|e| e["at"].as_str().unwrap_or("").to_string())
        .collect()
}

/// A window nobody opened waits in its tile and says so, until its own linger runs out.
#[test]
fn a_window_waiting_in_a_tile_lights_the_mark_and_stops_by_itself() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(phone());
    screen.write(
        note("a", json!({"context": "work", "relevance": "0.9"})),
        1000,
    );
    screen.write(
        note("b", json!({"context": "work", "relevance": "0.05"})),
        1000,
    );
    screen.pass(json!({"kind": "stroke"}), 2000);

    assert_eq!(screen.curator(&window_id("alex", "a"), "level"), json!(1));
    assert_eq!(screen.curator(&window_id("alex", "b"), "level"), json!(0));
    assert_eq!(mark(&screen, "handy"), "1", "one window waits in a tile");

    // The linger ran from `since` = 1000, so at 21 s the reason is gone -- read at
    // EXACTLY that moment, because the moment is the statement: the count asks
    // `now - since < linger_of`, and a pass a second later would be dark either way.
    screen.pass(json!({"kind": "stroke"}), 21_000);
    assert_eq!(
        mark(&screen, "handy"),
        "0",
        "and the light goes out on its own -- no `seen`, no memory (R-23-4)"
    );
}

/// An urgent window that is not the one in front rings in its tile, and the mark says so
/// for as long as it rings (§ 4.18, § 4.33).
#[test]
fn an_urgent_window_in_a_tile_lights_the_mark() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(phone());
    screen.write(
        note("t1", json!({"context": "ambient", "state": "urgent"})),
        1000,
    );
    screen.write(
        note("t2", json!({"context": "system", "state": "urgent"})),
        1000,
    );
    screen.pass(json!({"kind": "stroke"}), 2000);

    let front: Vec<String> = ["t1", "t2"]
        .iter()
        .map(|v| window_id("alex", v))
        .filter(|oid| screen.curator(oid, "front") == json!(true))
        .collect();
    assert_eq!(
        front.len(),
        1,
        "exactly one urgent stands in front (§ 4.18)"
    );
    assert_eq!(
        mark(&screen, "handy"),
        "1",
        "the other one rings in its tile and the mark counts it"
    );
}

/// A window the finger put away does not light the mark, however fresh it is (§ 4.33).
///
/// The count reads "younger than its own linger", and a window that was just tapped away
/// is exactly that: level 0, `since` well inside the linger. Counting it would turn the
/// light ON in the moment a person put the window down -- the opposite of what the light
/// means. The control window is what makes the measurement a measurement: without the
/// put-away rule the mark would read two.
#[test]
fn a_window_the_finger_put_away_does_not_light_the_mark() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(phone());
    screen.write(
        note("a", json!({"context": "work", "relevance": "0.9"})),
        1000,
    );
    screen.write(
        note("b", json!({"context": "work", "relevance": "0.05"})),
        1000,
    );
    screen.pass(json!({"kind": "stroke"}), 2000);
    assert_eq!(mark(&screen, "handy"), "1", "the quiet one waits");

    let a = window_id("alex", "a");
    screen.pass(json!({"kind": "tap", "for": &a}), 3000);
    assert_eq!(screen.curator(&a, "dismissed_at"), json!(3000));
    assert_eq!(
        screen.curator(&a, "level"),
        json!(0),
        "it went into its tile"
    );

    // One pass later it is settled again, still level 0, still well inside its linger --
    // and still not asking for anything.
    screen.pass(json!({"kind": "stroke"}), 4000);
    assert_eq!(screen.curator(&a, "age"), json!("settled"));
    assert_eq!(
        screen.curator(&a, "since"),
        json!(1000),
        "inside its linger"
    );
    assert_eq!(
        mark(&screen, "handy"),
        "1",
        "the count is the quiet window alone: what the finger put away has been seen"
    );
}

/// The same for a window that rings: a put-away urgent keeps its rung and its tile, and
/// the mark stays as dark as it was (§ 4.18, § 4.33).
#[test]
fn an_urgent_window_the_finger_put_away_leaves_the_mark_dark() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(phone());
    screen.write(
        note("urg", json!({"context": "system", "state": "urgent"})),
        1000,
    );
    screen.write(
        note("b", json!({"context": "work", "relevance": "0.05"})),
        1000,
    );
    screen.pass(json!({"kind": "stroke"}), 2000);
    let urg = window_id("alex", "urg");
    assert_eq!(
        screen.curator(&urg, "level"),
        json!(3),
        "it stands in front"
    );
    assert_eq!(mark(&screen, "handy"), "1", "only the quiet window waits");

    screen.pass(json!({"kind": "tap", "for": &urg}), 3000);
    screen.pass(json!({"kind": "stroke"}), 4000);
    assert_eq!(
        screen.curator(&urg, "rung"),
        json!("urgent"),
        "it still rings"
    );
    assert_eq!(screen.curator(&urg, "level"), json!(0), "from its tile");
    assert_eq!(
        mark(&screen, "handy"),
        "1",
        "but the person has answered it, so it adds nothing to the count"
    );
}

/// And the clock orders the moment the light goes out (§ 4.34).
#[test]
fn the_clock_orders_the_moment_the_light_goes_out() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(phone());
    screen.write(
        note("b", json!({"context": "work", "relevance": "0.05"})),
        1000,
    );
    // The second pass: in the first the window is `fresh`, and a fresh window's own
    // stroke (one second later, § 4.34) would be the earliest moment of the state.
    screen.pass(json!({"kind": "stroke"}), 2000);
    assert_eq!(mark(&screen, "handy"), "1", "the light is on");
    assert_eq!(
        ordered(&screen),
        vec!["1970-01-01T00:00:21Z".to_string()],
        "`since` 1000 plus the linger of 20 s: the screen woke itself for the moment \
         its reason runs out"
    );
}

/// S-044: the count is one number for the whole screen, and the dock's cut does not
/// touch it -- seven quiet windows count seven on the phone that draws five tiles.
#[test]
fn the_count_is_the_same_on_every_output_whatever_each_dock_draws() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(tv_and_phone());
    for i in 0..7 {
        screen.write(
            note(
                &format!("n{i}"),
                json!({"context": "work", "relevance": "0.05"}),
            ),
            1000,
        );
    }
    screen.pass(json!({"kind": "stroke"}), 2000);
    for i in 0..7 {
        assert_eq!(
            screen.curator(&window_id("alex", &format!("n{i}")), "level"),
            json!(0),
            "all seven are quiet: none of them is open"
        );
    }
    assert_eq!(
        screen.props("tv.display.dock").expect("the dock")["count"],
        7,
        "the television draws all seven tiles"
    );
    assert_eq!(
        screen.props("handy.display.dock").expect("the dock")["count"],
        5,
        "the phone draws five (§ 4.30)"
    );
    assert_eq!(mark(&screen, "tv"), "7");
    assert_eq!(
        mark(&screen, "handy"),
        "7",
        "and the number is the state's, not the dock's (S-044)"
    );
}

/// A window the curator holds back as a repetition has no tile anywhere, so it lights no
/// mark (§ 4.12, § 4.33).
#[test]
fn a_repetition_without_a_tile_does_not_light_the_mark() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let weather = |title: &str| {
        json!({"context": "ambient", "relevance": "0.05", "topic": "weather",
               "title": title})
    };
    let mut screen = Screen::new(phone());
    screen.write(
        component_view("w", "main", pane("w", weather("ours"))),
        1000,
    );
    screen.pass(json!({"kind": "stroke"}), 2000);
    assert_eq!(mark(&screen, "handy"), "1", "ours waits in its tile");

    // Another app puts the same subject up. The standing window takes the touch (f), the
    // fresh one becomes `topic_dupe`: no window, no tile, not counted.
    let theirs = foreign_view("w", "main", pane("w", weather("theirs")));
    screen.write(theirs, 3000);
    let dupe = window_id("robin", "w");
    assert_eq!(screen.curator(&dupe, "topic_dupe"), json!(true));
    assert!(!screen.curator(&dupe, "present").as_bool().unwrap_or(true));
    assert!(
        !screen.holds(&format!(
            "handy.display.dock/tile.{}",
            dupe.replace('/', "~")
        )),
        "a repetition has no tile"
    );
    assert_eq!(
        mark(&screen, "handy"),
        "1",
        "so the count is the standing window alone"
    );
}

/// A window that rings lights the mark on its own, long past its linger (§ 4.33).
///
/// The count has two reasons and they are read as one `or`: inside a linger the second
/// one answers first, so every measurement taken there proves nothing about the ring.
/// Forty-two seconds after the touch every linger on this screen is twenty seconds gone
/// -- and the second urgent window still rings in its tile, so the mark still says one.
#[test]
fn a_ringing_window_lights_the_mark_long_after_its_linger() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(phone());
    screen.write(
        note("t1", json!({"context": "ambient", "state": "urgent"})),
        1000,
    );
    screen.write(
        note("t2", json!({"context": "system", "state": "urgent"})),
        1000,
    );
    screen.pass(json!({"kind": "stroke"}), 42_000);

    let ringing: Vec<String> = ["t1", "t2"]
        .iter()
        .map(|v| window_id("alex", v))
        .filter(|oid| screen.curator(oid, "level") == json!(0))
        .collect();
    assert_eq!(ringing.len(), 1, "one of the two is not in front");
    assert_eq!(
        screen.curator(&ringing[0], "rung"),
        json!("urgent"),
        "and it still rings"
    );
    assert_eq!(
        mark(&screen, "handy"),
        "1",
        "with no linger left to say it (§ 4.33: urgent is a reason of its own)"
    );
}
