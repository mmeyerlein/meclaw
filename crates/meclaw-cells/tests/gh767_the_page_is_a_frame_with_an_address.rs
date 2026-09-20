//! The sheet's § 6c: a page is a frame with an address under it (GH #767).
//!
//! A frame around a picture rendered somewhere else has to STAND before the
//! first picture arrives -- a window without a stream stands all the same
//! (display-hive.md § 6.14) -- and it has to stand at the page's proportions,
//! or every click in it lands somewhere else than it looked.
//!
//! Five statements, each with its own case below: the four declarations on the
//! canvas that carry that; the three states a person can see without reading,
//! and the one state that says nothing; the two focus rules, which are not the
//! same rule twice; the line a refusal is written into, which takes no room
//! when there is none; and that every one of the seven rules stands exactly
//! once at the top level, because the same class names stand INDENTED inside
//! the reduced-transparency block and a count that did not say where it looked
//! would read them as duplicates.
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

fn sheet() -> Option<String> {
    std::fs::read_to_string(repo(SHEET)).ok()
}

/// The body of one top-level rule, by its exact selector at the start of a line.
fn rule<'a>(css: &'a str, selector: &str) -> &'a str {
    let head = format!("\n{selector} {{");
    let at = css
        .find(&head)
        .unwrap_or_else(|| panic!("the sheet has no top-level rule `{selector}`"));
    let from = at + head.len();
    let to = css[from..]
        .find('}')
        .unwrap_or_else(|| panic!("`{selector}` is never closed"));
    &css[from..from + to]
}

/// How often a selector opens a rule of its own at the START of a line.
///
/// The line start is what makes this a count and not a guess: the same class
/// names stand INDENTED inside the reduced-transparency block, and a plain
/// `contains` would read them as a second rule.
fn top_level(css: &str, selector: &str) -> usize {
    css.match_indices(&format!("\n{selector} {{")).count()
}

/// The four declarations the picture cannot do without.
#[test]
fn the_canvas_takes_the_width_and_the_pages_proportions() {
    if !library_ships() {
        return;
    }
    let Some(css) = sheet() else {
        return;
    };
    let view = rule(&css, ".display-browser-view");
    for needle in [
        // The window decides the width, and the curator decides the window.
        "inline-size: 100%",
        // The height before the first frame, and the hook writes the property.
        "aspect-ratio: var(--browser-ratio, 16 / 10)",
        // A page is text, and text reads better softly scaled.
        "image-rendering: auto",
        // Or the viewer's own browser eats the first swipe as a scroll.
        "touch-action: none",
    ] {
        assert!(
            view.contains(needle),
            "the page canvas declares `{needle}`: {view}"
        );
    }
}

/// Three states that are seen, and one that is not said.
#[test]
fn the_three_states_are_visible_and_ready_says_nothing() {
    if !library_ships() {
        return;
    }
    let Some(css) = sheet() else {
        return;
    };
    for state in ["loading", "suspended", "error"] {
        assert!(
            css.contains(&format!(".display-browser[data-page-state=\"{state}\"]")),
            "`{state}` is a state a person can see"
        );
    }
    // A suspended page shows the LAST picture before it was closed, so it is
    // drained of colour as well as dimmed: dimming alone reads as "loading".
    let suspended = rule(
        &css,
        ".display-browser[data-page-state=\"suspended\"] .display-browser-view",
    );
    assert!(
        suspended.contains("grayscale"),
        "a suspended page is not to be mistaken for a living one: {suspended}"
    );
    assert!(
        !css.contains(".display-browser[data-page-state=\"ready\"]"),
        "a page that works is a page, not a status"
    );
    // The second voice, on its own attribute: a page whose join the cell
    // turned down is broken from where the reader sits, whatever the
    // application still believes about the cell (OR-G.g18.1).
    let down = rule(
        &css,
        ".display-browser[data-page-link=\"down\"] .display-browser-view",
    );
    assert!(
        down.contains("inset 0 0 0 2px"),
        "a channel that does not stand wears the same outline: {down}"
    );
    assert!(
        !css.contains(".display-browser[data-page-link=\"up\"]"),
        "and a channel that stands says nothing, the same way `ready` does"
    );
}

/// Two focus rules, and the hidden field they belong to.
#[test]
fn both_focus_rules_stand_and_the_field_is_rendered_but_unseen() {
    if !library_ships() {
        return;
    }
    let Some(css) = sheet() else {
        return;
    };
    // The canvas a Tab reached.
    assert!(
        css.contains(".display-browser-view:focus-visible"),
        "the canvas shows where the keyboard is"
    );
    // And the mark the hook writes while the hidden field holds the focus. The
    // field hangs on `document.body`, outside the figure, so `:has()` on the
    // figure could never see it -- the two rules are not one rule twice.
    assert!(
        css.contains(".display-browser[data-keys=\"true\"] .display-browser-view"),
        "the hook's own focus mark has a rule"
    );
    let keys = rule(&css, ".display-browser-keys");
    assert!(
        keys.contains("position: fixed") && keys.contains("opacity: 0"),
        "the field is off the screen and invisible: {keys}"
    );
    for gone in ["display: none", "visibility: hidden"] {
        assert!(
            !keys.contains(gone),
            "a box that is not rendered takes no focus and opens no keyboard \
             on a telephone: {keys}"
        );
    }
}

/// The refusal is a line a person reads, and an empty one takes no room.
#[test]
fn the_refusal_has_a_rule_and_disappears_when_there_is_none() {
    if !library_ships() {
        return;
    }
    let Some(css) = sheet() else {
        return;
    };
    let reason = rule(&css, ".display-browser-reason");
    // In the accent, beside the address the error state already puts forward:
    // the two together are the whole repair -- which page, and what is wrong
    // with it.
    assert!(
        reason.contains("var(--accent)"),
        "the refusal is said in the colour of the error state: {reason}"
    );
    // The span ships on EVERY page, empty. A flex row with a gap would keep a
    // gap for it, so an empty one leaves the row.
    assert!(
        css.contains(".display-browser-reason:empty"),
        "a page that works shows no room for a refusal"
    );
}

/// Every one of the seven rules stands exactly once at the top level.
#[test]
fn each_of_the_six_classes_has_one_rule_of_its_own() {
    if !library_ships() {
        return;
    }
    let Some(css) = sheet() else {
        return;
    };
    for class in [
        ".display-browser",
        ".display-browser-view",
        ".display-browser-bar",
        ".display-browser-title",
        ".display-browser-url",
        ".display-browser-keys",
        ".display-browser-reason",
    ] {
        assert_eq!(
            top_level(&css, class),
            1,
            "`{class}` opens exactly one rule of its own at the top level"
        );
    }
}
