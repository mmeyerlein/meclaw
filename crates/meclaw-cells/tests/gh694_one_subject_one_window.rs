//! One subject, one window (display-hive.md § 4.12).
//!
//! A fresh window of ANOTHER application on the topic of a present window yields: the
//! standing window receives the touch (source f) and the fresh one is marked
//! `topic_dupe` -- neither present nor open, no window, no tile, no rung, not computed --
//! until the standing window loses its presence. Windows of one owner are never compared:
//! three timers stay three timers.
//!
//! The judge does not lift the mark (Ruling 13.09., S-029). It decides what is open,
//! never what exists.
//!
//! The script runs the way a `code` cell runs it: as a subprocess, with the pass's
//! document on stdin.

mod support;

use meclaw_core::serde_json::{Value, json};
use support::{Screen, library_ships, pane, view_of, window_id};

/// The dock child of a window, keyed by the window's own id.
fn tile_of(window: &str) -> String {
    format!("display.dock/tile.{}", window.replace('/', "~"))
}

fn weather(pinned: bool) -> Value {
    view_of(
        "ambient",
        "weather",
        "main",
        pane(
            "w",
            json!({"context": "ambient", "relevance": "0.3", "topic": "weather:berlin",
                   "pinned": pinned}),
        ),
    )
}

fn card() -> Value {
    view_of(
        "v2v",
        "card",
        "main",
        pane(
            "c",
            json!({"context": "conversation", "relevance": "0.9", "topic": "weather:berlin"}),
        ),
    )
}

/// Two applications, one subject: the standing window is touched and the fresh one
/// yields -- no window, no tile, no rung, not computed.
#[test]
fn a_fresh_window_yields_to_a_standing_one_on_the_same_topic() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(json!({}));
    screen.write(weather(true), 1000);
    screen.write(card(), 60_000);

    let standing = window_id("ambient", "weather");
    let fresh = window_id("v2v", "card");
    assert_eq!(
        screen.curator(&fresh, "topic_dupe"),
        json!(true),
        "the floor marks it: {}",
        screen.screen_state()
    );
    assert_eq!(
        screen.curator(&fresh, "present"),
        json!(false),
        "and it is not present"
    );
    assert_eq!(
        screen.curator(&fresh, "rung"),
        Value::Null,
        "a dupe carries no rung -- it is not computed at all"
    );
    assert!(
        !screen.holds(&format!("tv.{fresh}/c.c")),
        "no window anywhere: {:?}",
        screen.held
    );
    assert!(
        !screen.holds(&tile_of(&fresh)),
        "and no tile either: {:?}",
        screen.held
    );
    // The answer reaches the standing window instead: that is what source (f) is for.
    assert_eq!(
        screen.curator(&standing, "since"),
        json!(60_000),
        "the standing window is touched: {}",
        screen.screen_state()
    );
    assert!(screen.holds(&tile_of(&standing)), "and keeps its tile");
}

/// Two windows of the SAME owner are never compared: three timers stay three timers.
#[test]
fn one_owner_may_say_the_same_thing_twice() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let timer = |view_id: &str, key: &str| {
        view_of(
            "ambient",
            view_id,
            "main",
            pane(key, json!({"context": "ambient", "topic": "timer"})),
        )
    };
    let mut screen = Screen::new(json!({}));
    screen.write(timer("t1", "a"), 1000);
    screen.write(timer("t2", "b"), 1000);
    for view_id in ["t1", "t2"] {
        let oid = window_id("ambient", view_id);
        assert_eq!(
            screen.curator(&oid, "topic_dupe"),
            json!(false),
            "{oid} is not a duplicate: {}",
            screen.screen_state()
        );
        assert_eq!(screen.curator(&oid, "present"), json!(true), "{oid}");
        assert!(screen.holds(&tile_of(&oid)), "{oid} has a tile of its own");
    }
}

