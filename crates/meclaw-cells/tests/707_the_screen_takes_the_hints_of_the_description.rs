//! display-hive.md § 3: the closed list of what an application may say about its own window,
//! and § 2: the words that are struck. The `web` cell refuses an undeclared prop, so the
//! prop schema of the four window components IS the contract surface -- a hint that is not
//! declared cannot be sent, and a struck word that is still declared is still sendable.
//!
//! Replaces `gh702_the_screen_takes_four_new_hints.rs`, which pinned `modal` and `plane`.

use std::fs;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The body of the `CURATED` map of `compose.py`: the props every window declares.
fn curated() -> String {
    let src = fs::read_to_string(repo("templates/display/compose/compose.py")).unwrap();
    let start = src.find("\nCURATED = {").expect("the CURATED map") + 1;
    let stop = src[start..].find("\n}\n").expect("its end") + start;
    src[start..stop].to_string()
}

fn declares(body: &str, key: &str) -> bool {
    body.contains(&format!("\"{key}\":"))
}

#[test]
fn every_hint_of_the_description_is_declared() {
    // § 3, the table: what an application writes to its window. `turn_id` is new in 2.5.0
    // (§ 8.3: the turn a window came out of, which is how the chat closes, § 4.13).
    let body = curated();
    for hint in [
        "context",
        "relevance",
        "class",
        "pinned",
        "relevant_until",
        "touched",
        "topic",
        "layer",
        "seat",
        "seat_ord",
        "linger",
        "state",
        "turn_id",
    ] {
        assert!(
            declares(&body, hint),
            "the hint `{hint}` of § 3 is not declared"
        );
    }
}

#[test]
fn the_rendering_values_are_declared_and_the_struck_words_are_gone() {
    // The curator's values reach the sheet as props of the window object (§ 3.1: the same
    // on every output). What § 2 struck may not stand here at all -- a declared prop is a
    // prop an application may send, so a struck word that is still declared is still live.
    let body = curated();
    for value in ["rung", "level", "front", "age", "led", "since", "score"] {
        assert!(
            declares(&body, value),
            "the rendering value `{value}` is not declared"
        );
    }
    for struck in [
        "plane",
        "modal",
        "canvas_slots",
        "dock_overflow",
        "canvas_rank",
        "topic_dupe",
        "topic_relevance",
        "judged_relevance",
        "judged_hidden",
        "dismissed_at",
        "on_canvas",
    ] {
        assert!(
            !declares(&body, struck),
            "`{struck}` is a struck word of § 2 and is still declared"
        );
    }
}

#[test]
fn the_curator_keeps_no_memory_on_the_objects() {
    // Since 2.5.0 the state lies in the store (§ 3.1, OR-H2). There is no `CURATOR_KEYS`
    // beside the hints any more: a value the display holds is a RENDERING, and a rendering
    // that is also the memory makes every output carry a memory of its own.
    let src = fs::read_to_string(repo("templates/display/compose/compose.py")).unwrap();
    assert!(
        !src.contains("CURATOR_KEYS = ("),
        "the display objects are no longer the curator's memory"
    );
    assert!(
        src.contains("STATE_VIEW_ID = \"screen-state\""),
        "the state row is"
    );
}

#[test]
fn the_shell_wears_the_output_and_the_switch_and_no_geometry_of_the_old_model() {
    let src = fs::read_to_string(repo("templates/display/compose/compose.py")).unwrap();
    let start = src.find("\"name\": \"display-shell\"").expect("the shell");
    let stop = src[start..].find("\"editable\": []").expect("its end") + start;
    let shell = &src[start..stop];
    for prop in [
        "exit",
        "switch",
        "tap",
        "input_line",
        "dock",
        "dock_max",
        "scale",
        "inputs",
    ] {
        assert!(
            shell.contains(&format!("\"{prop}\":")),
            "the root declares `{prop}`"
        );
    }
    for struck in [
        "\"screen\":",
        "canvas_slots",
        "dock_overflow",
        "\"focus\":",
        "judged_at",
    ] {
        assert!(!shell.contains(struck), "the root still declares {struck}");
    }
}
