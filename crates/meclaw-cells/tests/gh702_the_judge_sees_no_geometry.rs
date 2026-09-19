//! display-hive.md § 4.4 / § 4.7 -- the judge sees no geometry, and a profile carries its
//! own dials.
//!
//! Two halves of one ruling (R-23-6). The judge decides ONE state for every output at
//! once, so a number that differs per output is not a fact it can act on: which output a
//! tree happens to be rendered for, where a window stands on that output, whether it has
//! a tile and how it ranks in the dock all leave the situation and the instructions
//! (OR-F11). What stays is what a `hidden` COSTS -- that a hidden window keeps its tile is
//! a statement about the verdict, not about geometry. The other half: the cut of the dock
//! and whether the dock is drawn at all move into `screens.<name>`, so each output answers
//! them for itself.
//!
//! What no longer holds, and is asserted against below: `canvas_slots` and a root-level
//! `dock_max` in params are struck, the root says `exit` (the TYPE) rather than `profile`,
//! and a profile without `display_type` is an ERROR, not a silent television (§ 4.7).

mod support;

use meclaw_core::serde_json::{Value, json};
use support::{COMPOSE, Screen, component_view, library_ships, pane, repo, window_id};

/// A screen with a judge and one output, the shape every judge lock of this wave uses.
fn params() -> Value {
    json!({"judge": "on", "judge_min_interval_ms": 3000,
           "screens": {"monitor": {"display_type": "monitor", "inputs": ["pointer"]}},
           "default_screen": "monitor"})
}

/// What the judge was shown in the last pass, or none when it was not asked.
fn question(screen: &Screen) -> Option<Value> {
    let asked = screen.lane("judge");
    assert!(asked.len() <= 1, "at most one question a pass");
    asked.first().map(|e| {
        meclaw_core::serde_json::from_str(e["messages"][0]["text"].as_str().expect("the payload"))
            .expect("the payload is JSON")
    })
}

/// The judge decides one state for every output at once, so it is shown no geometry:
/// neither which output this tree happens to be rendered for, nor anything the pass
/// computed per output (R-23-6, OR-F11).
#[test]
fn the_situation_carries_neither_the_exit_nor_the_overflow() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(params());
    screen.write(
        component_view(
            "a",
            "main",
            pane(
                "a",
                json!({"title": "Chat", "context": "conversation", "relevance": "0.8",
                       "topic": "chat"}),
            ),
        ),
        100_000,
    );
    let seen = question(&screen).expect("an app touch asks the judge");
    // Neither on the state nor on a window: every one of these was either a per-output
    // number or a place on a screen, and the verdict is about neither.
    for gone in ["screen", "dock_overflow", "dock_max", "preferences", "now"] {
        assert!(
            seen.get(gone).is_none(),
            "`{gone}` is no longer shown: {seen}"
        );
    }
    let window = seen["windows"][0].clone();
    for gone in [
        "rank",
        "tile",
        "level",
        "on_canvas",
        "topic_dupe",
        "dismissed_at",
        "region",
        "touched",
        "view_id",
    ] {
        assert!(
            window.get(gone).is_none(),
            "`{gone}` is geometry or bookkeeping and left the situation: {window}"
        );
    }
    // What does stay of the ladder: the rung the window reached, and whether it LEADS its
    // ladder -- which is `rung == focus` and nothing about a canvas.
    assert_eq!(window["id"], window_id("alex", "a").as_str());
    assert_eq!(window["rung"], "relevant", "{window}");
    assert_eq!(
        window["leads"], false,
        "it is not the one in focus: {window}"
    );
    assert_eq!(
        window["relevance"], 0.8,
        "the hint as the door read it: {window}"
    );
    assert_eq!(window["verdict"]["judged_hidden"], Value::Null, "{window}");
}

