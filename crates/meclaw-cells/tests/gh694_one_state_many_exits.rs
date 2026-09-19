//! One screen state, many exits (`display-hive.md` § 6): every entry of the `screens`
//! setting is a page of the SAME state, the root of that page wears the profile of the
//! exit it is rendered for, and the routes are written only when the exits change.
//!
//! Two sentences this file used to pin are struck. The default output is no longer the
//! one WITHOUT a prefix: every named exit has its own copy (`tv.display.root`,
//! `desk.display.root`), and the unprefixed tree is the SWITCH at `/` -- it draws the
//! `default_screen`'s profile, says `data-switch="1"` and carries the names the client
//! needs to lead on (§ 6.5). And the root's word for its output is `exit` now, which is
//! the TYPE (`tv`/`monitor`/`phone`, what the sheet selects on); the NAME stands beside
//! it as `screen_name`. `screen`, `profile` and `dock_overflow` are gone.
//!
//! The script runs the way a `code` cell runs it: as a subprocess, one pass at a time,
//! with the state row and the held tree carried between the passes (`support::Screen`).

mod support;

use meclaw_core::serde_json::{Value, json};
use support::{Screen, component_view, library_ships, pages, pane};

fn note(view_id: &str, relevance: &str) -> Value {
    component_view(
        view_id,
        "main",
        pane(
            view_id,
            json!({"context": "conversation", "relevance": relevance}),
        ),
    )
}

/// One screen state, N exits. Every exit is a prefixed copy of the one tree with its own
/// profile at the root; `/` is the switch and draws the default's.
#[test]
fn every_exit_is_a_page_of_the_same_state() {
    if !library_ships() {
        return;
    }
    let mut screen = Screen::new(json!({"screens": {
        "tv": {"display_type": "tv", "viewing_distance_m": 3.0, "inputs": ["audio"]},
        "desk": {"display_type": "monitor", "viewing_distance_m": 0.7,
                 "inputs": ["pointer", "keyboard"]},
    }, "default_screen": "tv"}));
    let calls = screen.write(note("a", "0.8"), 1000);

    let set = pages(&calls);
    for (route, root) in [
        ("/", "display.root"),
        ("/tv", "tv.display.root"),
        ("/desk", "desk.display.root"),
    ] {
        assert!(
            set.contains(&(route.to_string(), root.to_string())),
            "{route} is a page of this screen: {set:?}"
        );
    }

    let switch = screen.props("display.root").expect("the switch");
    let tv = screen.props("tv.display.root").expect("the television");
    let desk = screen.props("desk.display.root").expect("the monitor");
    assert_eq!(switch["switch"], "1", "`/` is the switch (§ 6.5): {switch}");
    assert_eq!(tv["switch"], "", "a named exit is not: {tv}");
    assert_eq!(desk["switch"], "");
    assert_eq!(
        switch["screen_name"], "tv",
        "and it is drawn as the default output: {switch}"
    );
    assert_eq!(switch["exit"], tv["exit"], "with the default's profile");
    assert_eq!(switch["scale"], tv["scale"]);
    // What makes it a switch rather than a fourth screen: it knows the names.
    for root in [&switch, &tv, &desk] {
        assert_eq!(root["default_screen"], "tv", "{root}");
        assert_eq!(
            root["screens"], "[\"desk\", \"tv\"]",
            "every root names the outputs: {root}"
        );
    }

    assert_eq!(tv["exit"], "tv", "the KIND of display at the root: {tv}");
    assert_eq!(tv["screen_name"], "tv", "and the name beside it: {tv}");
    assert_eq!(tv["scale"], "1.6", "a television is scaled up: {tv}");
    assert_eq!(tv["inputs"], "", "§ 4.7: a television takes nothing: {tv}");
    assert_eq!(tv["dock_max"], 7, "and carries the type's dock: {tv}");
    assert!(
        tv["client_js"]
            .as_str()
            .is_some_and(|js| js.contains("DisplayScene: hook")),
        "the shell script is the scene hook: {}",
        tv["client_js"]
    );
    assert_eq!(desk["exit"], "monitor", "{desk}");
    assert_eq!(
        desk["screen_name"], "desk",
        "the name is the member's word, the kind is the screen's: {desk}"
    );
    assert_eq!(desk["scale"], "1.0", "{desk}");
    assert_eq!(
        desk["inputs"], "pointer keyboard",
        "as the profile said them"
    );
    assert_eq!(desk["dock_max"], 8);
    assert_eq!(
        desk["client_js"], tv["client_js"],
        "the second exit carries the same motion"
    );

    // The same windows, the same dock, on every exit -- the state ran once (§ 3.1).
    for prefix in ["", "tv.", "desk."] {
        assert!(
            screen.holds(&format!("{prefix}view.alex.a/c.a")),
            "{prefix} draws the window: {:?}",
            screen.held
        );
        assert!(
            screen.holds(&format!("{prefix}display.dock")),
            "{prefix} draws the dock"
        );
    }
    // The KIND of display is said once, at the root (§ 6.1: `data-exit`), and
    // every exit's tree says its own. The dock used to carry a copy of it in a
    // prop no selector read; display 2.5.0 took the copy away, because a value
    // in two places is a value that can disagree with itself.
    for (prefix, kind) in [("tv", "tv"), ("desk", "monitor")] {
        assert_eq!(
            screen
                .props(&format!("{prefix}.display.root"))
                .expect("the root")["exit"],
            kind,
            "{prefix} is drawn as a {kind}"
        );
        let dock = screen
            .props(&format!("{prefix}.display.dock"))
            .expect("dock");
        assert_eq!(
            dock["profile"],
            Value::Null,
            "the dock says the type a second time: {dock}"
        );
    }

    // The routes go LAST: `page.set` refuses a root that does not exist yet.
    let last = calls.last().expect("calls");
    assert_eq!(
        last["op"], "page.set",
        "the routes are set after the objects: {last}"
    );
}

