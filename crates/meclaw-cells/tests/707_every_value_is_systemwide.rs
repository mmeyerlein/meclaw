//! display-hive.md § 3.1: there is exactly ONE screen state per member, and all its values
//! are systemwide -- no value depends on the output. The rendering per output (§ 6) is no
//! part of it: what an output decides is its own, and that is the dock's cut alone (§ 4.30).
//!
//! So: three outputs, two open canvas windows. On every one of them the same window objects
//! carry the same `data-rung` and the same `data-level`; only how many tiles are drawn
//! differs (S-059, Q-14).

mod support;

use meclaw_core::serde_json::{Value, json};
use support::{Screen, component_view, library_ships, pane, window_id};

fn params() -> Value {
    json!({"screens": {
        "tv": {"display_type": "tv", "viewing_distance_m": 3.0},
        "monitor": {"display_type": "monitor", "inputs": ["pointer", "keyboard"]},
        "phone": {"display_type": "phone", "inputs": ["touch", "keyboard", "audio"],
                  "dock_max": 2},
    }, "default_screen": "monitor"})
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

#[test]
fn every_output_draws_the_same_rungs_and_levels() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(params());
    screen.write(note("n1", "0.9"), 100_000);
    screen.write(note("n2", "0.8"), 100_000);
    // The second pass: a fresh window takes no focus in the pass it appears in (§ 4.35).
    screen.pass(json!({"kind": "stroke"}), 101_000);

    for view_id in ["n1", "n2"] {
        let oid = window_id("alex", view_id);
        let window = format!("{oid}/c.{view_id}");
        let mut seen: Vec<(String, String)> = Vec::new();
        for exit in ["tv", "monitor", "phone"] {
            let props = screen
                .props(&format!("{exit}.{window}"))
                .unwrap_or_else(|| panic!("{exit} draws {window}"));
            seen.push((
                props["rung"].as_str().unwrap_or("").to_string(),
                props["level"].as_str().unwrap_or("").to_string(),
            ));
        }
        assert_eq!(seen[0], seen[1], "{view_id}: tv and monitor disagree");
        assert_eq!(seen[1], seen[2], "{view_id}: monitor and phone disagree");
        assert_eq!(
            seen[0].1, "1",
            "{view_id} is an open canvas window: level 1"
        );
    }
    // What DOES differ per output: how many tiles it draws (§ 4.30). The phone's profile
    // says two, and the state holds two -- so the difference shows on the count the dock
    // wears, not on any window.
    assert_eq!(
        screen.props("phone.display.dock").unwrap()["dock_max"],
        Value::Null,
        "the cut is the dock's business, the number the root's"
    );
    assert_eq!(screen.props("phone.display.root").unwrap()["dock_max"], 2);
    assert_eq!(screen.props("tv.display.root").unwrap()["dock_max"], 7);
}

#[test]
fn the_count_at_the_mark_is_one_number_for_the_whole_screen() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // § 4.33: `unseen` is systemwide and independent of the dock's cut. A phone that draws
    // fewer tiles does not therefore show a smaller number (S-044).
    let mut screen = Screen::new(params());
    screen.write(note("n1", "0.1"), 100_000);
    screen.write(note("n2", "0.1"), 100_000);
    screen.write(note("n3", "0.1"), 100_000);
    let marks: Vec<String> = ["tv", "monitor", "phone"]
        .iter()
        .map(|exit| {
            screen.props(&format!("{exit}.display.os")).unwrap()["unseen"]
                .as_str()
                .unwrap_or("")
                .to_string()
        })
        .collect();
    assert_eq!(marks[0], marks[1]);
    assert_eq!(marks[1], marks[2]);
    assert_eq!(
        marks[0], "3",
        "three windows inside their linger: {marks:?}"
    );
}
