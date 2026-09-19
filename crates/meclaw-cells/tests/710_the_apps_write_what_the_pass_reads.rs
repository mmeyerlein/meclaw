//! The apps seam: what an app writes really travels the door, and the pass reads it.
//!
//! display-hive.md § 8.8 and § 9 say what an application writes -- `touched` on every new
//! line, `turn_id` on every turn, three views for `ambient`, a minute of the clock without
//! a touch. A subprocess test hands those hints to the pass by hand, which proves the pass
//! and not the door. This file lets a real cell emit them: `/alex/apps/probe` is a `code`
//! cell that speaks exactly the vocabulary of § 8.8 and § 9, its emissions travel the
//! member's lane onto the screen, and what is asserted is the pass's own state row.
//!
//! voice2vision is a foreign repository and does not travel with this one, which is why
//! the stand-in exists at all -- the seam under test is the DOOR, not the app.
//!
//! Anchors out of the 105 (befund 04 § B.2, seam "apps"): **S-009**, **S-010** and
//! **S-011** (`ambient` writes three views), **Q-02** and **Q-13** (`turn_id` and
//! `touched` of the chat), **Q-06**, **S-066**, **S-067** and **S-068** (the answer's
//! window closes the chat), **S-051** (the chat's two relevances), **S-056** and
//! **S-057** (a write without `touched` is no touch and asks nobody).

#[path = "support/display_colony.rs"]
mod display_colony;

use std::time::Duration;

use display_colony::{Boot, PROBE, boot, curator, have_python, library_ships};
use meclaw_core::serde_json::{Value, json};

const QUIET: Duration = Duration::from_millis(300);

/// One word of a window's entry in the dock's order (§ 4.28).
fn dock_entry(state: &Value, oid: &str) -> Value {
    state["dock_order"]
        .as_array()
        .expect("the state carries the dock's order")
        .iter()
        .find(|e| e["oid"] == oid)
        .unwrap_or_else(|| panic!("{oid} has no tile: {}", state["dock_order"]))
        .clone()
}

fn seat(state: &Value, oid: &str) -> Value {
    dock_entry(state, oid)["seat"].clone()
}

fn seat_ord(state: &Value, oid: &str) -> Value {
    dock_entry(state, oid)["seat_ord"].clone()
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
    for which in ["clock", "weather", "timer"] {
        colony
            .app_put(
                json!({"do": "ambient", "view": which, "at": now_ms(), "text": which}),
                which,
            )
            .await;
    }
    let three = colony.state().await.expect("a state row stands");
    for oid in [&clock, &weather, &timer] {
        assert!(
            three["views"].get(oid).is_some(),
            "each of the three is a window of its own (§ 7.1): {three}"
        );
    }
    assert_eq!(
        three["views"][&clock]["seat"], "bottom",
        "the clock has a seat at the bottom of the dock (§ 9.1)"
    );
    assert_eq!(
        (seat(&three, &clock), seat_ord(&three, &clock)),
        (json!(true), json!(0)),
        "and the dock gives it that seat, lowest (§ 4.28): {}",
        three["dock_order"]
    );
    assert_eq!(
        (seat(&three, &weather), seat_ord(&three, &weather)),
        (json!(true), json!(10)),
        "the weather sits above it (§ 9.1)"
    );
    assert_eq!(
        seat(&three, &timer),
        json!(false),
        "and the timer has no seat at all: {}",
        three["dock_order"]
    );
    assert_eq!(
        three["views"][&weather]["topic"], "weather:berlin",
        "the weather says what it is about, place and all (§ 9.1)"
    );

    // -- S-056, S-057, § 9.3: a write without `touched` is no touch, and asks nobody --
    let before_since = curator(&three, &clock, "since");
    let before_asks = colony.judge_questions().await;
    colony
        .app_put(json!({"do": "minute", "text": "12:01"}), "clock")
        .await;
    colony.settle(QUIET).await;
    let minute = colony.state().await.expect("a state row stands");
    assert_eq!(
        curator(&minute, &clock, "since"),
        before_since,
        "the clock's minute moved nothing: the app said no `touched` (§ 4.8): {minute}"
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
        after_turn["chat"]["last_turn_id"],
        json!(turn),
        "the pass reads the chat's own `turn_id` off its window (§ 4.13, § 8.8): \
         {after_turn}"
    );
    assert_eq!(
        after_turn["views"][&chat]["relevance"],
        json!(0.8),
        "and the chat is a turn's chat while its newest line is a turn (S-051, § 8.5)"
    );
    assert_eq!(
        after_turn["views"][&chat]["layer"], "modal",
        "the chat competes on the modal ladder (§ 8.5)"
    );
    assert!(
        curator(&after_turn, &chat, "dismissed_at") == json!(0),
        "and nothing has put it away yet: {after_turn}"
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
    assert_eq!(
        closed["views"][&chat]["relevance"],
        json!(0.6),
        "the chat's newest line is an answer now (S-051, § 8.5): {closed}"
    );
    assert!(
        curator(&closed, &chat, "dismissed_at")
            .as_i64()
            .is_some_and(|at| at > 0),
        "a canvas window carrying the last turn's id closes the chat (§ 4.13): {closed}"
    );
    assert_eq!(
        curator(&closed, &chat, "led_until"),
        json!(0),
        "and the finger no longer holds it"
    );
    assert_eq!(
        curator(&closed, &card, "present"),
        json!(true),
        "while the window that closed it stands: {closed}"
    );
    assert_eq!(
        closed["views"][&card]["turn_id"],
        json!(turn),
        "carrying the turn of the answer it came from (§ 8.3)"
    );

    colony.shutdown().await;
}

/// § 9.1: `ambient` is ONE act of the app and three views, and all three stand afterwards.
///
/// What this case prevents is the lost window: three writes in one breath are three passes,
/// and a pass that was handed the state row of the pass before it computes a state in which
/// the earlier view was never written -- gone for ever, because no later event writes it
/// again. `reconcile()` (§ 3.1, OR-H0.9, H1-F4) catches the state up with the store's rows
/// before the pass's own event runs, and carries this case;
/// `707_the_state_is_reconciled_with_the_store.rs` pins the rule itself.
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
        .wait_state("all three views of one act reach the state row", |s| {
            oids.iter().all(|oid| s["views"].get(oid).is_some())
        })
        .await;

    assert_eq!(
        colony.writes_taken().await,
        vec!["in_view", "in_view", "in_view"],
        "one act, three writes at the screen's door (§ 9.1)"
    );
    for oid in &oids {
        assert_eq!(
            curator(&three, oid, "present"),
            json!(true),
            "{oid} stands on the screen: {three}"
        );
        assert_eq!(
            dock_entry(&three, oid)["oid"],
            json!(oid),
            "and carries its own tile in the dock (§ 4.28): {}",
            three["dock_order"]
        );
    }
    assert_eq!(
        three["dock_order"]
            .as_array()
            .expect("the dock's order")
            .len(),
        3,
        "three tiles, no more and no fewer: {}",
        three["dock_order"]
    );

    colony.shutdown().await;
}