/// The routes are written once, and again only when the exits change.
#[test]
fn the_routes_are_not_rewritten_every_tick() {
    if !library_ships() {
        return;
    }
    let mut screen = Screen::new(
        json!({"screens": {"tv": {"display_type": "tv", "viewing_distance_m": 3.0}},
               "default_screen": "tv"}),
    );
    screen.pass(json!({"kind": "stroke"}), 1000);
    let quiet = screen.pass(json!({"kind": "stroke"}), 2000);
    assert!(
        pages(&quiet).is_empty(),
        "a quiet tick sets no page: {quiet:?}"
    );

    screen.params = json!({"screens": {
        "tv": {"display_type": "tv", "viewing_distance_m": 3.0},
        "hand": {"display_type": "phone", "viewing_distance_m": 0.35, "inputs": ["touch"]},
    }, "default_screen": "tv"});
    let grown = screen.pass(json!({"kind": "stroke"}), 3000);
    let set = pages(&grown);
    assert!(
        set.contains(&("/hand".to_string(), "hand.display.root".to_string())),
        "a new exit is routed when the setting changes: {set:?}"
    );
}

/// With no `screens` of its own the screen is one television at three metres -- and
/// nothing else is guessed: a profile without `display_type` and a `default_screen` that
/// names no entry are ERRORS the screen says out loud (§ 4.7), not a silent television.
#[test]
fn the_shipped_screen_is_a_television() {
    if !library_ships() {
        return;
    }
    let mut screen = Screen::new(json!({"default_screen": "tv"}));
    screen.pass(json!({"kind": "stroke"}), 1000);
    let root = screen.props("tv.display.root").expect("the shipped exit");
    assert_eq!(root["exit"], "tv");
    assert_eq!(root["screen_name"], "tv");
    assert_eq!(root["scale"], "1.6");
    assert_eq!(root["inputs"], "");
    assert_eq!(root["dock_max"], 7);
    assert_eq!(
        root["screens"], "[\"tv\"]",
        "the root names its exits: {root}"
    );
    assert_eq!(
        pages(&screen.pass(json!({"kind": "stroke"}), 2000)),
        Vec::new(),
        "and a second tick changes no route"
    );

    let mut broken = Screen::new(
        json!({"screens": {"z": {"display_type": "phone"}, "b": {}}, "default_screen": "nope"}),
    );
    broken.pass(json!({"kind": "stroke"}), 1000);
    let said: Vec<String> = broken
        .lane("receipt")
        .iter()
        .map(|e| {
            e["receipt"]["error_code"]
                .as_str()
                .unwrap_or("")
                .to_string()
        })
        .collect();
    assert!(
        said.iter().any(|c| c == "profile_error"),
        "a profile without `display_type` is refused, not made a television: {said:?}"
    );
    assert!(
        said.iter().any(|c| c == "setting_error"),
        "and a `default_screen` that names no exit is refused too: {said:?}"
    );
}

/// A change of `default_screen` alone -- the exits unchanged -- leaves every named route
/// where it is (§ 6.5) and re-draws the SWITCH as the new default output.
#[test]
fn a_new_default_screen_moves_only_the_switch() {
    if !library_ships() {
        return;
    }
    let screens = json!({
        "tv": {"display_type": "tv", "viewing_distance_m": 3.0},
        "desk": {"display_type": "monitor", "viewing_distance_m": 0.7, "inputs": ["pointer"]},
    });
    let mut screen = Screen::new(json!({"screens": screens, "default_screen": "tv"}));
    screen.pass(json!({"kind": "stroke"}), 1000);
    assert_eq!(screen.props("display.root").expect("switch")["exit"], "tv");

    screen.params = json!({"screens": screens, "default_screen": "desk"});
    let second = screen.pass(json!({"kind": "stroke"}), 2000);

    let switch = screen.props("display.root").expect("the switch");
    assert_eq!(
        switch["screen_name"], "desk",
        "the switch follows the default: {switch}"
    );
    assert_eq!(switch["exit"], "monitor", "{switch}");
    assert_eq!(switch["default_screen"], "desk", "and says so: {switch}");
    assert_eq!(switch["switch"], "1", "and stays the switch");
    assert_eq!(
        screen.props("tv.display.root").expect("the television")["exit"],
        "tv",
        "the television's own page did not move"
    );
    let set = pages(&second);
    assert!(
        set.is_empty()
            || set.contains(&("/tv".to_string(), "tv.display.root".to_string()))
                && set.contains(&("/desk".to_string(), "desk.display.root".to_string())),
        "no named route changed its root: {set:?}"
    );
}
