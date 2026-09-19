//! display-hive.md § 4.6/§ 4.7: settings and profiles pass the SAME door as a view, and the
//! door says out loud what it would not take. A value it replaces is refused ONCE -- in the
//! pass that replaces it, because the replacement is written into the state and the next
//! pass reads it back from the state row. `display_type missing` and a `default_screen` that
//! names no output are errors and are reported in EVERY pass: nothing replaces them.
//!
//! Scenarios: S-078 (settings), S-079 (profile dials), S-039 (errors), Q-11 (a television
//! has no inputs, whatever its profile says).

mod support;

use meclaw_core::serde_json::{Value, json};
use support::{Screen, component_view, library_ships, pane};

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

/// The `error_code`s of the receipts this pass emitted, in order.
fn receipts(screen: &Screen) -> Vec<String> {
    screen
        .lane("receipt")
        .iter()
        .map(|e| {
            e["receipt"]["error_code"]
                .as_str()
                .unwrap_or("")
                .to_string()
        })
        .collect()
}

fn details(screen: &Screen) -> Vec<String> {
    screen
        .lane("receipt")
        .iter()
        .map(|e| e["receipt"]["detail"].as_str().unwrap_or("").to_string())
        .collect()
}

#[test]
fn a_setting_the_door_replaces_is_refused_once() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // S-078: `fade_ms: 0` and `focus_default: 1.5` are not values of § 3. The defaults
    // 120000 and 0.3 stand, the pass RUNS, and the decay falls over the default fade.
    let mut screen = Screen::new(json!({
        "fade_ms": 0, "focus_default": 1.5,
        "screens": {"monitor": {"display_type": "monitor", "inputs": ["pointer"]}},
        "default_screen": "monitor",
    }));
    screen.write(note("n1", "1.0"), 100_000);
    assert_eq!(
        receipts(&screen),
        vec!["setting_refused", "setting_refused"]
    );
    assert!(
        details(&screen).iter().any(|d| d.contains("fade_ms")),
        "the receipt names the setting: {:?}",
        details(&screen)
    );
    assert_eq!(screen.screen_state()["bar"], 0.3);

    // The second pass reads the replaced values back off the state row: nothing left to
    // refuse, and the decay of § 4.15 runs over 120000 ms.
    screen.pass(json!({"kind": "stroke"}), 130_000);
    assert!(
        receipts(&screen).is_empty(),
        "refused once, in the pass that replaces it: {:?}",
        receipts(&screen)
    );
    let decay = screen.curator("view.alex.n1", "decay").as_f64().unwrap();
    assert!(
        (decay - 0.9167).abs() < 0.001,
        "the decay falls over the DEFAULT fade, not over 0: {decay}"
    );
}

#[test]
fn a_profile_dial_the_door_replaces_is_refused_once_and_the_cut_follows() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // S-079: `dock_max: "2"` is the number 2 (§ 3.3, a number may travel as text);
    // `dock_default: "open"` is not a word of § 3, so the type default `hidden` stands
    // and the value is refused once.
    let mut screen = Screen::new(json!({
        "screens": {"phone": {"display_type": "phone", "inputs": ["touch", "keyboard", "audio"],
                              "dock_max": "2", "dock_default": "open"}},
        "default_screen": "phone",
    }));
    screen.write(note("n1", "0.9"), 100_000);
    assert_eq!(receipts(&screen), vec!["profile_refused"]);
    assert!(
        details(&screen)[0].contains("dock_default") && details(&screen)[0].contains("open"),
        "the receipt names the profile and the dial: {:?}",
        details(&screen)
    );
    let root = screen
        .props("phone.display.root")
        .expect("the phone's root");
    assert_eq!(root["dock_max"], 2, "\"2\" is the number two");
    assert_eq!(root["dock"], "hidden", "the type default stands");

    screen.write(note("n2", "0.8"), 100_000);
    screen.write(note("n3", "0.7"), 100_000);
    assert!(
        receipts(&screen).is_empty(),
        "refused once: {:?}",
        receipts(&screen)
    );
    let drawn = screen
        .held
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| {
            o["component"] == "display-tile" && o["id"].as_str().unwrap().starts_with("phone.")
        })
        .count();
    assert_eq!(
        drawn, 2,
        "`dock_max` is the maximum, without exception (§ 4.30)"
    );
}

