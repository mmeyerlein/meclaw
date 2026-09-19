//! Welle F, F4-N7 -- the conversation shows its NEWEST line (OR-F78).
//!
//! F4-N6 capped the leading window and made its body scroll, which put the
//! input line back within reach of a finger. It left the scroller standing at
//! `scrollTop = 0`: the window opened on the OLDEST line of the conversation,
//! and an answer that arrived did not move it. For a chat that is broken --
//! a person types at the foot of the window and the answer lands outside the
//! box they are looking at.
//!
//! What the hook has to do, and what each case here stands for:
//!
//!   * a conversation is chronological, so its newest line is at the FOOT of
//!     whatever scrolls above the lines -- and a window that opens goes there
//!     (`mounted`);
//!   * so does one that a patch has just grown a line in (`updated`);
//!   * but only where nobody has scrolled away from it. A window that pulls
//!     the floor out from under somebody reading back is as broken as one that
//!     never follows the answer, so the decision is taken in `beforeUpdate`,
//!     while the scroller still stands where the person left it, and what was
//!     held is carried into `updated` untouched. Away from the ANCHOR the hook
//!     stamped, never distance-to-the-foot: content grows by itself between two
//!     patches -- a late image, a font swap, a phone's address bar sliding out
//!     of the way -- and the foot reading calls that growth a reader and never
//!     follows an answer again;
//!   * the scroller is FOUND and not named. The sheet's own scroller selector
//!     (F4-N6) is a structural one (`> :not(.display-input)`), and a hook that
//!     spelled it a second time would go quietly blind the day the overflow
//!     moves one box in or out. Walking up from `.display-chat-lines` to the
//!     first ancestor that actually scrolls asks the layout instead;
//!   * and nothing is put on a clock. Polling is refused in this tree (EDA+ES)
//!     and there is nothing to poll for: LiveView patches the screen, and a
//!     patch is what `beforeUpdate`/`updated` are.
//!
//! One case runs the hook in a real browser, because the rest of this file is
//! source matching and source matching has a ceiling: a plan-external review
//! built two one-line mutants -- `tails()` returning an empty list, and
//! `held()` returning everything -- that leave every identifier below in place
//! and make the screen behave exactly as it did before this fix. Both are red
//! against the browser case and green against everything else here.
//!
//! Every other case reads the FUNCTION it is about, never the file as one string:
//! `compose.py` carries the sheet as well as the script, and a needle against
//! the whole file holds even when the function it names stopped saying it
//! (the lesson of the first draft of the § 5.9 lock, today
//! `708_no_window_outgrows_the_output`).
//!
//! Skips when the templates do not ship (R2b).

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

/// The scene hook AS THE BROWSER GETS IT.
///
/// `compose.py` carries the script as a run of quoted lines, and the sheet and
/// a second hook (the OS mark) stand in the same file. Reading the file as one
/// string would let a needle be answered by either of those, and would leave
/// the Python quoting between every two lines, where a body reader cannot see
/// where a function ends. So the run is cut out at its own name, each line is
/// unquoted, and the result is the JavaScript itself.
fn script() -> String {
    let file = std::fs::read_to_string(repo(COMPOSE)).expect("compose.py");
    let block = file
        .split("SCENE_CLIENT_JS = (\n")
        .nth(1)
        .expect("compose.py carries SCENE_CLIENT_JS")
        .split("\n)\n")
        .next()
        .expect("the run of lines is closed");
    let mut out = String::with_capacity(block.len());
    for line in block.lines() {
        let line = line.trim();
        let Some(inner) = line.strip_prefix('"').and_then(|l| l.strip_suffix('"')) else {
            panic!("a line of SCENE_CLIENT_JS is not a quoted string: {line}");
        };
        out.push_str(&inner.replace("\\n", "\n").replace("\\\"", "\""));
    }
    out
}

