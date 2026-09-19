//! GH #722 -- § 5.4's "says the refusal" was said to the screen reader alone.
//!
//! Measured (the owner, human test of the fresh instance, 18.09.2026, row M4b,
//! laptop on a LAN
//! address over plain http with no microphone): *"dims, but no visible text
//! 'microphone needs https'"*. And in both engines, in the table of
//! `plans/welle-h-2026-09-17/berichte/LIVE-hold-no-mic.md` § 2: a 900 ms hold
//! leaves the refusal in the live region -- and since
//! #719/#720 `data-phase="error"` and the mark at `--mark-dim` -- which is
//! everything § 5.4 asks for except the part a person can see. The sheet hides
//! `.display-os-state` with `clip-path: inset(50%)`, so the only reader of the
//! refusal is a screen reader, and the dimming alone names no reason.
//!
//! OR-H2.1 (wave H2): while `#display-os[data-phase="error"]` stands, the same
//! line the hook already writes is DRAWN -- one line, at the mark's trailing
//! edge, inside the safe area, nothing behind it, the mark's own dimming. Every
//! other phase keeps the quiet live region it has today.
//!
//! This is rendering (§ 6) and no field anywhere: the phase is client state of
//! the same kind as the listening glow (§ 6.8, #720), and the caption lives
//! exactly as long as the phase does -- what ends the phase is a gesture or a
//! device, never a clock.
//!
//! A file-text lock. What the caption actually measures to in a browser is
//! B-34, in both engines.
//!
//! Skips when the templates do not ship (R2b).

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const SHEET: &str = "templates/display/compose/display-dna.css";
const COMPOSE: &str = "templates/display/compose/compose.py";
const DRIVER: &str = "workshop/tools/display-layout-browser.mjs";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// The declarations of the FIRST rule whose selector list carries `sel`.
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

const VISIBLE: &str = ".display-columns[data-inputs~=\"audio\"] \
     .display-os[data-phase=\"error\"] .display-os-state";

/// The refusal is drawn while the phase stands.
#[test]
fn the_error_phase_draws_the_line_the_hook_wrote() {
    if !library_ships() {
        return;
    }
    let sheet = read(SHEET);
    let body = rule(&sheet, VISIBLE);
    // The three ways the quiet rule hides it, each of them undone by name: a
    // declaration the later rule does not repeat is simply inherited from the
    // rule above, and then nothing is visible at all.
    assert!(
        body.contains("clip-path: none"),
        "the caption is still clipped to nothing -- `clip-path: inset(50%)` of \
         the quiet rule stands until it is written over: {body}"
    );
    assert!(
        body.contains("block-size: auto"),
        "the caption keeps the quiet rule's `block-size: 1px`: {body}"
    );
    assert!(
        body.contains("overflow:"),
        "the caption does not restate `overflow` -- with a box that has a size \
         again, `hidden` is what cuts a sentence that will not fit instead of \
         letting it run off the leading edge: {body}"
    );
    for hidden in ["display: none", "visibility: hidden", "opacity: 0;"] {
        assert!(
            !body.contains(hidden),
            "`{hidden}` in the rule that is supposed to SHOW the refusal: {body}"
        );
    }
}

/// And what it says names what is missing and where the browser read it.
///
/// GH #741 (3): the caption used to read "microphone needs https or localhost",
/// and the owner read `localhost` as an address to go to -- it is not; the page
/// he was on is a LAN address over plain http, and the address that WOULD work
/// lives in a proxy the colony knows nothing about. So the client says the two
/// things it knows for certain: that https is what is missing, and which page
/// this is. It never invents an address it cannot verify.
#[test]
fn the_refusal_names_https_and_the_page_it_was_read_on() {
    if !library_ships() {
        return;
    }
    let compose = read(COMPOSE);
    let at = compose
        .find("isSecureContext")
        .expect("the client still refuses an insecure context");
    let line = &compose[at..compose[at..].find('\n').map_or(compose.len(), |i| at + i)];
    assert!(
        line.contains("the microphone needs https"),
        "the refusal names what is missing: {line}"
    );
    assert!(
        line.contains("location.origin"),
        "the refusal names the page it was read on, out of the browser rather \
         than out of a setting somebody has to keep true: {line}"
    );
    assert!(
        !compose.contains("https or localhost"),
        "`localhost` reads as a signpost and points at the wrong place (#741)"
    );
}