/// And the instructions lose the sentence about the dock's geometry. Read out of the
/// INSTRUCTION BLOCK and not out of the whole file: `dock_max` is a profile dial and a
/// root prop and stands in `compose.py` eighteen times; the question here is only what the
/// judge is TOLD (§ 4.4), not what the script knows.
#[test]
fn the_instructions_say_nothing_about_the_dock_being_full() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let script = std::fs::read_to_string(repo(COMPOSE)).expect("the script");
    let instructions = script
        .split_once("JUDGE_INSTRUCTIONS = \"\"\"")
        .expect("the instructions are in the script")
        .1
        .split_once("\"\"\"")
        .expect("and they end")
        .0;
    assert!(
        !instructions.contains("The dock on the right shows everything that is present"),
        "the geometry sentence is gone: {instructions}"
    );
    // The two words the geometry was made of. Neither is a fact the judge can act on --
    // both are per output, and it decides one state for all of them.
    assert!(
        !instructions.contains("overflow"),
        "and nothing tells it what did not fit: {instructions}"
    );
    assert!(
        !instructions.contains("dock_max"),
        "nor how large the dock is: {instructions}"
    );
    assert!(
        instructions.contains(
            "The dock shows everything that is present; your verdict decides only what stands LARGE."
        ),
        "and the one that stays says only what the verdict is about: {instructions}"
    );
    // What a `hidden` COSTS is not geometry and stays: without it the judge does not know
    // that hiding a window leaves its tile standing.
    assert!(
        instructions.contains(
            "A window you hide keeps its tile, so hiding costs the person nothing but the space."
        ),
        "and the judge still knows what hiding costs: {instructions}"
    );
}

/// The dials of a profile reach the root of the output they belong to (§ 4.7).
///
/// Three outputs and not two: the numbers an entry does NOT say are the third case, and
/// they are the one a single output cannot measure. `wohnzimmer` says nothing at all and
/// is of another kind than the default output, so every dial it wears has to come out of
/// ITS kind -- a phone default that hands its five tiles to a television is exactly the
/// silent fallback this asserts against.
#[test]
fn a_profile_carries_its_own_dials() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(json!({
        "default_screen": "handy",
        "screens": {
            "handy": {"display_type": "phone", "viewing_distance_m": 0.35,
                      "inputs": ["audio", "touch"]},
            "buero": {"display_type": "monitor", "viewing_distance_m": 0.7,
                      "dock_max": 2, "dock_default": "hidden",
                      "inputs": ["audio", "pointer", "keyboard"]},
            "wohnzimmer": {"display_type": "tv", "viewing_distance_m": 3.0,
                           "inputs": ["audio"]},
        },
    }));
    screen.pass(json!({"kind": "stroke"}), 1000);

    let root = screen
        .props("display.root")
        .expect("the default output is drawn");
    assert_eq!(
        root["exit"], "phone",
        "the root says the TYPE, not a profile name: {root}"
    );
    assert_eq!(root["screen_name"], "handy", "{root}");
    assert_eq!(root["dock_max"], 5, "a phone carries five tiles: {root}");
    assert_eq!(root["dock"], "hidden", "and its dock starts closed: {root}");
    // `canvas_slots` is struck (§ "What no longer holds"): how many windows stand large is
    // the pass's own answer (§ 4.24), not a number an output hands out.
    assert!(root.get("canvas_slots").is_none(), "{root}");

    // A number the kind does not hand out anyway (a monitor's own default is eight): an
    // entry's word has to be READ, not merely agreed with.
    let other = screen
        .props("buero.display.root")
        .expect("the second output");
    assert_eq!(other["exit"], "monitor", "{other}");
    assert_eq!(other["dock_max"], 2, "the entry's own word wins: {other}");
    assert_eq!(other["dock"], "hidden", "{other}");

    // And the silent output: every dial out of its own kind, none out of the default
    // output's. A television carries seven tiles and its dock is drawn from the start --
    // the phone beside it decides none of that. Its `inputs` are emptied whatever the
    // profile said, because a television is what a television is (§ 4.7).
    let tv = screen
        .props("wohnzimmer.display.root")
        .expect("the third output");
    assert_eq!(tv["exit"], "tv", "{tv}");
    assert_eq!(
        tv["dock_max"], 7,
        "a television carries seven tiles, whatever kind the default output is: {tv}"
    );
    assert_eq!(tv["dock"], "shown", "{tv}");
    assert_eq!(tv["inputs"], "", "a television has no input at all: {tv}");
    assert_eq!(tv["tap"], false, "so nothing on it is tappable: {tv}");
}