/// The body of the scene hook's function `name`, up to the line that closes it.
///
/// Every function of this script stands one indentation deep, so it opens with
/// `  function <name>(` and closes with a `  }` in the first column of that
/// depth. Cutting at that closing brace is what keeps a case from reading the
/// NEXT function's lines as the one it asks about.
fn body<'a>(src: &'a str, name: &str) -> &'a str {
    let head = format!("  function {name}(");
    let from = src
        .split(&head)
        .nth(1)
        .unwrap_or_else(|| panic!("the scene hook has no function `{name}`"));
    let end = from
        .find("\n  }\n")
        .unwrap_or_else(|| panic!("the function `{name}` is never closed"));
    &from[..end]
}

/// The body of one of the hook's lifecycle members (`mounted`, `updated`, ...).
fn phase<'a>(src: &'a str, name: &str) -> &'a str {
    let head = format!("    {name}: function () {{");
    let from = src
        .split(&head)
        .nth(1)
        .unwrap_or_else(|| panic!("the scene hook has no `{name}`"));
    let end = from
        .find("\n    },")
        .unwrap_or_else(|| panic!("`{name}` is never closed"));
    &from[..end]
}

#[test]
fn the_scroller_is_found_by_walking_up_from_the_lines() {
    if !library_ships() {
        return;
    }
    let src = script();
    let tails = body(&src, "tails");
    assert!(
        tails.contains(".display-chat-lines"),
        "the search starts at the list of lines -- the one element that says \
         THIS is a conversation and its order is time: {tails}"
    );
    assert!(
        tails.contains("parentElement"),
        "and walks UP from there: {tails}"
    );
    assert!(
        tails.contains("scrollHeight") && tails.contains("clientHeight"),
        "to the first ancestor that actually scrolls. Asking the layout is \
         what keeps this working when the sheet moves the overflow one box in \
         or out; spelling F4-N6's structural selector a second time would go \
         quietly blind that day. Body: {tails}"
    );
    // The sheet's scroller selector, spelled out in the script, is the mistake
    // this case exists to forbid.
    assert!(
        !tails.contains(":not(.display-input)") && !tails.contains("display-pane-body"),
        "and it does NOT re-spell the sheet's own scroller rule: {tails}"
    );
}

#[test]
fn a_window_that_opens_stands_at_its_newest_line() {
    if !library_ships() {
        return;
    }
    let src = script();
    let end = body(&src, "toEnd");
    assert!(
        end.contains("scrollTop = ") && end.contains("scrollHeight"),
        "the foot of a scroller is its full height: {end}"
    );
    let mounted = phase(&src, "mounted");
    assert!(
        mounted.contains("toEnd(el, [], st)"),
        "a window that opens goes there, and nothing is held back on the first \
         look -- the conversation was there before this screen was: {mounted}"
    );
}

#[test]
fn a_new_line_is_followed_only_where_the_foot_was_in_sight() {
    if !library_ships() {
        return;
    }
    let src = script();
    // The decision is taken BEFORE the patch: afterwards the new line is
    // already in the box, and every scroller that was at its foot reads as one
    // line short of it -- the measurement would answer a question about the
    // patch instead of about the person.
    let before = phase(&src, "beforeUpdate");
    assert!(
        before.contains("held(this.el)"),
        "what a person is holding is read while the scroller still stands \
         where they left it: {before}"
    );
    let updated = phase(&src, "updated");
    assert!(
        updated.contains("toEnd(") && updated.contains("held"),
        "and a patch follows the conversation everywhere else: {updated}"
    );
    let held = body(&src, "held");
    // The anchor, and this is the half a review had to find: held means the
    // person scrolled AWAY from where this hook last put them -- not that the
    // foot is far away. Content grows on its own between two patches (a late
    // image, a font swap, a phone's address bar sliding out of the way), and
    // the distance-to-the-foot reading calls that growth a reader: the screen
    // then never follows an answer again, for as long as the page stands.
    assert!(
        held.contains("__end") && held.contains("scrollTop"),
        "held is the distance from the anchor this hook stamped, not from the \
         foot: {held}"
    );
    assert!(
        !held.contains("scrollHeight"),
        "and it does NOT measure the foot -- content that grew by itself would \
         read as somebody scrolling back, and the conversation would stop \
         following for good: {held}"
    );
    assert!(
        body(&src, "toEnd").contains("__end = "),
        "which means the anchor is stamped wherever the hook scrolls"
    );
    assert!(
        held.contains("END_SLACK"),
        "with a named slack rather than an exact equality -- `scrollTop` is \
         fractional and a bounce overshoots, so `!=` would read every \
         conversation as held for ever: {held}"
    );
    let slack = src
        .split("var END_SLACK = ")
        .nth(1)
        .expect("END_SLACK is a number of its own")
        .split(';')
        .next()
        .unwrap()
        .trim()
        .parse::<i32>()
        .expect("END_SLACK is a number");
    assert!(
        (2..=40).contains(&slack),
        "and the slack is under one line of chat and over the browser's own \
         rounding: {slack}px is neither"
    );
    // The held list carries NODES. An attribute this script writes onto an
    // element can be taken off it by the next diff; morphdom patches the
    // element in place, so the node is the same object one pass later.
    assert!(
        !held.contains("setAttribute") && !held.contains("dataset"),
        "and it is remembered as nodes, not as a mark a diff can wipe: {held}"
    );
}