#[test]
fn a_profile_without_a_kind_is_an_error_in_every_pass() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // S-039: `display_type` is mandatory. A missing one is an ERROR, not a silent
    // television, and nothing replaces it -- so it is reported again in every pass.
    let mut screen = Screen::new(json!({
        "screens": {"tv": {"display_type": "tv"}, "odd": {"viewing_distance_m": 1.0}},
        "default_screen": "tv",
    }));
    screen.pass(json!({"kind": "stroke"}), 100_000);
    assert_eq!(receipts(&screen), vec!["profile_error"]);
    assert!(
        details(&screen)[0].contains("display_type missing"),
        "{:?}",
        details(&screen)
    );
    // § 4.7 keeps the error in the STATE of every pass -- nothing replaces it, so the
    // door finds it again -- and the state row says so.
    assert_eq!(
        screen.screen_state()["said"],
        json!([["error", "screen", "odd", "display_type missing"]]),
        "the error stands in the state of this pass"
    );

    // But the RECEIPT is a message, and a message is said once per value: a profile
    // nobody fixed would otherwise send one per pass, for ever, to nobody (a screen
    // refusal has no owner and dead-letters).
    screen.pass(json!({"kind": "stroke"}), 101_000);
    assert!(
        receipts(&screen).is_empty(),
        "the same error is not said twice: {:?}",
        receipts(&screen)
    );
    assert_eq!(
        screen.screen_state()["said"],
        json!([["error", "screen", "odd", "display_type missing"]]),
        "and it is still true in the state"
    );

    // A SECOND broken profile is a new value, so it is said.
    screen.params["screens"]["also"] = json!({"viewing_distance_m": 2.0});
    screen.pass(json!({"kind": "stroke"}), 102_000);
    assert_eq!(
        receipts(&screen),
        vec!["profile_error"],
        "a value the screen has not said yet is said"
    );
    assert!(
        details(&screen)[0].contains("also"),
        "{:?}",
        details(&screen)
    );
}

#[test]
fn a_profile_the_door_refused_is_still_drawn_and_still_cut() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // OR-H1.16. § 4.7 says what a profile without `display_type` is -- an error, reported
    // every pass, and the profile stays raw -- and says nothing about what the output then
    // SHOWS. The screen still draws it, because a member who cannot see the screen cannot
    // read the error either; it draws it with the fallback kind, and it BINDS nothing,
    // because the kind is unknown and a promise of a finger would be made up. And it is
    // cut like any other output: § 4.30 says `dock_max` holds without exception, so the
    // type default of the fallback stands in for the number the profile never named.
    let mut screen = Screen::new(json!({
        "screens": {"monitor": {"display_type": "monitor", "inputs": ["pointer"]},
                    "odd": {"viewing_distance_m": 1.0, "inputs": ["touch", "keyboard"]}},
        "default_screen": "monitor",
    }));
    for i in 0..8 {
        screen.write(note(&format!("n{i}"), "0.5"), 100_000);
    }
    let root = screen
        .props("odd.display.root")
        .expect("the odd output is drawn");
    assert_eq!(root["exit"], "tv", "the fallback kind");
    assert_eq!(root["screen_name"], "odd");
    assert_eq!(
        root["inputs"], "",
        "nothing is bound on a profile the door refused"
    );
    assert_eq!(root["tap"], false);
    assert_eq!(root["input_line"], false);
    assert_eq!(root["dock_max"], 7, "the fallback kind's number");
    let drawn = |exit: &str| -> usize {
        screen
            .held
            .as_array()
            .unwrap()
            .iter()
            .filter(|o| {
                o["component"] == "display-tile"
                    && o["id"]
                        .as_str()
                        .unwrap()
                        .starts_with(&format!("{exit}.display.dock/"))
            })
            .count()
    };
    assert_eq!(drawn("monitor"), 8, "a monitor draws eight");
    assert_eq!(
        drawn("odd"),
        7,
        "and the refused profile is cut like any other"
    );
}

