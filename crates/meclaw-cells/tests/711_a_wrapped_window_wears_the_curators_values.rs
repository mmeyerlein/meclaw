//! display-hive.md § 7.1: an app is a view and sends exactly ONE window -- and the screen
//! draws that window with the curator's values on it, wherever in the tree it stands.
//!
//! An app may wrap its window: `unwrap_window` in `compose.py` says so in as many words
//! ("An app may wrap its window in a `display-stack`; the pass reads the window"), and
//! `chat@0.3.0` does it -- its view is a `display-stack` with the pane inside. The PASS
//! read such a window correctly all along; the RENDER did not. `add_tree` handed the
//! curator's values to a window at the root of a tree and handed the children `None`, so
//! a wrapped window reached the DOM wearing nothing but the app's own props.
//!
//! Measured on the instance e25 (wave H, 17.09.), one render, two apps:
//!
//! ```text
//! <section id="chat"      data-rung=""       data-level=""  data-pinned="true">
//! <section id="t-clock"   data-rung="hidden" data-level="0" data-pinned="1">
//! ```
//!
//! `ambient` puts its pane at the root and gets the pass's words; the chat wraps its pane
//! and gets the raw `pinned: true` it sent itself. No level rule of § 7d reaches such a
//! window -- not even `[data-level="0"] { display: none }` -- so a conversation the
//! curator had put away (rung `ambient`, level 0, tile only) stood open and 4948 px tall
//! on a 852 px phone, and three browser proofs read the screen through it: B-09 (a window
//! outgrew the output), B-20 (a stage with no window on any level) and B-06 (the pass
//! wiping the level the client had just drawn).
//!
//! What is asserted here is that the two shapes are ONE shape: the same hints written
//! flat and wrapped put the same values on the window.

mod support;

use meclaw_core::serde_json::{Value, json};
use support::{Screen, component_view, library_ships, pane, window_id};

fn params() -> Value {
    json!({"screens": {"monitor": {"display_type": "monitor",
                                   "inputs": ["pointer", "keyboard"]}},
           "default_screen": "monitor"})
}

/// The hints of a window, the same for both shapes.
fn hints(name: &str) -> Value {
    json!({"title": name, "context": "conversation", "relevance": "0.9",
           "pinned": true, "layer": "modal", "topic": format!("t:{name}")})
}

/// The app that puts its window at the root of its view (`ambient`'s shape, § 9.1).
fn flat(view_id: &str) -> Value {
    component_view(view_id, "main", pane(view_id, hints(view_id)))
}

/// The app that wraps its window (`chat@0.3.0`'s shape, `compose.py:764`).
fn wrapped(view_id: &str) -> Value {
    component_view(
        view_id,
        "main",
        json!({"component": "display-stack", "key": "c.wrap", "props": {"gap": "m"},
               "children": [pane(view_id, hints(view_id))]}),
    )
}

/// The words the curator puts on a window, in the order they are read here.
const WORDS: [&str; 5] = ["rung", "level", "pinned", "age", "layer"];

fn words(props: &Value) -> Vec<String> {
    WORDS
        .iter()
        .map(|w| props[*w].as_str().unwrap_or("<none>").to_string())
        .collect()
}

#[test]
fn a_wrapped_window_wears_the_same_values_as_a_flat_one() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // TWO screens, not two windows on one: the same view, written flat on one and wrapped
    // on the other. Side by side they would be two windows competing for one ladder --
    // only one modal is `focus` (§ 4.21) -- and the difference measured would be the
    // pass's, which is not the question here.
    let draw = |row: Value, id: &str| -> Value {
        let mut screen = Screen::new(params());
        screen.write(row, 100_000);
        // A fresh window takes no focus in the pass it appears in (§ 4.35).
        screen.pass(json!({"kind": "stroke"}), 101_000);
        screen
            .props(id)
            .unwrap_or_else(|| panic!("the screen draws {id}"))
    };
    let oid = window_id("alex", "one");
    let bare_props = draw(flat("one"), &format!("monitor.{oid}/c.one"));
    let inside_props = draw(wrapped("one"), &format!("monitor.{oid}/c.wrap/c.one"));

    assert_eq!(
        words(&bare_props),
        words(&inside_props),
        "one window of one view (§ 7.1): a wrapper is not a reason to draw it differently"
    );
    // And the values are the CURATOR's, not the app's. `pinned: true` is what the app
    // sent; `"1"` is what `window_attrs` writes, and the difference between the two is
    // exactly what the instance showed.
    assert_eq!(
        inside_props["pinned"], "1",
        "the wrapped window wears the curator's `pinned`, not the app's raw boolean"
    );
    assert!(
        inside_props["level"]
            .as_str()
            .is_some_and(|l| !l.is_empty()),
        "a window without a level is drawn at a depth nobody decided: {inside_props}"
    );
    assert_eq!(
        inside_props["rung"], bare_props["rung"],
        "the rung is the pass's word about the window, not about its wrapper"
    );
}

#[test]
fn the_wrapper_itself_stays_the_app_s_own_node() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(params());
    screen.write(wrapped("inside"), 100_000);
    screen.pass(json!({"kind": "stroke"}), 101_000);

    // The values travel PAST the wrapper, they do not land on it: a `display-stack` is
    // not a window (§ 2 Window), and a rung on it would be a second window's worth of
    // state on a node the pass never computed one for.
    let wrap = format!("monitor.{}/c.wrap", window_id("alex", "inside"));
    let props = screen
        .props(&wrap)
        .unwrap_or_else(|| panic!("the screen draws the wrapper {wrap}"));
    for word in ["rung", "level", "age"] {
        assert_eq!(
            props[word],
            Value::Null,
            "the wrapper carries no `{word}`: it is the app's own node"
        );
    }
    assert_eq!(props["gap"], "m", "and it keeps what the app said about it");
}