/// A LOCK and not a statement, and it says so rather than being counted.
///
/// Against the stand before this fix it is green and could not have been
/// anything else: there was no scroll code to put on a clock. What it forbids
/// is a WRONG build -- the shape a later hand reaches for when the hook misses
/// a patch, and the one this tree refuses (EDA+ES). The four cases above are
/// the ones that were measured red.
#[test]
fn nothing_about_the_scroll_is_put_on_a_clock() {
    if !library_ships() {
        return;
    }
    let src = script();
    // One interval exists in this hook and it is the second hand of the timer
    // plus the ring (D-27, real time semantics). Nothing about the scroll may
    // ride on it: a patch is the event, and asking the layout every second
    // where a person is scrolling is exactly the polling this tree refuses.
    let clock = src
        .split("var iv = root.setInterval(function () {")
        .nth(1)
        .expect("the hook's one interval")
        .split("}, 1000);")
        .next()
        .unwrap();
    assert!(
        !clock.contains("toEnd") && !clock.contains("tails") && !clock.contains("held"),
        "the one interval carries the second hand and the ring, and nothing \
         about the scroll: {clock}"
    );
    assert_eq!(
        src.matches("root.setInterval").count(),
        1,
        "and no second clock was started for it"
    );
}

#[test]
fn the_screen_counts_what_it_scrolled() {
    if !library_ships() {
        return;
    }
    let src = script();
    // The same bag the flips, the ticks and the chimes are counted in. A
    // browser driver reads `window.__displayScene` and has no other way to ask
    // whether the hook acted at all -- a scroller that is already at its foot
    // and one the hook never touched look identical in the geometry.
    let mounted = phase(&src, "mounted");
    assert!(
        mounted.contains("ends: 0"),
        "the count starts with the rest of the scene's state: {mounted}"
    );
    assert!(
        body(&src, "toEnd").contains("st.ends++"),
        "and every scroller this hook moves is counted"
    );
}

// ---------------------------------------------------------------------------
// And the same hook, RUN.
// ---------------------------------------------------------------------------

const DRIVER: &str = "workshop/tools/display-scene-browser.mjs";

