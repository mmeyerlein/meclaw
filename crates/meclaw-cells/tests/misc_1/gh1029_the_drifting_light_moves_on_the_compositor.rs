//! GH #1029 -- the drifting light moves on the compositor, not by repainting the screen.
//!
//! Measured (display before-run, headless with software GL beside an
//! empty control page at 60 fps): monitor 15-20 fps and phone (WebKit) 3-5 fps while
//! NOTHING happened on the screen. The cause is the one eternal animation of the ground:
//! `body { animation: display-light-drift … infinite }` moved the `background-position`
//! of the body's gradients. `background-position` is not compositable, so every frame
//! repainted the whole output, for ever (the same mechanism GH #740 took off the
//! television; monitor and phone kept paying, and a phone pays in battery too).
//!
//! The light keeps drifting -- motion is part of the screen -- but as its own layer that
//! moves by `transform`, which the compositor does without a repaint. The television and
//! `prefers-reduced-motion` keep it still.
//!
//! A file-text lock in the form of `740_a_television_stops_the_drifting_ground`: it
//! guards that the drift names only a compositable property and stands on its own layer.
//! The frame rate itself is measured by the display lab.
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

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// The declarations of the first rule whose selector line is exactly `sel` followed by
/// ` {` at the start of a line.
fn rule(sheet: &str, sel: &str) -> String {
    let head = format!("\n{sel} {{");
    let at = sheet
        .find(&head)
        .unwrap_or_else(|| panic!("no rule `{sel}` at the start of a line"));
    let body = &sheet[at + head.len()..];
    let close = body
        .find('}')
        .unwrap_or_else(|| panic!("`{sel}` never closes"));
    body[..close].to_string()
}

/// The whole `@keyframes display-light-drift` block (it nests one level of braces).
fn keyframes(sheet: &str) -> String {
    let head = "@keyframes display-light-drift {";
    let at = sheet
        .find(head)
        .unwrap_or_else(|| panic!("no `{head}` in the sheet"));
    let rest = &sheet[at..];
    let end = rest.find("\n}").expect("the keyframes close");
    rest[..end].to_string()
}

/// The drift moves `transform` and nothing a repaint pays for.
#[test]
fn the_drift_moves_only_a_transform() {
    if !library_ships() {
        return;
    }
    let sheet = read(SHEET);
    let frames = keyframes(&sheet);
    assert!(
        !frames.contains("background-position"),
        "the drift still moves `background-position`: a full repaint every frame: {frames}"
    );
    assert!(
        frames.contains("transform:"),
        "the drift no longer moves at all -- the light has to keep drifting: {frames}"
    );
}

/// And it stands on its own layer, never on `body` itself.
#[test]
fn the_light_is_its_own_layer() {
    if !library_ships() {
        return;
    }
    let sheet = read(SHEET);
    let ground = rule(&sheet, "body");
    assert!(
        !ground.contains("display-light-drift"),
        "`body` itself still drifts: {ground}"
    );
    let light = rule(&sheet, "body::before");
    for word in [
        "animation: display-light-drift",
        "position: fixed",
        "pointer-events: none",
        "will-change: transform",
        "radial-gradient(",
    ] {
        assert!(
            light.contains(word),
            "the light layer lacks `{word}`: {light}"
        );
    }
}

/// The television and a reader who asked for less motion keep the light still.
#[test]
fn the_television_and_reduced_motion_keep_it_still() {
    if !library_ships() {
        return;
    }
    let sheet = read(SHEET);
    let tv = rule(&sheet, "body:has([data-exit=\"tv\"])::before");
    assert!(
        tv.contains("animation: none"),
        "the television drifts again: {tv}"
    );
    let reduced = sheet
        .split("@media (prefers-reduced-motion: reduce) {")
        .nth(1)
        .expect("a reduced-motion block");
    let reduced = &reduced[..reduced.find("\n}").expect("the block closes")];
    assert!(
        reduced.contains("body::before { animation: none; }"),
        "reduced motion still drifts the light"
    );
}

/// And the sheet reached the shipped copy.
#[test]
fn the_light_reached_the_shipped_copy() {
    if !library_ships() {
        return;
    }
    let compose = read(COMPOSE);
    assert!(
        compose.contains("body:has([data-exit=\"tv\"])::before"),
        "`scripts/display_sync.py` has not run: `compose.py` carries an older sheet"
    );
}
