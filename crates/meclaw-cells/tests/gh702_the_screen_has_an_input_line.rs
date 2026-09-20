//! The one component of the catalogue a person types into (display-hive.md § 7.3).
//!
//! `display-input` is a content component like any other: no glass, no script, three
//! props. It binds on `keyup` with `phx-key="Enter"` and not on a form, because the
//! LiveView client serialises a form to a URL-encoded string and this scope reads object
//! ids out of `event.value` -- a query string is not one, so a submitted form would
//! dead-letter (Befund 03 B.1, OR-F17). And `for` is filled by the SCREEN with the object
//! id of the window the field stands in, the way a tile's `oid` is: the application
//! cannot know that id, it is the index chain the tree walk mints.
//!
//! Whether the line is there at all is the output's business (§ 6.4): only a screen whose
//! profile names `keyboard` or `touch` carries one, and only `pointer` or `touch` binds
//! the tap. Both are one value on the root, systemwide in the state and cut per output.
//!
//! The script runs the way a `code` cell runs it: as a subprocess, with the pass's
//! document on stdin.

mod support;

use std::process::Command;

use meclaw_core::serde_json::{Value, json};
use support::{COMPOSE, Screen, component_view, library_ships, pane_id, repo, view_of};

/// `components()` as the shipped script defines them, asked of the script.
fn probe() -> Vec<Value> {
    let out = Command::new("python3")
        .arg("-c")
        .arg(
            "import importlib.util, json, sys\n\
             spec = importlib.util.spec_from_file_location('compose', sys.argv[1])\n\
             m = importlib.util.module_from_spec(spec)\n\
             spec.loader.exec_module(m)\n\
             print(json.dumps(m.components()))",
        )
        .arg(repo(COMPOSE))
        .output()
        .expect("python3 runs");
    assert!(
        out.status.success(),
        "compose.py did not load:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = meclaw_core::serde_json::from_slice(&out.stdout).expect("components() is JSON");
    v.as_array().expect("a list").clone()
}

fn one(all: &[Value], name: &str) -> Value {
    all.iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("`{name}` is not defined"))
        .clone()
}

/// The catalogue has an input line, and it is a content component like any other.
#[test]
fn the_catalogue_has_an_input_line() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let all = probe();
    assert_eq!(
        all.len(),
        36,
        // Twenty-five since `display@2.6.0` put `display-browser` in the
        // catalogue (GH #767); the count was left at the wave-F number.
        "six the screen owns, five windows, twenty-five contents"
    );
    let input = one(&all, "display-input");
    assert_eq!(input["layer"], "content", "{input}");
    assert_eq!(
        input["editable"],
        json!([]),
        "nothing here is dragged: {input}"
    );
    assert!(
        input["prop_schema"].get("client_js").is_none(),
        "and it brings no script -- exactly two components do: {input}"
    );
    let carries_script: Vec<&str> = all
        .iter()
        .filter(|c| c["prop_schema"].get("client_js").is_some())
        .map(|c| c["name"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(carries_script, ["display-shell", "display-os"], "the two");
    for key in ["placeholder", "event", "for"] {
        assert_eq!(input["prop_schema"][key], "text", "{input}");
    }
}

/// The binding is `keyup` on Enter and not a form: a form serialises to a query string,
/// and the screen's own `event_object_id` reads object ids, so the event would
/// dead-letter (Befund 03 B.1, OR-F17).
#[test]
fn it_binds_on_enter_and_carries_the_window() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let t = one(&probe(), "display-input")["template"]
        .as_str()
        .expect("a template")
        .to_string();
    assert!(t.contains("phx-keyup=\"{{event}}\""), "{t}");
    assert!(t.contains("phx-key=\"Enter\""), "{t}");
    assert!(t.contains("phx-value-for=\"{{for}}\""), "{t}");
    assert!(
        !t.contains("phx-submit") && !t.contains("<form"),
        "no form: {t}"
    );
    assert!(t.contains("class=\"display-input\""), "{t}");
    assert!(t.contains("class=\"display-input-field\""), "{t}");
}

/// The screen fills `for` with the object id of the window the field stands in: the
/// application cannot know it, because it is the index chain the tree walk mints.
#[test]
fn the_screen_fills_the_field_with_its_own_window() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let tree = json!({
        "component": "display-pane",
        "key": "c.chat",
        "props": {"pane_id": "c.chat", "context": "conversation", "topic": "chat",
                  "layer": "modal", "pinned": true},
        "children": [
            {"component": "display-chat", "props": {"title": "Chat"}},
            {"component": "display-input", "key": "say",
             "props": {"placeholder": "Write something …", "event": "say", "for": ""}},
        ],
    });
    let mut screen = Screen::new(json!({}));
    screen.write(component_view("chat", "main", tree), 1000);
    let field = screen
        .props(&format!("{}/say", pane_id("chat", "chat")))
        .expect("the field is an object of its own");
    assert_eq!(
        field["for"],
        pane_id("chat", "chat").as_str(),
        "the screen named the window the typing belongs to: {field}"
    );
    assert_eq!(
        field["event"], "say",
        "and left the application's name alone: {field}"
    );
}

/// The line appears only where somebody can type, and the tap is bound only where
/// somebody can point (§ 6.4). Both are one value of the systemwide state, rendered per
/// output -- a television takes neither, and a dead binding on it would be a promise the
/// screen cannot keep.
#[test]
fn the_line_and_the_tap_follow_the_outputs_inputs() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(json!({"screens": {
        // A television is given inputs on purpose: the door takes them away again,
        // because a tv has none whatever its profile says (§ 4.7).
        "wall": {"display_type": "tv", "inputs": ["pointer", "keyboard"]},
        "desk": {"display_type": "monitor", "inputs": ["pointer"]},
        "typing": {"display_type": "monitor", "inputs": ["pointer", "keyboard"]},
        "pad": {"display_type": "phone", "inputs": ["touch", "audio"]},
    }, "default_screen": "desk"}));
    screen.write(
        view_of(
            "alex",
            "chat",
            "main",
            support::pane(
                "chat",
                json!({"context": "conversation", "relevance": "0.9"}),
            ),
        ),
        1000,
    );
    let root = |exit: &str| {
        screen
            .props(&format!("{exit}.display.root"))
            .unwrap_or_else(|| panic!("{exit} has a root"))
    };
    for (exit, tap, line) in [
        ("wall", false, false),
        ("desk", true, false),
        ("typing", true, true),
        ("pad", true, true),
    ] {
        let props = root(exit);
        assert_eq!(props["tap"], tap, "{exit} binds the tap: {props}");
        assert_eq!(props["input_line"], line, "{exit} shows the line: {props}");
    }
}