/// The judge does not lift the mark (S-029): a verdict may raise a window's relevance as
/// far as it likes, and a dupe still has no window. Presence is not the judge's to give
/// -- it decides size, never existence.
#[test]
fn the_judge_does_not_lift_the_dupe() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(json!({"judge": "on"}));
    screen.write(weather(true), 1000);
    screen.write(card(), 60_000);
    let fresh = window_id("v2v", "card");
    assert_eq!(screen.curator(&fresh, "topic_dupe"), json!(true));

    let mut named = meclaw_core::serde_json::Map::new();
    named.insert(fresh.clone(), json!({"judged_relevance": 1.0}));
    screen.pass(
        json!({"kind": "verdict", "bar": 0.1, "weights": {"conversation": 1.0},
               "windows": named}),
        61_000,
    );
    assert_eq!(
        screen.curator(&fresh, "topic_dupe"),
        json!(true),
        "the verdict does not lift it: {}",
        screen.screen_state()
    );
    assert_eq!(screen.curator(&fresh, "present"), json!(false));
    assert_eq!(
        screen.curator(&fresh, "score"),
        json!(0.0),
        "a dupe is not computed, whatever the judge said"
    );
    assert!(!screen.holds(&tile_of(&fresh)), "{:?}", screen.held);
}

/// The mark falls when the standing window loses its presence: the subject is free
/// again, and the window that yielded takes it.
#[test]
fn the_dupe_falls_when_the_standing_window_goes() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(json!({}));
    screen.write(weather(false), 1000);
    screen.write(card(), 2000);
    let standing = window_id("ambient", "weather");
    let fresh = window_id("v2v", "card");
    assert_eq!(screen.curator(&fresh, "topic_dupe"), json!(true));

    screen.withdraw("ambient", "weather");
    screen.pass(
        json!({"kind": "app_withdraw", "oid": standing.clone()}),
        3000,
    );
    assert_eq!(
        screen.curator(&fresh, "topic_dupe"),
        json!(false),
        "the subject is free again: {}",
        screen.screen_state()
    );
    assert_eq!(screen.curator(&fresh, "present"), json!(true));
    // And in the next pass it stands where the weather stood.
    screen.pass(json!({"kind": "stroke"}), 4000);
    assert!(screen.holds(&tile_of(&fresh)), "{:?}", screen.held);
    assert!(
        !screen.holds(&tile_of(&standing)),
        "the withdrawn one is gone: {:?}",
        screen.held
    );
}

/// The hints reach a prose window as well: `topic` is written on it, and a hint of the
/// wrong shape is refused at the door.
#[test]
fn a_prose_window_carries_its_topic() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let content = json!({"title": "Weather", "body": "Sunny", "topic": "weather:berlin"});
    let prose = json!({
        "owner": "alex", "view_id": "p", "region": "main", "ord": 0,
        "kind": "prose", "content": content.to_string(),
        "components": "[]", "ttl_ms": 0, "updated_at": 1,
    });
    let mut screen = Screen::new(json!({}));
    // Through the door, as the app sends it: a prose row has no window node, its whole
    // `content` IS the hints (`hints_of_row`).
    screen.write(prose, 1000);
    let oid = window_id("alex", "p");
    let props = screen.props(&oid).expect("the prose window");
    assert_eq!(props["topic"], "weather:berlin", "{props}");
    assert_eq!(
        screen.curator(&oid, "topic_dupe"),
        json!(false),
        "alone on its subject"
    );
    // And a `topic` that is not text never reaches the state at all: § 3.3 makes the
    // wire of a hint part of the description -- `topic` is text -- and the ONE door of
    // § 4.6 answers the sender a receipt they can read instead of taking it. Driven over
    // the write lane, because that is where the door sits.
    let doc = json!({
        "params": {},
        "body": {"view_id": "p", "region": "main", "kind": "prose",
                 "content": {"title": "Weather", "body": "21 degrees",
                             "topic": {"about": "weather:berlin"}},
                 "messages": []},
        "envelope": {"reply_to": "members/alex/apps/weather",
                     "header": {"hop": {"route": "in_view"}, "context": {}}},
    });
    let emissions = support::raw(&doc);
    let receipts: Vec<&Value> = emissions
        .iter()
        .filter(|e| e["header"]["route"] == "receipt")
        .collect();
    assert_eq!(
        receipts.len(),
        1,
        "one receipt, and no store bundle: {emissions:?}"
    );
    assert_eq!(receipts[0]["receipt"]["error_code"], "view_refused");
    assert!(
        receipts[0]["receipt"]["detail"]
            .as_str()
            .is_some_and(|d| d.contains("topic") && d.contains("§ 3.3")),
        "the receipt names the hint and the sentence: {}",
        receipts[0]["receipt"]["detail"]
    );
    assert!(
        emissions.iter().all(|e| e["header"]["route"] != "views"),
        "and nothing is written to the store: {emissions:?}"
    );
}