/// The hook's source, as the browser gets it. Asking `compose.py` for it is the
/// only way to be sure the bytes under test are the bytes that ship.
fn scene_js(to: &std::path::Path) -> Option<()> {
    let out = std::process::Command::new("python3")
        .arg("-c")
        .arg(
            "import importlib.util, sys\n\
             spec = importlib.util.spec_from_file_location('compose', sys.argv[1])\n\
             m = importlib.util.module_from_spec(spec)\n\
             spec.loader.exec_module(m)\n\
             open(sys.argv[2], 'w').write(m.SCENE_CLIENT_JS)",
        )
        .arg(repo(COMPOSE))
        .arg(to)
        .output()
        .ok()?;
    assert!(
        out.status.success(),
        "compose.py did not answer:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(())
}

/// One point of the driver's line, as `<top>/<max>` -- `foot(a/b)` counts as
/// `a/b`, so a case can ask for the foot by name or read the numbers.
fn point(line: &str, key: &str) -> (i64, i64) {
    let raw = line
        .split_whitespace()
        .find_map(|p| p.strip_prefix(key))
        .unwrap_or_else(|| panic!("{key} is not in {line:?}"));
    let raw = raw.trim_start_matches("foot(").trim_end_matches(')');
    let (top, max) = raw
        .split_once('/')
        .unwrap_or_else(|| panic!("{key} is not <top>/<max> in {line:?}"));
    (
        top.parse().expect("a number"),
        max.parse().expect("a number"),
    )
}

/// The hook, in a real engine, against a conversation taller than its window.
///
/// This is the case the four above cannot be: every one of them is a needle in
/// the source, and a plan-external review built two one-line mutants that keep
/// every needle and undo the whole fix -- `tails()` that never collects a
/// scroller, and `held()` that holds every one of them. Measured against this
/// case, with the driver printing the numbers:
///
/// ```text
/// built      open=foot(568/568) follow=foot(624/624) held=0 regrow=foot(904/904) ends=4
/// tails=[]   open=0/568         follow=0/624         held=0 regrow=680/904       ends=0
/// held=all   open=foot(568/568) follow=568/624       held=0 regrow=680/904       ends=1
/// foot-held  open=foot(568/568) follow=foot(624/624) held=0 regrow=736/904       ends=3
/// ```
///
/// The third mutant is the one that matters most and the one a source needle
/// cannot see at all: `held()` measuring the distance to the FOOT rather than
/// the distance from the anchor. Content grows between two patches without
/// anybody's hand -- a late image, a font swap, a phone's address bar sliding
/// out of the way -- and that reading calls the growth a reader. `regrow` is
/// the point it falls on, and only there.
///
/// Skips when the templates, the driver, node or a browser are missing (R2b) --
/// this tree installs nothing for a test.
#[test]
fn a_browser_puts_the_conversation_at_its_newest_line() {
    if !library_ships() || !repo(DRIVER).is_file() {
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    let js = td.path().join("scene.js");
    if scene_js(&js).is_none() {
        return;
    }
    let out = match std::process::Command::new("node")
        .arg(repo(DRIVER))
        .arg(&js)
        .output()
    {
        Ok(out) => out,
        Err(_) => return,
    };
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if out.status.code() == Some(3) || stderr.contains("SKIP") {
        println!("{stderr}");
        return;
    }
    assert!(
        out.status.success(),
        "the driver failed:\n{stdout}\n{stderr}"
    );
    let line = stdout
        .lines()
        .find(|l| l.starts_with("SCENE "))
        .unwrap_or_else(|| panic!("the driver printed no numbers:\n{stdout}\n{stderr}"));
    println!("{line}");
    // Without this the page proved nothing: a conversation that never outgrew
    // its window answers every question below by standing still.
    assert!(
        line.contains("grew=true"),
        "the conversation grew taller than its window on every patch: {line}"
    );
    let (open, open_max) = point(line, "open=");
    assert!(
        open_max > 0 && open >= open_max - 2,
        "a window that opens stands at its newest line: {line}"
    );
    let (follow, follow_max) = point(line, "follow=");
    assert!(
        follow >= follow_max - 2,
        "and a line that arrives is followed: {line}"
    );
    let held: i64 = line
        .split_whitespace()
        .find_map(|p| p.strip_prefix("held="))
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("held= is not in {line:?}"));
    assert_eq!(
        held, 0,
        "and a scroller somebody put back by hand STAYS there -- the one point \
         a hook that scrolls unconditionally gets wrong: {line}"
    );
    let (regrow, regrow_max) = point(line, "regrow=");
    assert!(
        regrow >= regrow_max - 2,
        "and content that grew with no patch behind it is not mistaken for a \
         person reading back: after the next patch the newest line stands \
         again: {line}"
    );
}
