//! Welle F -- the light on the mark (R-23-4, OR-F4).
//!
//! A closed dock hides its tiles, so something urgent or freshly touched can
//! be present and invisible. The mark is the only anchor a closed dock has,
//! so the dot sits on it. It is a pure function of the state -- a count the
//! curator writes, on a window's plane and its age -- and there is no "has
//! the person seen it" memory anywhere: R-23-4 forbids the place such a
//! marker would have to live, and time plus plane are enough.
//!
//! Skips when the templates do not ship (R2b).

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const SHEET: &str = "templates/display/compose/display-dna.css";
const COMPOSE: &str = "templates/display/compose/compose.py";

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

/// What the dot asks about the COUNT. Four values reach the attribute and only
/// one of them means light: no attribute at all and `"0"` are dark, a count is
/// light -- and the EMPTY string is dark too, because `unseen` is declared as
/// text (OR-F27) and an exit nobody counted for would otherwise glow for ever.
const UNSEEN: &str =
    ".display-os[data-unseen]:not([data-unseen=\"0\"]):not([data-unseen=\"\"])::after";

/// And what it asks about the DOCK (display-hive.md § 6.8: "as long as
/// `unseen > 0` and the dock on this output is actually closed, whether by
/// profile or by press"). Closed has two shapes, so the dot is lit by two
/// selectors and taken back by none: until 2.5.0 it was lit for every count and
/// unlit by `[data-dock="shown"]` and by `html[data-dock-open="1"]`, which reads
/// like the same sentence and is not -- a monitor whose dock a finger had just
/// closed still carries `data-dock="shown"`, so the dot stayed dark beside a
/// shut dock.
/// Both ask <html> POSITIVELY: no attribute yet means nothing was pressed and
/// the profile decides; `"0"` means a finger closed it, whatever the profile
/// ships. The `:not()` that is left holds ONE compound -- a complex
/// `:not(html[…] *)` needs Selectors 4, and an engine that cannot parse it
/// drops the whole rule, which here is the dot itself on every output at once.
const BY_PROFILE: &str = "html:not([data-dock-open]) .display-columns[data-dock=\"hidden\"]";
const BY_PRESS: &str = "html[data-dock-open=\"0\"] .display-columns";

#[test]
fn the_dot_sits_on_the_mark_and_only_while_the_dock_is_shut() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    let dot = rule(&sheet, &format!("{BY_PROFILE} {UNSEEN}"));
    for decl in [
        "content: \"\"",
        "position: absolute",
        // A token, not a colour: the forced-colours block replaces the value
        // and can no longer win on specificity (§ 13, and the case below).
        "background-color: var(--dot-fill",
    ] {
        assert!(dot.contains(decl), "the dot is a dot: {decl}");
    }
    assert!(
        dot.contains("var(--scale)"),
        "and it scales with the screen like everything else: {dot}"
    );
    // The dot sits over the top right corner of the mark, and the press
    // handlers hang on the button inside it: a pseudo-element that took the
    // pointer would swallow the hold exactly while it is lit.
    assert!(
        dot.contains("pointer-events: none"),
        "and it never takes the gesture off the button under it: {dot}"
    );
}

#[test]
fn an_open_dock_gives_no_light_at_all() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    // The four states of one output, read off the two selectors that light the
    // dot: dock hidden by profile and untouched -> lit; hidden and pressed open
    // -> dark (the `:not()` takes it); shown by profile -> dark; shown and
    // pressed closed -> lit (the second selector). Nothing switches it off
    // again, because nothing has to.
    assert!(
        sheet.contains(&format!("{BY_PROFILE} {UNSEEN}")),
        "the dot is dark where the profile ships the dock closed"
    );
    assert!(
        sheet.contains(&format!("{BY_PRESS} {UNSEEN}")),
        "the dot is dark where a finger closed a dock the profile ships open -- \
         `tapDock()` writes `html[data-dock-open=\"0\"]` and leaves the root's own \
         `data-dock` alone, so a rule that only reads the root cannot see it"
    );
    for part in [BY_PROFILE, BY_PRESS] {
        assert!(
            !part.contains("] *)"),
            "`{part}` asks a complex `:not()` -- Selectors 4, and an engine that \
             cannot parse it drops the rule and with it the dot"
        );
    }
    assert!(
        !sheet.contains("content: none"),
        "the dot is lit and taken back again -- § 6.8 is one condition, and the \
         case it lost the last time was the one the two exceptions could not see"
    );
}

/// Forced colours: the OS owns every colour and the dot has to be the system's
/// own signal (§ 13). Since § 6.8 put the dock into the dot's own selector, that
/// selector weighs a class and five attributes -- more than anything the block
/// could write -- and `forced-color-adjust: none` would have kept the author's
/// accent on a screen whose owner replaced every colour. So the fill travels as
/// a custom property: a declaration on the same pseudo-element wins over the
/// inherited one whatever the weights are.
#[test]
fn the_dot_keeps_the_systems_colour_where_the_system_owns_it() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    let lit = rule(&sheet, &format!("{BY_PROFILE} {UNSEEN}"));
    assert!(
        lit.contains("background-color: var(--dot-fill"),
        "the dot paints a colour of its own instead of spending a token the \
         forced-colours block can replace: {lit}"
    );
    let forced = sheet
        .split("@media (forced-colors: active)")
        .nth(1)
        .expect("the sheet has a forced-colours block")
        .split("\n}")
        .next()
        .unwrap();
    let dot = rule(forced, &format!("{UNSEEN} {{"));
    assert!(
        dot.contains("--dot-fill: Highlight"),
        "the block does not replace the dot's fill: {dot}"
    );
    assert!(
        dot.contains("forced-color-adjust: none"),
        "and it has to say that the replacement is deliberate: {dot}"
    );
}

#[test]
fn nothing_remembers_that_it_was_seen() {
    if !library_ships() {
        return;
    }
    let src = std::fs::read_to_string(repo(COMPOSE)).expect("compose.py");
    assert!(
        !src.contains("dock_seen"),
        "no seen-event and no seen-store: the dot is a function of the state (OR-F4)"
    );
}

#[test]
fn forced_colours_keep_the_dot() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    // The block, and not the rest of the file: everything after it would
    // hold a needle the block itself never carried.
    let forced = sheet
        .split("@media (forced-colors: active)")
        .nth(1)
        .expect("the sheet has a forced-colours block")
        .split("\n}")
        .next()
        .unwrap();
    assert!(
        forced.contains(UNSEEN),
        "a signal that only exists in one colour is a signal that can vanish, and a \
         bare `.display-os::after` here would never reach the dot's own rule: {forced}"
    );
    assert!(
        rule(forced, &format!("{UNSEEN} {{")).contains("Highlight"),
        "in forced colours the dot is the system's own signal colour: {forced}"
    );
}
