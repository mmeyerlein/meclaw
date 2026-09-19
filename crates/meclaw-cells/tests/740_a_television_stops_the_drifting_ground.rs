//! GH #740 — the television output stops the drifting ground and drops the grain.
//!
//! Measured (`plans/welle-h3-2026-09-18/messungen/G-rendering.md` § c, 18.09.2026):
//! headless Chromium at 3840x2160 against the live `/tv` page with CDP CPU
//! throttling 6 — a Pi approximation — runs at **5 fps while nothing happens**,
//! before any window changes. The owner's words were "the television is very
//! slow at building up and tearing down windows"; the screen is that slow all
//! the time, and it only shows where one waits for movement.
//!
//! The cause is not the glass, not the level blur, not the view transitions and
//! not the window keyframes. It is the two eternal layers under everything:
//! `body { animation: display-light-drift … infinite }` moves the
//! `background-position` of three gradients — not a compositable property, so a
//! full repaint of 8.3 megapixels every frame, for ever — and `body::after`
//! (the grain, `mix-blend-mode: overlay`) is re-blended into every one of them.
//! With only those two off: 60 fps, and a window settles in 84–223 ms with the
//! glass and the blur left exactly as they are.
//!
//! | variant                              | median frame | fps  |
//! |--------------------------------------|--------------|------|
//! | the sheet as it was                  | 199.9 ms     |  5.0 |
//! | `animation: none` + `::after: none`  |  16.7 ms     | 59.9 |
//!
//! A television is a profile, not a width (§ 6.6), so the condition is the exit
//! the curator writes: `data-exit` stands on the columns, and `body` is their
//! ancestor, which is what `:has()` is for.
//!
//! A file-text lock. What it guards is that the two rules keep naming the tv
//! exit and keep saying the two words that cost the frames; the frame rate
//! itself is measured with the scripts beside the report.
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

/// The declarations of the first rule whose selector list carries `sel`.
fn rule(sheet: &str, sel: &str) -> String {
    let at = sheet
        .find(sel)
        .unwrap_or_else(|| panic!("no rule carries the selector `{sel}`"));
    let open = sheet[at..]
        .find('{')
        .unwrap_or_else(|| panic!("`{sel}` opens no block"));
    let close = sheet[at + open..]
        .find('}')
        .unwrap_or_else(|| panic!("`{sel}` never closes"));
    sheet[at + open + 1..at + open + close].to_string()
}

/// The ground of a television does not drift, and it carries no grain.
#[test]
fn the_television_ground_neither_drifts_nor_grains() {
    if !library_ships() {
        return;
    }
    let sheet = read(SHEET);
    let ground = rule(&sheet, "body:has([data-exit=\"tv\"]) {");
    assert!(
        ground.contains("animation: none"),
        "the drifting ground still repaints 8.3 megapixels a frame on the one \
         output that cannot pay for it: {ground}"
    );
    let grain = rule(&sheet, "body:has([data-exit=\"tv\"])::after {");
    assert!(
        grain.contains("display: none"),
        "the grain is still blended into every one of those repaints: {grain}"
    );
}

/// And nothing else of the television is taken away with them.
///
/// The measurement is explicit that the glass and the two level blurs cost
/// nothing there (§ c, variants v5 and v9). A television that lost its material
/// would be a different screen, not a faster one.
///
/// Read over the WHOLE sheet, not over the one rule above: a test that asks a
/// single-declaration rule whether it contains three other words cannot go red,
/// and a lock that cannot go red costs reading time and buys nothing (wave H3
/// review). Every rule whose selector names the tv exit is collected, and the
/// three words are looked for in all of them together.
#[test]
fn the_television_keeps_its_glass_and_its_blur() {
    if !library_ships() {
        return;
    }
    let sheet = read(SHEET);
    let mut said = String::new();
    let mut rest = sheet.as_str();
    while let Some(at) = rest.find("[data-exit=\"tv\"]") {
        // From the start of that selector's line to the end of its block.
        let line = rest[..at].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let end = rest[at..]
            .find('}')
            .map(|i| at + i + 1)
            .unwrap_or(rest.len());
        said.push_str(&rest[line..end]);
        rest = &rest[end..];
    }
    assert!(
        said.contains("animation: none"),
        "no rule of the television says anything: the search found {} bytes",
        said.len()
    );
    for taken in [
        "backdrop-filter: none",
        "--f-back-2: none",
        "--glass-opaque",
    ] {
        assert!(
            !said.contains(taken),
            "`{taken}` is not what the television pays for (G-rendering § c): {said}"
        );
    }
}

/// And the sheet reached the two shipped copies.
#[test]
fn the_rules_reached_the_shipped_copy() {
    if !library_ships() {
        return;
    }
    let compose = read(COMPOSE);
    assert!(
        compose.contains("body:has([data-exit=\"tv\"])::after"),
        "`scripts/display_sync.py` has not run: `compose.py` carries an older sheet"
    );
}
