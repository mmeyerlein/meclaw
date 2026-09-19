//! The screen has four levels, and they are four numbers (§ 4.24, R-23-2).
//!
//! Until 2.3.3 the sheet carried nine single z-indexes, and that the OS mark
//! stood above an urgent window was document order rather than a statement:
//! both were 30. This wave says it instead. The level stands on the window as
//! `data-level`, the curator derives it from the application's `layer` hint
//! and the rung -- ONCE for the whole screen, § 6.2 -- and the blur follows
//! the level rather than a boolean an application may set about itself. The
//! z-order tokens keep the name `--plane-*`: they are the sheet's own
//! vocabulary, and what § 2 struck is a value per output.
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

/// The declarations of one rule, by its selector prefix.
fn rule<'a>(sheet: &'a str, selector: &str) -> &'a str {
    sheet
        .split(selector)
        .nth(1)
        .unwrap_or_else(|| panic!("the sheet has no rule for {selector}"))
        .split('}')
        .next()
        .unwrap()
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
/// Since the trim of wave H3 the VALUE is a token (`--f-back-2` / `--f-back-3`)
/// so that a setting which takes blur away can say `none` rather than a zero
/// radius -- a filter that is not `none` still costs a render layer. The count
/// therefore asks for the declaration, not for the word `blur`.
fn rule_level_blurs(sheet: &str) -> usize {
    sheet
        .lines()
        .filter(|line| line.trim_start().starts_with("filter: var(--f-back-"))
        .count()
}

#[test]
fn the_sheet_gives_every_plane_one_number() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    for (token, value) in [
        ("--plane-canvas", "5"),
        ("--plane-modal", "20"),
        ("--plane-urgent", "30"),
        ("--plane-os", "40"),
    ] {
        assert!(
            sheet.contains(&format!("{token}: {value};")),
            "the sheet declares {token}: {value}"
        );
    }
    for (level, token) in [
        ("1", "--plane-canvas"),
        ("2", "--plane-modal"),
        ("3", "--plane-urgent"),
    ] {
        // The window's word is `level` since 2.5.0 (display-hive.md § 4.24,
        // § 2: `plane` per output is struck). The z-order TOKENS keep their
        // names -- they are the sheet's own vocabulary and depend on no exit.
        let selector = format!(
            ":where(.display-pane, .display-panel, .display-overlay)[data-level=\"{level}\"]"
        );
        assert!(
            rule(&sheet, &selector).contains(&format!("z-index: var({token})")),
            "level {level} is {token}"
        );
    }
    // The furniture hangs on nothing: a modal or an urgent window may not
    // cover the dock or the mark (R-23-2).
    assert!(
        rule(&sheet, ".display-dock {").contains("z-index: var(--plane-os)"),
        "the dock is the OS layer"
    );
    assert!(
        rule(&sheet, ".display-os {").contains("z-index: calc(var(--plane-os) + 1)"),
        "and the mark is one above it, said rather than left to document order"
    );
}

#[test]
fn the_single_numbers_gave_their_z_index_up() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    for (what, selector) in [
        ("the overlay", ".display-overlay {"),
        (
            "the focus rung",
            ":where(.display-pane, .display-panel, .display-overlay)[data-rung=\"focus\"] {",
        ),
        (
            "the urgent rung",
            ":where(.display-pane, .display-panel, .display-overlay)[data-rung=\"urgent\"] {",
        ),
    ] {
        assert!(
            !rule(&sheet, selector).contains("z-index:"),
            "{what} carries no z-index of its own any more: the plane does"
        );
    }
}

#[test]
fn the_modal_stands_centred_over_the_canvas() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    let modal = rule(
        &sheet,
        ".display-columns > [data-region] :is([data-level=\"2\"], [data-level=\"3\"]) {",
    );
    // The two `inset-*` are half the centring and were the half a string
    // check could lose: `position: fixed` with `translate: -50% -50%` and no
    // insets puts the modal into the viewport's top-left corner, off the
    // screen by half its own size -- and reads identically to a test that
    // only names those two. The clamp is the other guard: a modal that is
    // `min(100%, 100vw)` wide is a strip, and D-11 says a dialogue is not one.
    for decl in [
        "position: fixed",
        "inset-block-start: 50%",
        "inset-inline-start: 50%",
        "translate: -50% -50%",
        "inline-size: min(",
        "clamp(22rem, 44vw, 40rem)",
    ] {
        assert!(
            modal.contains(decl),
            "the modal is centred over the canvas, not a column in it: {decl}"
        );
    }
}

#[test]
fn the_blur_follows_the_plane_and_not_a_boolean() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    assert!(
        !sheet.contains("data-modal"),
        "the `modal` boolean no longer reaches the sheet: `layer` does (OR-F19)"
    );
    let modal = rule(
        &sheet,
        ".display-columns:has([data-level=\"2\"]) [data-level=\"1\"] {",
    );
    // The grade stands in a token, so the two settings that take blur away set a
    // zero instead of repeating these selectors (OR-H3.sheet.2).
    assert!(
        sheet.contains("--f-back-2: blur(4px);"),
        "the modal's filter is a token"
    );
    assert!(modal.contains("filter: var(--f-back-2)") && modal.contains("opacity: 0.72"));
    let urgent = rule(
        &sheet,
        ".display-columns:has([data-level=\"3\"]) :is([data-level=\"1\"], [data-level=\"2\"]) {",
    );
    assert!(
        sheet.contains("--f-back-3: blur(10px);"),
        "the urgent's filter is a token"
    );
    assert!(urgent.contains("filter: var(--f-back-3)") && urgent.contains("opacity: 0.5"));
    // Two grades, and two only: a third blur would be a third meaning nobody
    // declared. Counted as a declaration of its own, so the material's own
    // `backdrop-filter` and the blur inside the leave keyframe do not count.
    assert_eq!(
        rule_level_blurs(&sheet),
        2,
        "exactly two rule-level blurs, and both are plane rules"
    );
}
