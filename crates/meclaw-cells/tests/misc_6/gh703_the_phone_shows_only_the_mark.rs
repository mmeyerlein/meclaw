//! Welle F -- on a phone the dock is closed until a hand opens it (R-23-4).
//!
//! Three things this sheet has to get right, and all three were measured on
//! the device in Welle E rather than reasoned about:
//!
//! 1. Hidden is `display: none`. A tile carries its own `backdrop-filter`, a
//!    backdrop layer is the compositor's own decision, and WebKit painted
//!    such layers straight through an ancestor's `opacity`, measured on a
//!    device. `opacity: 0` is a request; `display: none` is a box that was
//!    never made.
//! 2. The ground state comes from the server (`data-dock` in the dead
//!    render, out of the profile), the OPENING from the client
//!    (`data-dock-open` on <html>). A preset that lives only in the sheet is
//!    not a state: every patch re-rendered the born value and the tiles
//!    blinked away, measured on a device (OR-F5).
//! 3. No width query undoes a profile. A television is far, not small.
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
fn a_hidden_dock_is_not_drawn_at_all() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    // The closed dock stands in ONE rule with both its cases since GH #741, so
    // the selector to look for is the pair -- a lone
    // `.display-columns[data-dock="hidden"] .display-dock {` now matches the
    // OPENING rule first, whose selector ends in the same three words.
    let hidden = rule(
        &sheet,
        ".display-columns[data-dock=\"hidden\"] .display-dock,\n\
         html[data-dock-open=\"0\"] .display-columns[data-dock=\"shown\"] .display-dock {",
    );
    assert!(
        hidden.contains("display: none"),
        "a hidden dock is a box that was never made: {hidden}"
    );
    // It ENDS as a box that was never made and never as a request to the
    // compositor (R-23-4). Since GH #741 it carries an `opacity` beside the
    // `display` -- that is the movement, and `allow-discrete` is what keeps the
    // end state discrete all the same.
    assert!(
        !hidden.contains("visibility"),
        "and never a request to the compositor (R-23-4): {hidden}"
    );
    // The gutter the canvas holds free for the dock belongs to the canvas
    // while there is no dock -- 120 px of 393 otherwise.
    assert!(
        rule(&sheet, ".display-columns[data-dock=\"hidden\"] {")
            .contains("padding-inline-end: calc(var(--pad-window) - var(--gutter))"),
        "a canvas with no dock beside it keeps no lane free for one (the gutter \
         itself lives in the scroller since GH #739)"
    );
}

#[test]
fn the_opening_is_the_clients_and_lives_outside_the_container() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    const OPENING: &str =
        "html[data-dock-open=\"1\"] .display-columns[data-dock=\"hidden\"] .display-dock {";
    // The BODY of the rule, not only its selector: a rule that opens nothing
    // would read the same from the outside.
    let opening = rule(&sheet, OPENING);
    assert!(
        opening.contains("display: flex"),
        "the opening hangs on <html>, where no LiveView patch reaches, and it opens (OR-F5): {opening}"
    );
    // It arrives rather than appearing -- since GH #741 as a TRANSITION and no
    // longer as a keyframe, so that the same movement runs in both directions
    // and on every output, not only into a phone.
    assert!(
        !sheet.contains("@keyframes display-dock-in"),
        "the one-way keyframe is gone"
    );
    assert!(
        rule(&sheet, ".display-dock {").contains("display var(--t-dock) allow-discrete"),
        "and `display: none` stays the closed end state while it moves (§ 6.7)"
    );
    assert!(
        sheet.contains("@starting-style"),
        "a dock opening for the first time has a state to come from"
    );
    let motion = sheet
        .split("@media (prefers-reduced-motion: reduce)")
        .nth(1)
        .expect("the sheet has a reduced-motion block");
    assert!(
        rule(motion, ".display-dock {").contains("transition: none !important"),
        "under reduced motion the dock is there or it is not, and the base sheet's \
         !important has to be shouted back at: {motion}"
    );
}

#[test]
fn the_furniture_stays_out_of_the_home_indicator() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    assert!(
        rule(&sheet, ".display-os {").contains("env(safe-area-inset-bottom, 0px)"),
        "the mark is the one thing the home indicator reaches"
    );
    assert!(
        rule(&sheet, ".display-dock {").contains("env(safe-area-inset-bottom, 0px)"),
        "and the dock stands on the mark, so it inherits the distance"
    );
    // A second declaration, not a @supports: an engine that does not know
    // `dvh` drops the line and keeps `100vh`.
    assert!(
        sheet.contains("min-height: 100vh;\n  min-height: 100dvh;"),
        "`100vh` on iOS is the height without the address bar"
    );
}

#[test]
fn the_phone_has_rules_of_its_own() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    const PROFILE: &str = ".display-columns[data-exit=\"phone\"] {";
    const DOCK_HIDDEN: &str = ".display-columns[data-dock=\"hidden\"] {";
    let air = rule(&sheet, PROFILE);
    // Since GH #739 the air is a TOKEN, because it is not spent here: the canvas
    // carries it as its own padding, inside the edge it clips at. The cascade
    // trap the shorthand used to set -- a later 0-2-0 `padding` overwriting the
    // `padding-inline-end` of this rule -- went with it.
    assert!(
        air.contains("--gutter: 14px"),
        "a phone gets its own air: {air}"
    );
    assert!(
        !air.contains("padding"),
        "and it does not spend it here any more: {air}"
    );
    // The ORDER still holds: both dock rules are 0-2-0 and the later one wins.
    assert!(
        sheet.find(PROFILE) < sheet.find(DOCK_HIDDEN),
        "the profile's air stands before the dock rules that correct it"
    );
    // The second phone rule, by its content rather than by counting needles:
    // a window in focus on 393 px is not a 22rem box with a margin around it.
    // Named in full: since 2.5.0 the phone's arrangement (§ 6.3) opens with a
    // rule on the region itself, and the first match would be that one.
    let box_rule = rule(
        &sheet,
        ".display-columns[data-exit=\"phone\"] > [data-region]\n  :where(",
    );
    assert!(
        box_rule.contains("max-inline-size: 100%"),
        "and a window on a phone is as wide as the phone: {box_rule}"
    );
}