/// And in every other phase it stays what it was: said, never shown.
#[test]
fn the_quiet_live_region_is_untouched() {
    if !library_ships() {
        return;
    }
    let sheet = read(SHEET);
    let quiet = rule(&sheet, ".display-os-state {");
    assert!(
        quiet.contains("clip-path: inset(50%)"),
        "the live region is no longer hidden outside the refusal: a screen that \
         reads `deepgram/cartesia` beside the mark at all times is what D-17 \
         took away: {quiet}"
    );
    // Not a tie-break: `.display-os-state` weighs (0,1,0) and the showing rule
    // (0,4,0), so specificity decides and the order cannot. What the order says
    // is which of the two is the BASE -- a reader meets the quiet rule first,
    // and the exception only has to name what it undoes because of that.
    assert!(
        sheet.find(".display-os-state {").unwrap() < sheet.find(VISIBLE).unwrap(),
        "the quiet rule is no longer the base the exception is written against"
    );
}

/// Where it stands: at the mark, inside the safe area, with nothing behind it.
#[test]
fn the_caption_stands_at_the_mark_and_inside_the_safe_area() {
    if !library_ships() {
        return;
    }
    let sheet = read(SHEET);
    let body = rule(&sheet, VISIBLE);
    assert!(
        body.contains("inset-inline-end: calc(100%"),
        "the caption does not stand BESIDE the mark, on its leading side \
         (OR-H2.1): {body}"
    );
    // ABOVE the mark is the one place it may not be: the dock ends there, with
    // `--dock-pad` between its last tile and the mark, and a caption in that gap
    // lay 11.6 px on the tile (GH #722, F1). § 6.11 lets no part of a tile go
    // under 0.88, and a half-opaque line drawn over one does exactly that.
    assert!(
        !body.contains("inset-block-end: 100%"),
        "the caption is hung over the mark again, where the dock ends (§ 12b, \
         § 6.11): {body}"
    );
    assert!(
        body.contains("1.3em"),
        "the caption does not sit on the mark's own middle, so it drifts with \
         the scale: {body}"
    );
    // And the gate the dimming of the same phase already spends (§ 6.4): a hold
    // -- and with it a refused hold -- exists only on an output whose profile
    // says `audio`.
    assert!(
        body.contains("var(--os)") && body.contains("var(--dock-gap)"),
        "the cap does not account for the mark and the gap it now stands beside, \
         so on a narrow phone it reaches past the leading inset: {body}"
    );
    // `.display-os` carries the trailing safe inset already; a box hung off its
    // inline-end inherits that side and runs off the LEADING one, which on a
    // phone is where a long sentence goes.
    assert!(
        body.contains("max-inline-size:") && body.contains("safe-area-inset-left"),
        "nothing stops the caption at the leading edge of a phone: § 6.7 and \
         B-17 want the safe area asked for wherever a box reaches it: {body}"
    );
    assert!(
        !body.contains("background"),
        "a plate under two words is the loudest thing on the canvas, and the \
         mark itself has none (§ 9, D-3): {body}"
    );
    assert!(
        body.contains("opacity: var(--mark-dim)"),
        "§ 5.4 dims the refused mark, and the caption is part of the same \
         refused thing -- one quantity, one name (§ 2): {body}"
    );
}

/// Every size it spends is a token, and the curator names none of them.
#[test]
fn the_caption_spends_tokens_and_the_curator_no_numbers() {
    if !library_ships() {
        return;
    }
    let sheet = read(SHEET);
    let body = rule(&sheet, VISIBLE);
    assert!(
        body.contains("font-size: var(--t-caption)"),
        "the caption's size is not the sheet's smallest type token: a number \
         here is a quantity outside the sheet (§ 2): {body}"
    );
    assert!(
        body.contains("var(--dock-gap)"),
        "the gap between caption and mark is a number of its own instead of \
         the one the mark's own furniture already spends: {body}"
    );
    assert!(
        body.contains("var(--dock-pad)"),
        "the caption's cap is not measured from the inset the mark itself \
         stands at, so the two drift apart on a television: {body}"
    );
    // § 2: the SHEET names quantities. The curator writes classes.
    let py = read(COMPOSE);
    let os = {
        let at = py.find("OS_TEMPLATE = (").expect("OS_TEMPLATE is gone");
        let end = py[at..].find("\n)").expect("OS_TEMPLATE is not one block");
        py[at..at + end].to_string()
    };
    assert!(
        os.contains("display-os-state display-detail"),
        "the line is not marked as detail-level text, so what it IS depends on \
         a rule reading its phase and on nothing it says about itself (§ 7): \
         {os}"
    );
    assert!(
        !os.contains("style="),
        "the curator writes a size onto the mark by hand: {os}"
    );
}

