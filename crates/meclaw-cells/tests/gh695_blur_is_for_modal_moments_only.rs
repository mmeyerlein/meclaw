//! Welle D -- blur is for modal moments only (D-12, D-15), carried forward to the
//! contract of `display-hive.md` § 4.24, where the moment has a NUMBER.
//!
//! A change of focus is not a modal moment: weather in focus does not make the rest of
//! the screen unsharp. Until 2.3.3 what was left blurred on a boolean the application
//! set about itself; since then it blurs on the level the curator derived, so a window
//! cannot make the whole screen unsharp by declaring a word.
//!
//! This half of the lock is the SCRIPT's: what the curator declares an application may
//! send (`CURATED`), and which of those names the window templates render as the sheet's
//! attributes (`ATTRS`, § 6 -- one list, one place). The sheet's own half stands below
//! and is another strand's to move.
//!
//! Skips when `python3` is absent or the templates do not ship (R2b).

mod support;

use std::process::Command;

use meclaw_core::serde_json::Value;
use support::{COMPOSE, library_ships, repo};

const SHEET: &str = "templates/display/compose/display-dna.css";

/// What the script says about its own vocabulary: the props an application may send
/// (`CURATED`), the hints the pass reads out of a row (`HINT_KEYS`) and the attribute
/// names the sheet reads (`ATTRS`). Asked of the script itself, because a constant
/// copied into a test is a second source of truth.
fn vocabulary() -> Option<Value> {
    let out = Command::new("python3")
        .arg("-c")
        .arg(
            "import importlib.util, json, sys\n\
             spec = importlib.util.spec_from_file_location('compose', sys.argv[1])\n\
             m = importlib.util.module_from_spec(spec)\n\
             spec.loader.exec_module(m)\n\
             print(json.dumps({'curated': m.CURATED, 'hints': list(m.HINT_KEYS),\n\
                               'attrs': m.ATTRS, 'window_attrs': list(m.WINDOW_ATTRS)}))",
        )
        .arg(repo(COMPOSE))
        .output()
        .ok()?;
    assert!(
        out.status.success(),
        "compose.py did not load:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(meclaw_core::serde_json::from_slice(&out.stdout).expect("JSON"))
}

/// Plane-filter declarations that stand in a rule of their own, at any indentation.
///
/// The obvious needle is `"\n  filter: "`, and it counts correctly today and
/// wrongly tomorrow: a rule inside an `@media` block is indented by four
/// spaces, so a third grade added there would slip past a count that is spelled
/// with two. The material's own `backdrop-filter` and the blur inside the leave
/// keyframe (which stands inline behind its `{`) are not declarations of their
/// own and are not counted, which is the whole point of the measure.
///
/// Since wave H3 the VALUE is a token (`--f-back-2` / `--f-back-3`) so that a
/// setting which takes blur away can say `none` rather than a zero radius -- a
/// filter that is not `none` still costs a render layer and a containing block.
/// The count therefore asks for the declaration, not for the word `blur`.
fn rule_level_blurs(sheet: &str) -> usize {
    sheet
        .lines()
        .filter(|line| line.trim_start().starts_with("filter: var(--f-back-"))
        .count()
}

/// The grade a window is drawn at is the curator's `level` (§ 4.24), and an application
/// may not send it: `plane` is struck, and the word the app IS allowed to say about
/// itself (`state`) is only ever `urgent` or `hidden` (§ 4.6).
#[test]
fn the_grade_is_a_curator_value_and_not_an_app_word() {
    if !library_ships() {
        return;
    }
    let Some(v) = vocabulary() else {
        return;
    };
    assert_eq!(v["curated"]["level"], "text", "the curator writes `level`");
    assert_eq!(v["curated"]["layer"], "text", "and the ladder beside it");
    assert_eq!(v["curated"]["topic"], "text");
    assert!(
        v["curated"]["plane"].is_null(),
        "`plane` is struck -- `level` replaced it: {}",
        v["curated"]
    );
    // An app hint is a hint and nothing more: `level` is not among the names an
    // application may say about its own window.
    let hints = v["hints"].as_array().expect("a list");
    assert!(
        !hints.iter().any(|k| k == "level" || k == "plane"),
        "no application sends the grade it is drawn at: {hints:?}"
    );
    assert!(
        hints.iter().any(|k| k == "layer") && hints.iter().any(|k| k == "topic"),
        "the ladder and the topic ARE the app's to say: {hints:?}"
    );
}

/// `ATTRS` is the one place the attribute names live (§ 6), and the window templates
/// render exactly what it says -- `data-level`, not the struck `data-plane`.
#[test]
fn the_windows_render_the_names_attrs_declares() {
    if !library_ships() {
        return;
    }
    let Some(v) = vocabulary() else {
        return;
    };
    assert_eq!(v["attrs"]["level"], "data-level");
    assert_eq!(v["attrs"]["rung"], "data-rung");
    assert_eq!(v["attrs"]["layer"], "data-layer");
    assert!(
        v["attrs"]
            .as_object()
            .expect("a map")
            .values()
            .all(|a| a != "data-plane" && a != "data-state"),
        "neither struck name is in the list: {}",
        v["attrs"]
    );
    let src = std::fs::read_to_string(repo(COMPOSE)).expect("compose.py");
    for name in ["PANE_TEMPLATE", "PANEL_TEMPLATE", "OVERLAY_TEMPLATE"] {
        let t = src
            .split(&format!("{name} = ("))
            .nth(1)
            .unwrap_or_else(|| panic!("{name}"))
            .split("\n)\n")
            .next()
            .unwrap();
        for key in ["level", "rung", "layer"] {
            let attr = v["attrs"][key].as_str().expect("an attribute name");
            assert!(
                t.contains(&format!("{attr}=\"{{{{{key}}}}}\"")),
                "{name} renders {attr}: {t}"
            );
        }
        assert!(
            t.contains("data-topic=\"{{topic}}\""),
            "{name} still renders the topic beside them: {t}"
        );
        assert!(
            !t.contains("data-plane=") && !t.contains("data-state="),
            "{name} still renders a struck name: {t}"
        );
    }
    // And every name a window wears is one the curator declared, so nothing a template
    // reads can arrive undeclared at the `web` cell.
    for key in v["window_attrs"].as_array().expect("a list") {
        let key = key.as_str().expect("a name");
        assert!(
            !v["curated"][key].is_null(),
            "`{key}` stands in CURATED: {}",
            v["curated"]
        );
    }
}

#[test]
fn the_blur_is_the_plane_and_nothing_else() {
    if !library_ships() {
        return;
    }
    let sheet = std::fs::read_to_string(repo(SHEET)).expect("the sheet");
    assert!(
        !sheet.contains(".display-columns:has([data-state=\"focus\"]"),
        "the focus-change blur is struck (D-12)"
    );
    // And so is its twin on the scene. It was struck with the same ruling and
    // has to stay struck for the same reason: a screen that goes soft every
    // time the focus moves is a screen nobody can read while it works.
    assert!(
        !sheet.contains(".display-scene:has(> [data-state=\"focus\"]"),
        "and so is its twin on the scene (D-12)"
    );
    assert!(
        !sheet.contains(".display-columns:has([data-modal=\"true\"])"),
        "and so is the boolean one: the plane says it now (R-23-2)"
    );
    assert!(
        sheet.contains(".display-columns:has([data-level=\"2\"]) [data-level=\"1\"]")
            && sheet.contains(
                ".display-columns:has([data-level=\"3\"]) :is([data-level=\"1\"], [data-level=\"2\"])"
            ),
        "two grades, both out of the plane"
    );
    assert_eq!(
        rule_level_blurs(&sheet),
        2,
        "two rule-level plane filters, and both are plane rules"
    );
}

#[test]
fn a_pinned_tile_says_so() {
    if !library_ships() {
        return;
    }
    let sheet = std::fs::read_to_string(repo(SHEET)).expect("the sheet");
    assert!(
        sheet.contains(".display-tile[data-pinned=\"1\"]::after"),
        "pinned means the tile stays, and a person can see that it will (D-15)"
    );
    assert!(
        sheet.contains(".display-tile[data-open=\"1\"]"),
        "the same object, just large right now -- the tile of an OPEN window \
         (display-hive.md § 6.10; `on-canvas` was the word of the planes)"
    );
}
