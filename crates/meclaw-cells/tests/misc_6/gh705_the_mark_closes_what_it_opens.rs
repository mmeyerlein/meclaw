//! GH #705 -- the OS mark is a TOGGLE on every exit, and a drag moves nothing.
//!
//! Both halves were found on real devices, side by side, and neither is a
//! taste question:
//!
//! 1. `data-dock-open="1"` was only ever read beside a dock whose profile
//!    says `hidden`. The opening could reveal a closed dock; nothing closed
//!    an open one. On an exit that ships its dock open the tap arrived, the
//!    attribute flipped, and the screen did not move -- a control that reads
//!    as dead. The closing rule is the other half of the pair.
//!
//! 2. The client's first tap wrote `"1"` because the attribute was absent.
//!    With a closing rule alone, an exit that ships OPEN would need two taps
//!    before anything happened: one to write the `"1"` it already is, one to
//!    write `"0"`. So the opening state starts from the GROUND state the
//!    server rendered, and the first tap always means "the other one".
//!
//! 3. The page is furniture, not a document. A one-finger drag rubber-banded
//!    the whole screen, because `touch-action: none` sat on the mark alone.
//!    The page absorbs the gesture; the scrollers inside windows keep
//!    scrolling, which is why this is `overscroll-behavior` on the page and
//!    never `touch-action: none` on the canvas.
//!
//! Skips when the templates do not ship (R2b).

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const SHEET: &str = "templates/display/compose/display-dna.css";
const SCRIPT: &str = "templates/display/compose/compose.py";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

fn sheet() -> String {
    std::fs::read_to_string(repo(SHEET)).expect("the sheet ships")
}

fn script() -> String {
    std::fs::read_to_string(repo(SCRIPT)).expect("the script ships")
}

fn rule<'a>(sheet: &'a str, selector: &str) -> &'a str {
    sheet
        .split(selector)
        .nth(1)
        .unwrap_or_else(|| panic!("the sheet has no rule for {selector}"))
        .split('}')
        .next()
        .unwrap()
}

/// The half that was missing: a dock that ships OPEN can be closed.
#[test]
fn the_mark_closes_a_dock_that_ships_open() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    const CLOSING: &str =
        "html[data-dock-open=\"0\"] .display-columns[data-dock=\"shown\"] .display-dock {";
    let closing = rule(&sheet, CLOSING);
    assert!(
        closing.contains("display: none"),
        "closing is the same kind of act as hiding -- a box that was never made, \
         not a request to the compositor (R-23-4): {closing}"
    );
    // And the lane it held falls to the canvas, exactly as it does for an exit
    // whose profile ships the dock hidden.
    let columns = rule(
        &sheet,
        "html[data-dock-open=\"0\"] .display-columns[data-dock=\"shown\"] {",
    );
    assert!(
        columns.contains("padding-inline-end: calc(var(--pad-window) - var(--gutter))"),
        "a canvas with no dock beside it keeps no lane free for one -- and since \
         GH #739 the canvas carries `--gutter` inside its own scroller, so what \
         the column still owes is the rest of the window air: {columns}"
    );
}

/// The two rules are a PAIR: whatever the profile ships, one tap is enough.
#[test]
fn every_exit_answers_the_first_tap() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    for (ground, open) in [("hidden", "1"), ("shown", "0")] {
        let selector = format!(
            "html[data-dock-open=\"{open}\"] .display-columns[data-dock=\"{ground}\"] .display-dock {{"
        );
        assert!(
            sheet.contains(&selector),
            "an exit that ships `{ground}` has no rule for the state one tap puts it in"
        );
    }
}

/// The first tap reads the ground state instead of assuming it is closed.
#[test]
fn the_first_tap_starts_from_what_the_server_rendered() {
    if !library_ships() {
        return;
    }
    let script = script();
    let tap = script
        .split("function tapDock()")
        .nth(1)
        .expect("the script has the dock toggle")
        .split("st.taps++")
        .next()
        .expect("the toggle ends by counting the tap");
    // `.display-columns` is where the server wrote the ground state, and
    // reading it is the only way to know it. Matching on `data-dock` alone
    // would pass on the `data-dock-open` the toggle writes anyway. Since 2.5.0
    // the hook holds the columns in `cols` -- the profile's inputs and the
    // hold threshold are read off the same element (§ 6.4, § 5.5).
    assert!(
        tap.contains("cols && cols.getAttribute(\\\"data-dock\\\")"),
        "the toggle has to READ the ground state the server rendered on the columns, \
         or the first tap on an exit that ships open writes the value it already \
         has: {tap}"
    );
    assert!(
        script.contains("var cols = document.querySelector(\\\".display-columns\\\")"),
        "and `cols` is the columns themselves, not some other element"
    );
    assert!(
        tap.contains("data-dock-open"),
        "and it still writes the opening on <html>, where no patch reaches: {tap}"
    );
}

/// A drag on the page moves nothing -- and the scrollers keep scrolling.
#[test]
fn a_drag_on_the_page_moves_the_page_nowhere() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    for selector in ["html {", "body {"] {
        let r = rule(&sheet, selector);
        assert!(
            r.contains("overscroll-behavior: none"),
            "the page absorbs the gesture rather than rubber-banding under it: \
             {selector} {r}"
        );
    }
    let body = rule(&sheet, "body {");
    assert!(
        body.contains("overflow: hidden"),
        "the screen is furniture, not a document: it does not scroll as a whole: {body}"
    );
    // The gesture is taken from the PAGE, never from the canvas: a window that
    // scrolls its own content has to keep doing so, and the chat is one.
    assert!(
        !rule(&sheet, ".display-columns {").contains("touch-action: none"),
        "taking the gesture on the canvas would take it from the chat's scroller too"
    );
}

/// A tile stands against what is behind it -- a lit window included.
#[test]
fn a_tile_reads_over_a_window() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    let tile = rule(&sheet, ".display-tile {");
    // The dock is furniture in front of the canvas, not a pane among the
    // panes: over a lit chat window the thin tint left the tiles barely
    // visible, measured on a device.
    assert!(
        tile.contains("background-color: var(--glass-tint-thick)"),
        "a tile carries the thick tint, or it disappears over a lit window: {tile}"
    );
    // And the quiet rungs stay legible: `ambient` is quiet, not absent. Since
    // 2.5.0 they spend the one floor § 6.11 names, which is also the dimming
    // of an open window's tile (§ 6.10) -- the number itself stands once in
    // the sheet and is pinned by `708_no_part_of_a_tile_is_under_0_88`.
    let quiet = rule(
        &sheet,
        ".display-tile[data-rung=\"ambient\"],\n.display-tile[data-rung=\"hidden\"] {",
    );
    assert!(
        quiet.contains("opacity: var(--tile-open-opacity)"),
        "a quiet tile is quiet, not unreadable: it spends the floor of § 6.11 \
         like every other part of a tile -- found {quiet}"
    );
}
