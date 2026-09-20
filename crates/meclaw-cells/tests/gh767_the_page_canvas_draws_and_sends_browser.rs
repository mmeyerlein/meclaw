//! The page half of the scene hook, driven in a real browser (GH #767).
//!
//! The four files beside this one read the hook's source. This one runs it:
//! `workshop/tools/display-page-browser.mjs --mode local` builds a page out of
//! `compose.py`'s own parts -- the sheet, the hook, the markup a screen would
//! write -- hands it a FAKE socket, and measures what no grep can. That the
//! sweep runs at all. That `data-level` and not the element decides. That a
//! real binary frame lands on the canvas in PAGE pixels. That a click comes
//! back as a coordinate the cell could use. That the tap of the tile still
//! reaches the curator through a live page. And that a window put away gives
//! its topic, its field and its listeners back.
//!
//! Five tests, each reading its own part of the line -- the shape
//! `gh695_the_scene_ticks_and_chimes_browser.rs` has. And five RUNS: nextest
//! gives every test its own process, so `run()` below drives a browser once
//! per test, not once per file. Said plainly because it is a cost and a
//! ceiling -- BUILD-PREAMBLE § 4 of this wave allows two browsers at a time,
//! and five tests in one binary at `NEXTEST_TEST_THREADS=4` are four. Nothing
//! is installed for them beyond what the laboratory already needs: without
//! playwright or without a browser bundle the driver says `SKIP` and these
//! pass, the tool guard every template-reading test in this tree uses (R2b).

use std::process::Command;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";
const DRIVER: &str = "workshop/tools/display-page-browser.mjs";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