#[test]
fn a_hint_that_is_not_on_the_wire_is_refused_and_the_screen_lives() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // § 3.3 is a WIRE contract, not a formatting preference: hints that are numbers
    // travel as text, `pinned` is the one boolean, and everything else a hint says is
    // text. The reason the door has to hold it is the pass itself -- the pass is the
    // reference model, and a list where it expects a word does not come back there as a
    // refusal, it raises (`weight_of`: `ctx not in weights` on an unhashable value). So
    // the write lane answers a receipt the sender can read, and the screen runs on.
    let write = |props: Value| -> (Vec<String>, Vec<String>) {
        let doc = json!({
            "params": {},
            "body": {"view_id": "n1", "region": "main", "kind": "component",
                     "content": {"component": "display-pane", "key": "c.n1",
                                 "props": props},
                     "messages": []},
            "envelope": {"reply_to": "members/alex/apps/notes",
                         "header": {"hop": {"route": "in_view"}, "context": {}}},
        });
        let emissions = support::raw(&doc);
        (
            emissions
                .iter()
                .filter(|e| e["header"]["route"] == "receipt")
                .map(|e| {
                    e["receipt"]["error_code"]
                        .as_str()
                        .unwrap_or("")
                        .to_string()
                })
                .collect(),
            emissions
                .iter()
                .filter(|e| e["header"]["route"] == "receipt")
                .map(|e| e["receipt"]["detail"].as_str().unwrap_or("").to_string())
                .collect(),
        )
    };

    let (codes, details) = write(json!({"context": ["a"], "title": "N1"}));
    assert_eq!(
        codes,
        vec!["view_refused"],
        "a list is not a context: {details:?}"
    );
    assert!(details[0].contains("context"), "{details:?}");

    // A number in a numeric hint is refused, not coerced (OR-H1.19): § 3.3 names the
    // wire because the template language reads an `int 0` as empty, so a number here
    // would arrive as "nothing said" further down.
    let (codes, details) = write(json!({"relevance": 0.5, "title": "N1"}));
    assert_eq!(codes, vec!["view_refused"], "{details:?}");
    assert!(details[0].contains("text"), "{details:?}");

    let (codes, _) = write(json!({"pinned": "yes", "title": "N1"}));
    assert_eq!(codes, vec!["view_refused"], "`pinned` is the one boolean");

    // And the shape that IS the wire goes through: the write reaches the store.
    let doc = json!({
        "params": {},
        "body": {"view_id": "n1", "region": "main", "kind": "component",
                 "content": {"component": "display-pane", "key": "c.n1",
                             "props": {"context": "system", "relevance": "0.9",
                                       "pinned": true, "touched": "100000",
                                       "title": "N1"}},
                 "messages": []},
        "envelope": {"reply_to": "members/alex/apps/notes",
                     "header": {"hop": {"route": "in_view"}, "context": {}}},
    });
    let emissions = support::raw(&doc);
    assert!(
        emissions.iter().all(|e| e["header"]["route"] != "receipt"),
        "the wire of § 3.3 is taken: {emissions:?}"
    );
    assert_eq!(
        emissions
            .iter()
            .filter(|e| e["header"]["route"] == "views")
            .count(),
        1,
        "and it reaches the store"
    );
}

#[test]
fn an_absorbed_gesture_is_no_refusal_to_anybody() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // § 5.4 and § 6.12 (S-017, S-053): a hold with no chat view and a tap on a window
    // with no tile are ABSORBED -- nothing opens, no error. The pass writes them down so
    // a scenario can read them (`refused()`), and the screen says nothing out loud: a
    // receipt there is noise on the `receipt` lane of an app that did nothing wrong.
    let mut screen = Screen::new(json!({
        "screens": {"monitor": {"display_type": "monitor", "inputs": ["pointer"]}},
        "default_screen": "monitor",
    }));
    screen.write(note("n1", "0.9"), 100_000);
    assert!(receipts(&screen).is_empty());

    screen.pass(json!({"kind": "hold"}), 101_000);
    assert!(
        receipts(&screen).is_empty(),
        "a hold with no chat window is absorbed: {:?}",
        receipts(&screen)
    );
    assert_eq!(
        screen.screen_state()["said"],
        json!([["refused", "hold", "absorbed", "no chat view"]]),
        "and the pass wrote it down all the same"
    );

    screen.pass(json!({"kind": "tap", "for": "view.alex.gone"}), 102_000);
    assert!(
        receipts(&screen).is_empty(),
        "a tap on a window with no tile is absorbed: {:?}",
        receipts(&screen)
    );
    assert_eq!(
        screen.screen_state()["said"],
        json!([["refused", "view.alex.gone", "tap", "no tile"]])
    );

    // The door's own refusals are NOT absorbed: they reach the sender.
    screen.params["fade_ms"] = json!(0);
    screen.pass(json!({"kind": "stroke"}), 103_000);
    assert_eq!(receipts(&screen), vec!["setting_refused"]);
}

#[test]
fn a_television_has_no_inputs_whatever_its_profile_says() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // Q-11: a type rule, not a per-profile choice -- a television takes no taps and no
    // keyboard, so a `pointer` in its profile is dropped at the door. A profile that names
    // no `inputs` has none either, a phone included.
    let mut screen = Screen::new(json!({
        "screens": {"tv": {"display_type": "tv", "inputs": ["pointer"]},
                    "phone": {"display_type": "phone"},
                    "monitor": {"display_type": "monitor", "inputs": ["pointer", "keyboard"]}},
        "default_screen": "tv",
    }));
    screen.pass(json!({"kind": "stroke"}), 100_000);
    assert_eq!(screen.props("tv.display.root").unwrap()["inputs"], "");
    assert_eq!(screen.props("tv.display.root").unwrap()["tap"], false);
    assert_eq!(screen.props("phone.display.root").unwrap()["inputs"], "");
    assert_eq!(
        screen.props("phone.display.root").unwrap()["input_line"],
        false
    );
    let monitor = screen.props("monitor.display.root").unwrap();
    assert_eq!(monitor["inputs"], "pointer keyboard");
    assert_eq!(monitor["tap"], true);
    assert_eq!(monitor["input_line"], true);
    assert!(receipts(&screen).is_empty(), "{:?}", receipts(&screen));
}