/// A profile without `display_type` is an ERROR and stands as one in the state of every
/// pass -- it is not quietly read as a television (§ 4.7). The receipt is the whole point:
/// a member who names an output the screen cannot type has to be able to read that. The
/// MESSAGE is said once per value, because a screen refusal has no owner and one
/// misconfigured profile would otherwise dead-letter one receipt per pass, for ever; what
/// § 4.7 keeps in every pass is the error itself, and the state row carries it.
#[test]
fn a_profile_without_a_type_is_an_error_and_not_a_silent_television() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(json!({
        "default_screen": "kueche",
        "screens": {"kueche": {"viewing_distance_m": 1.0, "inputs": ["touch"]}},
    }));
    screen.pass(json!({"kind": "stroke"}), 1000);
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
    assert_eq!(codes, vec!["profile_error"], "{codes:?}");
    assert_eq!(
        screen.screen_state()["said"],
        json!([["error", "screen", "kueche", "display_type missing"]]),
        "the error stands in the state of this pass"
    );
    // Nothing replaces a missing type, so the door finds it again in the next pass and it
    // stays true in the state -- and the screen does not say the same thing twice.
    screen.pass(json!({"kind": "stroke"}), 2000);
    assert!(
        screen.lane("receipt").is_empty(),
        "the same error is not said twice"
    );
    assert_eq!(
        screen.screen_state()["said"],
        json!([["error", "screen", "kueche", "display_type missing"]]),
        "and it is still an error of this pass"
    );
}

/// Two urgent windows are the only shape that tells the rung and the level apart: both
/// stand at rung `urgent`, but only the loudest is drawn large (OR-F23), so the other one
/// is at level 0. The judge is shown the rung of both and the level of neither -- how many
/// windows an output draws is geometry it never sees (R-23-6, OR-F24, OR-F44). A reader
/// that asked the level here would tell the judge the second alarm was already put away,
/// and the next verdict would weigh a ringing window as a closed one.
#[test]
fn both_urgent_windows_reach_the_judge_without_their_level() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(params());
    screen.write(
        component_view(
            "old",
            "main",
            pane(
                "old",
                json!({"title": "Old", "context": "ambient", "state": "urgent"}),
            ),
        ),
        100_000,
    );
    // Past `judge_min_interval_ms`, so this pass asks again and sees both windows.
    screen.write(
        component_view(
            "new",
            "main",
            pane(
                "new",
                json!({"title": "New", "context": "system", "state": "urgent"}),
            ),
        ),
        104_000,
    );
    let seen = question(&screen).expect("an app touch after the interval asks the judge");
    let windows = seen["windows"].as_array().expect("windows");
    assert_eq!(windows.len(), 2, "{seen}");
    for w in windows {
        assert_eq!(w["rung"], "urgent", "the rung reaches the judge: {w}");
        assert!(w.get("level").is_none(), "and the level does not: {w}");
        assert!(
            w.get("front").is_none(),
            "nor which of them is in front: {w}"
        );
    }
    // The other half of the measurement: on the screen the two really do stand at
    // different levels -- so the absence above is an absence, not two windows that happen
    // to agree.
    let level = |view: &str| -> Value {
        screen
            .props(&format!("{}/c.{view}", window_id("alex", view)))
            .unwrap_or_else(|| panic!("{view} stands"))["level"]
            .clone()
    };
    assert_eq!(level("new"), "3", "the front alarm stands large");
    assert_eq!(
        level("old"),
        "0",
        "the one behind it is a tile and nothing else"
    );
}