/// The sheet and the hook as the browser gets them, asked of the script
/// itself: it is the only way to be sure the bytes under test are the bytes
/// that ship.
fn parts(dir: &std::path::Path) -> Option<()> {
    let out = Command::new("python3")
        .arg("-c")
        .arg(
            "import importlib.util, os, sys\n\
             spec = importlib.util.spec_from_file_location('compose', sys.argv[1])\n\
             m = importlib.util.module_from_spec(spec)\n\
             spec.loader.exec_module(m)\n\
             open(os.path.join(sys.argv[2], 'scene.js'), 'w').write(m.SCENE_CLIENT_JS)\n\
             open(os.path.join(sys.argv[2], 'sheet.css'), 'w').write(m.KIT_CSS)",
        )
        .arg(repo(COMPOSE))
        .arg(dir)
        .output()
        .ok()?;
    assert!(
        out.status.success(),
        "compose.py did not answer:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(())
}

/// One `local` run, or `None` where this host cannot drive a browser.
fn run() -> Option<String> {
    if !library_ships() || !repo(DRIVER).is_file() {
        return None;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    parts(td.path())?;
    let out = Command::new("node")
        .arg(repo(DRIVER))
        .arg(td.path())
        .arg(td.path().join("out"))
        .arg("--mode")
        .arg("local")
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if out.status.code() == Some(3) || stderr.contains("SKIP") {
        println!("{stderr}");
        return None;
    }
    let line = stdout
        .lines()
        .find(|l| l.starts_with("PAGE mode=local "))
        .unwrap_or_else(|| panic!("the driver printed no line:\n{stdout}\n{stderr}"))
        .to_string();
    println!("{line}");
    assert!(
        out.status.success(),
        "the driver measured something it did not want:\n{line}\n{stderr}"
    );
    Some(line)
}

/// One field of the driver's line.
fn field(line: &str, key: &str) -> String {
    line.split_whitespace()
        .find_map(|part| part.strip_prefix(key))
        .unwrap_or_else(|| panic!("{key} is not in {line:?}"))
        .to_string()
}

/// T4 -- joining and leaving follow `data-level`, and the viewport is read.
#[test]
fn a_page_joins_on_a_level_and_never_off_one() {
    let Some(line) = run() else {
        return;
    };
    assert_eq!(
        field(&line, "joined="),
        "1",
        "the standing page joined: {line}"
    );
    // The whole reason the sweep reads the level: two windows are in the DOM
    // and neither is standing, so neither has a channel. A `1` or `2` here is
    // a screencast held open for a window nobody can see -- the defect marker
    // G6 measures against a colony.
    assert_eq!(
        field(&line, "level0="),
        "0",
        "a window on level 0 opens no stream: {line}"
    );
    // The PROFILE, not a measurement. `640x400` here would be the CSS box of
    // the canvas, and the streamed page would reflow under the reader's hand
    // with every patch.
    assert_eq!(field(&line, "vp="), "1920x1080@1", "{line}");
    assert_eq!(field(&line, "mobile="), "false", "{line}");
    // And the frame has proportions before the first picture: empty here means
    // `data-viewport` was never read at join time.
    assert_eq!(field(&line, "ratio0="), "1280/800", "{line}");
    // A refusal is said on the frame, not swallowed.
    assert_eq!(field(&line, "refused="), "1", "{line}");
    // The curator still calls this page `loading` -- the application does not
    // know the join was turned down -- and the hook does not write over it.
    assert_eq!(field(&line, "state="), "loading", "{line}");
    assert_eq!(
        field(&line, "link="),
        "down",
        "what the CLIENT found out rides its own attribute: {line}"
    );
    // And said to a PERSON. `reason=` is the attribute the sheet reads,
    // `shown=` is the text standing in the caption: an attribute nothing
    // renders leaves an empty rectangle with an address in it, and the one
    // repair it needs -- mount a browser cell -- is nowhere on the screen
    // (contracts § 4). Underscores because the line is read by fields.
    assert_eq!(field(&line, "reason="), "no_browser_mounted", "{line}");
    assert_eq!(
        field(&line, "shown="),
        "no_browser_mounted",
        "the cell's word stands in the frame, not only in an attribute: {line}"
    );
}

/// T4b -- the seam: a page the curator calls asleep, on a channel that stands.
///
/// The one case where the two voices that once shared `data-page-state` say
/// different things. Measured here and not by reading the hook's source,
/// because the writers only meet in the DOM: the curator renders the word out
/// of the application's seven cell states (contracts § 4), the hook ran over
/// it a moment later, and nothing in either file says so. Before the split
/// this field read `ready` and the sheet's grayscale rule never drew -- a
/// sleeping page looked like a waking one (g17, offene Punkte; OR-G.g18.1).
#[test]
fn a_sleeping_page_does_not_wake_up_because_its_channel_stands() {
    let Some(line) = run() else {
        return;
    };
    assert_eq!(
        field(&line, "asleep="),
        "suspended",
        "the curator's word survives a join that worked: {line}"
    );
    // What the hook knows, and all it knows. `down` here would be a channel
    // that never stood and the measurement above would mean nothing.
    assert_eq!(
        field(&line, "asleepLink="),
        "up",
        "and the hook says what it knows on its own attribute: {line}"
    );
    // A channel that stands has no reason to give. A word here would put a
    // line in the caption of every working page.
    assert_eq!(field(&line, "asleepReason="), "", "{line}");
}

/// T5 -- a real JPEG on the canvas, sized by the head and not by the picture.
#[test]
fn a_binary_frame_lands_on_the_canvas_in_page_pixels() {
    let Some(line) = run() else {
        return;
    };
    assert_eq!(field(&line, "frames="), "1", "one frame drawn: {line}");
    // `2x2` would be the size of the picture the driver built, and every click
    // would then be out by the capping factor.
    assert_eq!(
        field(&line, "canvas="),
        "1280x800",
        "the canvas is the head's viewport: {line}"
    );
    assert_eq!(field(&line, "ratio="), "1280/800", "{line}");
    // And a second frame of another shape moves both. `1280/800` here would be
    // a frame whose proportions froze on the first picture.
    assert_eq!(field(&line, "canvas2="), "640x480", "{line}");
    assert_eq!(field(&line, "ratio2="), "640/480", "{line}");
}

/// T6 -- the hand reaches the cell, the curator hears nothing, the tile lives.
#[test]
fn the_hand_reaches_the_cell_and_the_tile_still_answers() {
    let Some(line) = run() else {
        return;
    };
    assert_eq!(field(&line, "clicked="), "1", "{line}");
    assert_eq!(field(&line, "wheeled="), "1", "{line}");
    assert_eq!(field(&line, "typed="), "1", "{line}");
    assert_eq!(field(&line, "text="), "Gr\u{fc}\u{df}e", "{line}");
    // The driver clicks the middle of a 1280-pixel page shown in a 640-pixel
    // box. `320,200` would be the scaling the wrong way round, and every tap
    // in the page would land at half its distance from the corner.
    assert_eq!(field(&line, "at="), "640,400", "{line}");
    // § 5.6: there is no `touch` event any more, and an undeclared route costs
    // not one message but the whole emission. A `1` here would mean the page
    // sends compose something, compose would discard it, and the page would
    // expire under the reader's hand without a sound.
    assert_eq!(
        field(&line, "events="),
        "0",
        "the page sends the curator nothing: {line}"
    );
    // And the dock still works over a live page. `0` here is a screen where
    // the only way to a window on a telephone is gone.
    assert_eq!(field(&line, "tap="), "1", "{line}");
}

/// T7 -- put away gives everything back, and a closed link rejoins once.
#[test]
fn a_page_put_away_is_given_back_whole() {
    let Some(line) = run() else {
        return;
    };
    assert_eq!(field(&line, "left="), "1", "the topic went back: {line}");
    assert_eq!(field(&line, "links="), "0", "{line}");
    // One field per level change on a wall screen that reconnects all day is
    // the difference between one element and a thousand.
    assert_eq!(field(&line, "fields="), "0", "{line}");
    // And exactly one: the cell let go, the window still stands.
    assert_eq!(field(&line, "rejoined="), "1", "{line}");
}
