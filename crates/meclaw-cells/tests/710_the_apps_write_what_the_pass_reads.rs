//! The apps seam: what an app writes really travels the door, and the pass reads it.
//!
//! display-hive.md § 8.8 and § 9 say what an application writes -- `touched` on every new
//! line, `turn_id` on every turn, three views for `ambient`, a minute of the clock without
//! a touch. A subprocess test hands those hints to the pass by hand, which proves the pass
//! and not the door. This file lets a real cell emit them: `/alex/apps/probe` is a `code`
//! cell that speaks exactly the vocabulary of § 8.8 and § 9, its emissions travel the
//! member's lane onto the screen, and what is asserted is what the pass drew at `web` and what it
//! kept in the store (since GH #809 the curator's state is memory and no message carries it).
//!
//! voice2vision is a foreign repository and does not travel with this one, which is why
//! the stand-in exists at all -- the seam under test is the DOOR, not the app.
//!
//! Anchors out of the 105 (befund 04 § B.2, seam "apps"): **S-009**, **S-010** and
//! **S-011** (`ambient` writes three views), **Q-02** and **Q-13** (`turn_id` and
//! `touched` of the chat), **Q-06**, **S-066**, **S-067** and **S-068** (the answer's
//! window closes the chat), **S-056** and **S-057** (a write without `touched` is no touch
//! and asks nobody). S-051 (the chat's two relevances) and the chat's `last_turn_id` are
//! values of the model that no message carries since GH #809; the CURATOR run holds them.

#[path = "support/display_colony.rs"]
mod display_colony;

use std::time::Duration;

use display_colony::{Boot, PROBE, attr, boot, have_python, library_ships, present, tile};
use meclaw_core::serde_json::{Value, json};

const QUIET: Duration = Duration::from_millis(300);

/// The tiles of the default output's dock, bottom first (the highest `ord` stands lowest).
fn dock(tree: &Value) -> Vec<String> {
    let mut tiles: Vec<(String, i64)> = tree
        .as_object()
        .expect("the tree is a map")
        .iter()
        .filter(|(id, _)| id.starts_with("display.dock/tile."))
        .map(|(_, o)| {
            (
                o["props"]["oid"].as_str().unwrap_or("").to_string(),
                o["ord"].as_i64().unwrap_or(0),
            )
        })
        .collect();
    tiles.sort_by_key(|t| -t.1);
    tiles.into_iter().map(|t| t.0).collect()
}

