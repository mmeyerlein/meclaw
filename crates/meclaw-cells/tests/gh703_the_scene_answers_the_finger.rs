//! Welle F -- the screen answers the finger before the colony does (F-14),
//! and it empties the line a person typed (E-10, OR-F17).
//!
//! Two small pieces of client, and both have a sharp edge:
//!
//! * The tile press says only "pressed". What the tap MEANS -- focus, or back
//!   into the tile -- is the server's word and arrives one pass later. A
//!   client that guessed the rung here would fight the next diff, which is
//!   exactly what E-17 looked like on the device.
//! * The input is emptied in a MACROTASK of its own, never synchronously in
//!   the handler. The vendored LiveView binds `keyup` on `window` rather than
//!   on the container, and in the bubble phase `document` runs BEFORE
//!   `window`; LiveView reads the field's value synchronously when it pushes.
//!   Emptying it in the handler would hand the server an empty string, and a
//!   sentence a person typed would never become a turn (OR-F48).
//!
//! Skips when the templates do not ship (R2b).

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";
const SHEET: &str = "templates/display/compose/display-dna.css";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

fn script() -> String {
    std::fs::read_to_string(repo(COMPOSE))
        .expect("compose.py")
        .replace("\\\"", "\"")
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
fn the_press_is_a_ring_and_nothing_else() {
    if !library_ships() {
        return;
    }
    let src = script();
    assert!(
        src.contains("el.addEventListener(\"pointerdown\", press, true)"),
        "capture phase, ahead of LiveView's own click path"
    );
    let press = src
        .split("var press = function (e) {")
        .nth(1)
        .expect("the press handler")
        .split("};")
        .next()
        .unwrap();
    // Against the HANDLER, not the file: the same selector stands three times
    // in `compose.py`, twice of them in the embedded sheet, so a whole-file
    // needle would hold even if the handler stopped naming it.
    assert!(
        press.contains(".display-tile[phx-click]"),
        "the hook answers only tiles that actually take a tap (OR-F18): {press}"
    );
    assert!(
        press.contains("setAttribute(\"data-zoomed\", \"true\")"),
        "it says pressed: {press}"
    );
    // And the effect of the tap, drawn before the pass answers (§ 5.7): the
    // press itself says "pressed" and hands the window to the drawing, which
    // is the only place that writes the curator's words. Until 2.5.0 this read
    // the other way round -- "the client never guesses" (E-17) -- because
    // there was nothing here that the pass would confirm.
    assert!(
        press.contains("optimistic(el, t, st)"),
        "the press hands the tap to the optimistic drawing (§ 5.7): {press}"
    );
    for guess in [
        "data-state",
        "data-plane",
        "data-on-canvas",
        "data-rank",
        "data-pinned",
    ] {
        assert!(
            !press.contains(guess),
            "and the press itself writes none of the curator's words, least of \
             all a struck one (§ 2): {guess} in {press}"
        );
    }
}

#[test]
fn enter_empties_the_line_after_liveview_has_read_it() {
    if !library_ships() {
        return;
    }
    let src = script();
    assert!(
        src.contains("document.addEventListener(\"keyup\", typed)"),
        "on the document, in the bubble phase (OR-F17)"
    );
    // Both spellings of the capture phase: `true` and the options object.
    for capture in [
        "document.addEventListener(\"keyup\", typed, true)",
        "document.addEventListener(\"keyup\", typed, { capture: true })",
        "document.addEventListener(\"keyup\", typed, {capture: true})",
    ] {
        assert!(
            !src.contains(capture),
            "NOT in the capture phase, or it would empty the field before it is read: {capture}"
        );
    }
    let typed = src
        .split("var typed = function (e) {")
        .nth(1)
        .expect("the keyup handler")
        .split("};")
        .next()
        .unwrap();
    assert!(
        typed.contains("e.key !== \"Enter\"") && typed.contains("display-input-field"),
        "one key, one class: {typed}"
    );
    // The measured shape (OR-F48): the vendored LiveView binds `keyup` on
    // `window`, `document` runs before `window` in the bubble phase, and
    // LiveView reads the value synchronously when it pushes. A macrotask of
    // its own runs after EVERY synchronous handler, wherever LiveView hangs.
    assert!(
        typed.contains("root.setTimeout(function () { f.value = \"\"; }, 0);"),
        "the field is emptied in a macrotask of its own: {typed}"
    );
    let without = typed.replace("root.setTimeout(function () { f.value = \"\"; }, 0);", "");
    assert!(
        !without.contains("f.value = \"\""),
        "and NEVER synchronously in the handler, where LiveView has not read it yet: {typed}"
    );
}

#[test]
fn both_listeners_are_given_back() {
    if !library_ships() {
        return;
    }
    let src = script();
    // LiveView re-mounts a hook after a reconnect, and a wall screen
    // reconnects all day: two listeners per life is a leak with a clock on it.
    assert!(
        src.contains("el.removeEventListener(\"pointerdown\", press, true)")
            && src.contains("document.removeEventListener(\"keyup\", typed)"),
        "destroyed() gives every life back"
    );
}

#[test]
fn the_sheet_dresses_the_tap_and_the_line() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    assert!(
        rule(&sheet, ".display-tile[phx-click] {").contains("cursor: pointer"),
        "a tile that answers a finger says so"
    );
    assert!(
        sheet.contains(".display-tile[phx-click]:active"),
        "and it answers in the same frame the finger lands"
    );
    // The two classes F1 added to the catalogue: `every_class_the_screen
    // _writes_has_a_rule` is the lock, this is the statement.
    for selector in [".display-input {", ".display-input-field {"] {
        assert!(
            sheet.contains(selector),
            "the line a person types into has a rule: {selector}"
        );
    }
    // The floor is not cosmetic (OR-F49): `--t-body` is `calc(16px *
    // var(--scale))` and the smallest scale is 0.8, so a phone reaches
    // 12.8px -- and iOS Safari zooms the whole page when a field under 16px
    // takes focus, and does not zoom back out by itself.
    assert!(
        rule(&sheet, ".display-input-field {").contains("font-size: max(16px, var(--t-body))"),
        "the screen's type, with a floor no mobile engine will zoom past"
    );
    assert!(
        sheet.contains(".display-input-field:focus-visible"),
        "a field a keyboard can reach shows where the keyboard is"
    );
    // A field is a control, so it keeps a readable ground where the glass goes.
    let reduced = sheet
        .split("@media (prefers-reduced-transparency: reduce)")
        .nth(1)
        .expect("the sheet has a reduced-transparency block")
        .split("@media")
        .next()
        .unwrap();
    assert!(
        reduced.contains(".display-input-field {"),
        "the line a person types into keeps a solid ground where glass is refused: {reduced}"
    );
}

/// One rule per class, and one only (OR-F26).
///
/// The floor ships a seam at the end of the sheet -- minimal rules for the
/// three classes its own templates write -- so that its class lock has
/// something to find while this strand is still a branch. This strand deletes
/// that seam when it rebases, and the deletion is the part that can silently
/// not happen: a seam that is merged instead of removed keeps every other test
/// in this file green, because `rule()` reads the FIRST hit, and then wins in
/// the browser, which reads the last.
///
/// Counted at the start of a line, and that is not pedantry: the same classes
/// stand indented inside `@media (prefers-reduced-transparency: reduce)`, so a
/// bare `matches(".display-input-field {")` would count two on a sheet that is
/// right.
#[test]
fn every_class_this_strand_dresses_has_exactly_one_top_level_rule() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    for selector in [
        "\n.display-input {",
        "\n.display-input-field {",
        "\n.display-tile-unit {",
    ] {
        assert_eq!(
            sheet.matches(selector).count(),
            1,
            "one rule, not two: the floor's seam is deleted, not merged -- {selector:?}"
        );
    }
    assert!(
        !sheet.contains("seam: F2 replaces this block"),
        "and the seam's own marker is gone with it"
    );
}