/// B-34 measures it in a browser, in the mode that needs no colony.
#[test]
fn the_browser_proof_measures_the_caption() {
    if !library_ships() {
        return;
    }
    // The driver lives under `workshop/`, and nothing from there travels into the
    // exported tree (R2c). The public clone has the template and not the proof, so
    // the absence is a SKIP and never a red test -- measured 2026-09-19, when this
    // read panicked in the public CI of 0.39.0 while every private gate was green.
    if !repo(DRIVER).is_file() {
        eprintln!("SKIP: {DRIVER} is not in this tree");
        return;
    }
    let driver = read(DRIVER);
    let at = driver
        .find("async function B34(ctx) {")
        .expect("B-34 is gone -- then the caption is only a file text (GH #722)");
    let end = driver[at..]
        .find("\nconst CHECKS")
        .unwrap_or(driver.len() - at);
    let b34 = &driver[at..at + end];
    assert!(
        b34.contains("data-phase") && b34.contains("error"),
        "B-34 never produces the phase it is about: a page with a microphone \
         refuses nothing and the proof would be green on nothing: {b34}"
    );
    assert!(
        b34.contains("getBoundingClientRect"),
        "B-34 reads no box: `visibility: visible` over a 1px clip is still \
         nothing a person sees: {b34}"
    );
    assert!(
        b34.contains("clipPath") || b34.contains("clip-path"),
        "B-34 does not read the clip, which is exactly what hid the line: {b34}"
    );
    assert!(
        b34.contains("safe") || b34.contains("innerWidth"),
        "B-34 does not measure that the caption stays on the screen: {b34}"
    );
    assert!(
        b34.contains("display-tile") && b34.contains("overlap"),
        "B-34 does not intersect the caption with the tiles -- the defect it \
         missed the first time was a caption lying on one (GH #722, F1): {b34}"
    );
    assert!(
        b34.contains("data-dock-open"),
        "B-34 measures with the dock as the exit ships it, and `phone` ships it \
         shut -- which is why the overlap went unseen: {b34}"
    );
    for exit in ["\"tv\"", "\"monitor\"", "\"phone\""] {
        assert!(
            b34.contains(exit),
            "B-34 does not build the exit {exit}: the three differ in viewport, \
             scale and the dock's default (§ 6.4, § 6.7)"
        );
    }
    assert!(
        b34.contains("\"\"") && b34.contains("gone"),
        "B-34 never puts the phase back, so nothing proves the caption goes \
         with it (OR-H2.1): {b34}"
    );
    assert!(
        driver.contains("\"B-34\": B34"),
        "B-34 is written and not dispatched"
    );
    let runs = {
        let at = driver.find("const RUNS = {").expect("RUNS is gone");
        let end = driver[at..].find("\n};").expect("RUNS is not one block");
        driver[at..at + end].to_string()
    };
    let sheet_line = {
        let at = runs.find("  sheet: [").expect("the sheet line is gone");
        runs[at..].to_string()
    };
    assert!(
        sheet_line.contains("\"B-34\""),
        "B-34 stands in no run, so no line ever drives it: {sheet_line}"
    );
}

/// The source and the runtime copy of it are the same bytes (three places, #669).
#[test]
fn the_sheet_reached_the_shipped_copies() {
    if !library_ships() {
        return;
    }
    let sheet = read(SHEET);
    let py = read(COMPOSE);
    assert!(
        py.contains(VISIBLE),
        "`compose.py` does not carry the sheet's new rule -- run \
         `scripts/display_sync.py`"
    );
    let cfg: serde_json::Value =
        serde_json::from_str(&read("templates/display/compose/config.json")).expect("config.json");
    assert_eq!(
        cfg["params"]["script_inline"].as_str().expect("inline"),
        py,
        "`config.json` ships another `compose.py` than the file beside it"
    );
    assert!(
        rule(&sheet, VISIBLE) == rule(&py, VISIBLE),
        "the rule in the sheet and the rule in the shipped script differ"
    );
}