/// A moment, in epoch milliseconds, the way an app stamps `touched` (§ 3.3).
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after 1970")
        .as_millis() as u64
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_apps_write_what_the_pass_reads() {
    if !library_ships() || !have_python() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // The judge is ON with its brake wide open, so "this write asked" and "this write did
    // not" are two measurable answers rather than one silence (S-057, Q-18). The verdict
    // is neutral: a low bar and no word about any window, so nothing below is the judge's
    // doing except the counting.
    let colony = boot(Boot {
        judge: true,
        judge_min_interval_ms: 1,
        linger_ms: 60_000,
        fade_ms: 120_000,
        verdict: json!({"bar": 0.2, "weights": {}, "windows": []}),
    })
    .await;
    let clock = colony.oid(PROBE, "clock");
    let weather = colony.oid(PROBE, "weather");
    let timer = colony.oid(PROBE, "timer");
    let chat = colony.oid(PROBE, "chat");
    let card = colony.oid(PROBE, "card");

    // -- S-009, S-010, S-011, § 9.1: ambient is three views, not one -----------------
    let mut three = Value::Null;
    for which in ["clock", "weather", "timer"] {
        three = colony
            .app_put(
                json!({"do": "ambient", "view": which, "at": now_ms(), "text": which}),
                which,
            )
            .await;
    }
    for oid in [&clock, &weather, &timer] {
        assert!(
            present(&three, oid),
            "each of the three is a window of its own (§ 7.1): {three}"
        );
    }
    assert_eq!(
        attr(&three, &clock, "seat"),
        json!("bottom"),
        "the clock asks for a seat at the bottom of the dock (§ 9.1)"
    );
    assert_eq!(
        (
            tile(&three, &clock)["seat"].clone(),
            tile(&three, &weather)["seat"].clone()
        ),
        (json!("1"), json!("1")),
        "and the dock gives the clock and the weather a seat each (§ 4.28): {:?}",
        dock(&three)
    );
    let order = dock(&three);
    let at = |oid: &String| order.iter().position(|o| o == oid);
    assert!(
        at(&clock) < at(&weather),
        "the clock sits lowest, the weather above it (§ 9.1): {order:?}"
    );
    assert_eq!(
        tile(&three, &timer)["seat"],
        json!(""),
        "and the timer has no seat at all: {order:?}"
    );
    assert_eq!(
        attr(&three, &weather, "topic"),
        json!("weather:berlin"),
        "the weather says what it is about, place and all (§ 9.1)"
    );

    // -- S-056, S-057, § 9.3: a write without `touched` is no touch, and asks nobody --
    let before_since = attr(&three, &clock, "since");
    let before_asks = colony.judge_questions().await;
    colony
        .app_put(json!({"do": "minute", "text": "12:01"}), "clock")
        .await;
    colony.settle(QUIET).await;
    let minute = colony.tree().await;
    assert_eq!(
        attr(&minute, &clock, "since"),
        before_since,
        "the clock's minute moved nothing: the app said no `touched` (§ 4.8)"
    );
    assert_eq!(
        colony.judge_questions().await,
        before_asks,
        "and a pass without a touch is no real event, so nobody was asked (§ 4.3)"
    );
    // The counter-probe: with the brake wide open, a write that DOES touch asks at once.
    colony
        .app_put(
            json!({"do": "ambient", "view": "weather", "at": now_ms(), "text": "19"}),
            "weather",
        )
        .await;
    colony.settle(QUIET).await;
    assert!(
        colony.judge_questions().await > before_asks,
        "a content change is a real event and asks (§ 4.3, § 9.4)"
    );

    // -- Q-02, Q-13, § 8.8: the chat writes `touched` and `turn_id` per turn ----------
    let turn = "turn-7";
    let after_turn = colony
        .app_put(
            json!({"do": "chat", "at": now_ms(), "turn_id": turn,
                   "text": "what is the weather", "relevance": "0.8"}),
            "chat",
        )
        .await;
    assert_eq!(
        attr(&after_turn, &chat, "turn_id"),
        json!(turn),
        "the chat's own `turn_id` reaches its window (§ 4.13, § 8.8)"
    );
    assert_eq!(
        attr(&after_turn, &chat, "layer"),
        json!("modal"),
        "the chat competes on the modal ladder (§ 8.5)"
    );
    let rest = colony.rest().await;
    assert_eq!(
        rest["views"][&chat]["dismissed_at"],
        json!(0),
        "and nothing has put it away yet: {rest}"
    );

    // -- Q-06, S-066, S-067, S-068, § 4.13: the answer's window closes the chat -------
    // The chat takes the answer (`touched`, its `turn_id` still the turn's), and the app
    // the answer woke puts up a canvas window carrying the SAME `turn_id`. What the two
    // writes do is step 5.
    colony
        .app_put(
            json!({"do": "chat", "at": now_ms(), "turn_id": turn,
                   "text": "21 degrees", "relevance": "0.6"}),
            "chat",
        )
        .await;
    let closed = colony
        .app_put(
            json!({"do": "card", "view_id": "card", "at": now_ms(), "turn_id": turn,
                   "title": "Berlin", "topic": "card:weather", "text": "21"}),
            "card",
        )
        .await;
    let rest = colony.rest().await;
    assert!(
        rest["views"][&chat]["dismissed_at"]
            .as_i64()
            .is_some_and(|at| at > 0),
        "a canvas window carrying the last turn's id closes the chat (§ 4.13): {rest}"
    );
    assert_eq!(
        rest["views"][&chat]["led_until"],
        json!(0),
        "and the finger no longer holds it"
    );
    assert_eq!(
        tile(&closed, &chat)["open"],
        json!(""),
        "the chat's tile says it is put away"
    );
    assert!(
        present(&closed, &card),
        "while the window that closed it stands: {closed}"
    );
    assert_eq!(
        attr(&closed, &card, "turn_id"),
        json!(turn),
        "carrying the turn of the answer it came from (§ 8.3)"
    );

    colony.shutdown().await;
}

/// § 9.1: `ambient` is ONE act of the app and three views, and all three stand afterwards.
///
/// What this case prevents is the lost window: three writes in one breath are three passes,
/// and a pass that did not know the write before it would lose that window for ever,
/// because no later event writes it again. Since GH #809 the three passes run one after
/// the other on the ONE state in the cell's memory.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_views_in_one_breath_lose_no_window() {
    if !library_ships() || !have_python() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // No judge, and a linger far longer than the run: nothing below is anybody's doing but
    // the three writes themselves.
    let colony = boot(Boot {
        linger_ms: 60_000,
        fade_ms: 120_000,
        ..Boot::default()
    })
    .await;
    let oids: Vec<String> = ["clock", "weather", "timer"]
        .iter()
        .map(|v| colony.oid(PROBE, v))
        .collect();

    // ONE command, and no `view` in it: the app writes clock, weather and timer in the
    // same act, and the three messages leave its door together.
    colony
        .app(json!({"do": "ambient", "at": now_ms(), "text": "one breath"}))
        .await;
    let three = colony
        .wait_tree("all three views of one act stand on the screen", |t| {
            oids.iter().all(|oid| present(t, oid))
        })
        .await;

    assert_eq!(
        colony.writes_taken().await,
        vec!["in_view", "in_view", "in_view"],
        "one act, three writes at the screen's door (§ 9.1)"
    );
    let order = dock(&three);
    for oid in &oids {
        assert!(
            order.contains(oid),
            "{oid} carries its own tile in the dock (§ 4.28): {order:?}"
        );
    }
    assert_eq!(
        order.len(),
        3,
        "three tiles, no more and no fewer: {order:?}"
    );

    colony.shutdown().await;
}
