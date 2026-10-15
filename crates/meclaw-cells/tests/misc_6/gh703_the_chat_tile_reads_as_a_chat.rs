//! Welle F -- a chat tile reads as a chat, and a value carries its unit
//! (F-15, F-21, E-7/E-8).
//!
//! The measure the mockups set: an operating system of this kind restyles the
//! CONTENT and leaves the frames and windows alone. So the tile keeps its
//! radius, its glass and its shadow; what changes is what stands inside it --
//! the bubble leads instead of the text, two lines instead of one cut, a dot
//! for a line nobody has read, and a value whose unit is set beside it rather
//! than swallowed by an ellipsis (`15.2…` was the measurement).
//!
//! Skips when the templates do not ship (R2b).

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const SHEET: &str = "templates/display/compose/display-dna.css";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

fn sheet() -> String {
    std::fs::read_to_string(repo(SHEET)).expect("the sheet ships")
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

#[test]
fn the_chat_tile_leads_with_the_bubble() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    let glyph = rule(
        &sheet,
        ".display-tile[data-topic=\"chat\"] .display-tile-glyph {",
    );
    assert!(
        glyph.contains("var(--t-title-1)") && glyph.contains("var(--accent)"),
        "the bubble is the word \"chat\" without a word: {glyph}"
    );
    let line = rule(
        &sheet,
        ".display-tile[data-topic=\"chat\"] .display-tile-line {",
    );
    assert!(
        line.contains("line-clamp: 2") && line.contains("white-space: normal"),
        "half a sentence is readable, a cut one is not: {line}"
    );
    // And the base rule stays one line for every other tile (D-1).
    let base = rule(&sheet, ".display-tile-line {");
    assert!(
        base.contains("white-space: nowrap"),
        "one size, always, for everything that is not a dialogue: {base}"
    );
}

#[test]
fn an_unread_line_shows_and_does_not_collide_with_the_pin() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    let unread = rule(&sheet, ".display-tile[data-unread=\"1\"]::before {");
    assert!(
        unread.contains("inset-inline-start"),
        "the unread dot sits on the left: the pin owns the right (`::after`, § 11): {unread}"
    );
    let pinned = rule(&sheet, ".display-tile[data-pinned=\"1\"]::after {");
    assert!(
        pinned.contains("inset-inline-end"),
        "and the pin is still where it was: {pinned}"
    );
}

#[test]
fn a_value_carries_its_unit_beside_it() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    let unit = rule(&sheet, ".display-tile-unit {");
    assert!(
        unit.contains("var(--t-caption)") && unit.contains("var(--fg-tertiary)"),
        "small, quiet, and beside the number rather than inside it: {unit}"
    );
}

#[test]
fn the_weather_window_leads_with_the_number() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    let temp = rule(&sheet, ".display-weather-temp {");
    assert!(
        temp.contains("var(--t-value)") && temp.contains("tabular-nums"),
        "the value carries the window (E-8): {temp}"
    );
    // And the unit stands UNDER it since GH #741: the value is a column, the
    // unit its label, the way the dock tile has always written it.
    assert!(
        temp.contains("flex-direction: column"),
        "the value is a column: {temp}"
    );
    let unit = rule(&sheet, ".display-weather-unit {");
    assert!(
        !unit.contains("vertical-align"),
        "and the unit no longer hangs beside it: {unit}"
    );
    let place = rule(&sheet, ".display-weather-place {");
    assert!(
        place.contains("var(--t-caption)") && place.contains("text-transform: uppercase"),
        "the place is small (E-8): {place}"
    );
    assert!(
        sheet
            .contains(".display-weather-range:not(:has(.display-weather-hi, .display-weather-lo))"),
        "and a range nobody delivered draws no empty row"
    );
}
