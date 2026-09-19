//! display-hive.md § 4.3–4.5: the curator calls the judge at the end of a pass in which at
//! least one window received an APP touch, at the earliest `judge_min_interval_ms` after the
//! last call. What the judge sees is `judge_sees` (§ 4.4) and nothing else -- no geometry:
//! not the rank, not the tile, not the level, not `dock_max`, not `dismissed_at`, and no
//! standing view that is not present. What it answers replaces the whole verdict: a window
//! the answer does not name loses its old one.
//!
//! Scenarios: Q-15 (the payload), S-033 (the replacement), S-019 (no judge cell),
//! S-032 (two touches inside the interval ask once).

mod support;

use meclaw_core::serde_json::{Value, json};
use support::{Screen, component_view, library_ships, pane};

fn params(judge: &str) -> Value {
    json!({"judge": judge, "judge_min_interval_ms": 3000,
           "screens": {"monitor": {"display_type": "monitor", "inputs": ["pointer"]}},
           "default_screen": "monitor"})
}

fn note(view_id: &str, relevance: &str) -> Value {
    component_view(
        view_id,
        "main",
        pane(
            view_id,
            json!({"title": view_id, "context": "system", "relevance": relevance,
                   "topic": format!("note:{view_id}")}),
        ),
    )
}

/// The situation of the one judge question of this pass, or none.
fn question(screen: &Screen) -> Option<Value> {
    let asked = screen.lane("judge");
    assert!(asked.len() <= 1, "at most one question a pass");
    asked.first().map(|e| {
        meclaw_core::serde_json::from_str(e["messages"][0]["text"].as_str().expect("the payload"))
            .expect("the payload is JSON")
    })
}

#[test]
fn the_judge_sees_the_windows_and_no_geometry() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(params("on"));
    screen.write(note("n1", "0.9"), 100_000);
    let seen = question(&screen).expect("an app touch asks the judge");
    let mut keys: Vec<&str> = seen["windows"][0]
        .as_object()
        .expect("one window")
        .keys()
        .map(|k| k.as_str())
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "age",
            "age_ms",
            "class",
            "context",
            "id",
            "leads",
            "owner",
            "pinned",
            "relevance",
            "rung",
            "text",
            "topic",
            "verdict"
        ],
        "§ 4.4: exactly these, and no geometry"
    );
    let mut top: Vec<&str> = seen
        .as_object()
        .unwrap()
        .keys()
        .map(|k| k.as_str())
        .collect();
    top.sort();
    assert_eq!(
        top,
        vec!["bar", "last_answer", "last_turn", "weights", "windows"]
    );
    assert_eq!(seen["windows"][0]["id"], "view.alex.n1");
}

#[test]
fn a_verdict_replaces_the_one_before_it() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // S-033: the next judge run replaces bar, weights AND the per-window values of every
    // window -- a window the verdict does not name loses its old verdict.
    let mut screen = Screen::new(params("on"));
    screen.write(note("n1", "0.9"), 100_000);
    screen.write(note("n2", "0.9"), 100_000);
    screen.pass(
        json!({"kind": "verdict", "bar": 0.4, "weights": {"system": 0.2},
               "windows": {"view.alex.n1": {"judged_hidden": true},
                           "view.alex.n2": {"judged_relevance": 0.95}}}),
        101_000,
    );
    assert_eq!(screen.screen_state()["bar"], 0.4);
    assert_eq!(screen.screen_state()["weights"]["system"], 0.2);
    assert_eq!(screen.curator("view.alex.n1", "rung"), "hidden");
    assert_eq!(
        screen.screen_state()["views"]["view.alex.n2"]["verdict"]["judged_relevance"],
        0.95
    );

    screen.pass(
        json!({"kind": "verdict", "bar": 0.3, "weights": {},
               "windows": {"view.alex.n1": {"judged_hidden": true}}}),
        102_000,
    );
    assert_eq!(
        screen.screen_state()["views"]["view.alex.n2"]["verdict"]["judged_relevance"],
        Value::Null,
        "a window the verdict does not name loses its old verdict (§ 4.5)"
    );
}

#[test]
fn without_a_judge_cell_nothing_is_asked_and_no_verdict_is_taken() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // S-019: `judge: off` (or missing, or any other word) means the hive HAS no judge cell,
    // so no verdict can arrive -- the bar is `focus_default` and every weight is 0.5.
    let mut screen = Screen::new(params("off"));
    screen.write(note("n1", "0.9"), 100_000);
    assert!(question(&screen).is_none(), "off: nothing is asked");
    screen.pass(
        json!({"kind": "verdict", "bar": 0.9, "weights": {"system": 0.1}, "windows": {}}),
        101_000,
    );
    let codes: Vec<String> = screen
        .lane("receipt")
        .iter()
        .map(|e| {
            e["receipt"]["error_code"]
                .as_str()
                .unwrap_or("")
                .to_string()
        })
        .collect();
    assert_eq!(codes, vec!["view_refused"]);
    assert_eq!(screen.screen_state()["bar"], 0.3);
    assert_eq!(screen.screen_state()["weights"], json!({}));
}

#[test]
fn two_app_touches_inside_the_interval_ask_once() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // S-032: a call that falls into the interval is DISCARDED, not made up -- the judge
    // sees the situation in the next pass with an app touch after the interval.
    let mut screen = Screen::new(params("on"));
    screen.write(note("n1", "0.9"), 100_000);
    assert!(question(&screen).is_some(), "the first touch asks");
    screen.write(note("n2", "0.9"), 101_000);
    assert!(
        question(&screen).is_none(),
        "1000 ms later: inside the interval"
    );
    screen.pass(json!({"kind": "stroke"}), 110_000);
    assert!(
        question(&screen).is_none(),
        "a stroke is no app touch (§ 4.3)"
    );
    screen.write(note("n3", "0.9"), 111_000);
    assert!(
        question(&screen).is_some(),
        "an app touch after the interval asks again"
    );
}
