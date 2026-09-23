//! D1 -- the OS mark is the anchor: a child of the root beside the dock,
//! written on every screen, and the old microphone object is written no more.
//!
//! The curator runs the way a `resident` code cell runs it (GH #809): ONE living
//! cell over many messages, with the store and the display played beside it by
//! `support::Screen`. What a pass draws is read off the patch it sends to the
//! display.

mod support;

use meclaw_core::serde_json::json;
use support::{Screen, library_ships, written};

/// A cell woken by the clock's first stroke on an empty store and an empty
/// display: its boot reads nothing back and draws the bare screen.
fn bare_screen() -> Screen {
    let mut screen = Screen::new(json!({}));
    screen.pass(json!({"kind": "stroke"}), 1000);
    screen
}

/// The anchor at the bottom right is the OS mark, and it is a child of the
/// ROOT beside the dock -- never inside it (OR-D4): it carries its own hook,
/// and a dock that re-renders must not take the microphone with it.
#[test]
fn the_os_mark_stands_on_the_root_below_the_dock() {
    if !library_ships() {
        return;
    }
    let mut screen = Screen::new(json!({}));
    let calls = screen.pass(json!({"kind": "stroke"}), 1000);
    let os = calls
        .iter()
        .find(|c| c["id"] == "display.os")
        .expect("the OS mark is created");
    assert_eq!(os["component"], "display-os", "the new component: {os}");
    assert_eq!(os["parent"], "display.root", "a child of the root: {os}");
    assert_eq!(os["props"]["mount"], "voice", "it knows its voice cell");
    assert!(
        os["props"]["client_js"]
            .as_str()
            .is_some_and(|js| js.contains("DisplayMic")),
        "and it carries the browser half"
    );
    let dock = calls
        .iter()
        .find(|c| c["id"] == "display.dock")
        .expect("the dock is created");
    assert!(
        dock["ord"].as_i64().unwrap_or(0) < os["ord"].as_i64().unwrap_or(0),
        "the mark sits below the dock: {dock} / {os}"
    );
    assert!(
        !calls.iter().any(|c| c["id"] == "display.mic"),
        "and nothing writes the old microphone any more: {calls:?}"
    );
    assert!(written(&calls, "display.os").is_some());
}

/// The mark is structural: it is written on a bare screen and it stands on
/// every later pass.
#[test]
fn the_mark_is_written_on_every_screen() {
    if !library_ships() {
        return;
    }
    let screen = bare_screen();
    assert!(
        screen.holds("display.os"),
        "the bare screen holds the mark: {}",
        screen.held
    );
    assert!(
        !screen.holds("display.mic"),
        "and not the old one: {}",
        screen.held
    );
}

/// A display that still holds the old microphone object is cleaned up by the
/// first patch after the lift (OR-D7): the restarted cell reads the tree once
/// at its boot and sweeps what it does not draw.
#[test]
fn the_old_microphone_is_swept_as_an_orphan() {
    if !library_ships() {
        return;
    }
    let mut screen = bare_screen();
    screen.web_put(json!({
        "id": "display.mic", "parent": "display.root", "ord": 20,
        "component": "display-os", "props": {"mount": "voice", "client_js": ""},
    }));
    screen.kill();
    let calls = screen.pass(json!({"kind": "stroke"}), 2000);
    assert!(
        calls
            .iter()
            .any(|c| c["op"] == "object.delete" && c["id"] == "display.mic"),
        "the old object is deleted: {calls:?}"
    );
    assert!(!screen.holds("display.mic"), "and gone: {}", screen.held);
}
