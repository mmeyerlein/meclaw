//! GH #708 -- the canvas is laid out by the type of output (§ 6.3), and it is
//! the canvas that scrolls, never the page.
//!
//! "Arrangement per type (deterministic, tokens): **monitor** canvas windows
//! side by side, up to three in a row, further ones below; **tv** up to two
//! side by side …; **phone** canvas windows stacked, the leading one on top and
//! larger; on every type, when the open windows do not fit on the output, the
//! canvas scrolls, never the page (§ 5.9), and no window is missing" (§ 6.3).
//!
//! Until this wave there was no arrangement at all: every output stood the open
//! windows in one flex column, and "does not fit" was answered by showing FEWER
//! windows (`canvas_slots`) -- struck by § 2 and § 6.2. The state is one; what
//! an output does with it is rendering, and rendering is this sheet.
//!
//! The browser proofs are B-11 (three across on a monitor, two on a tv, the
//! leading one on top and larger on a phone) and B-12 (the canvas scrolls, the
//! page does not, and no window is missing), strand H5.
//!
//! Skips when the templates do not ship (R2b).

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const SHEET: &str = "templates/display/compose/display-dna.css";
const COMPOSE: &str = "templates/display/compose/compose.py";
const SECTION: &str = "§ 6.3 arrangement per type";
/// The canvas region: `main`, not "every region". `aside` is a word older
/// senders may say and is drawn as canvas (OR-D3), but § 6.3 knows ONE canvas
/// -- two grids with two scrollers would be two.
const CANVAS: &str = ".display-columns > [data-region=\"main\"]";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

fn sheet() -> String {
    std::fs::read_to_string(repo(SHEET)).expect("the sheet ships")
}

/// The screen's own layout rules, which travel in the shell's `<style>` ahead
/// of the sheet. The BOX lives there; the canvas inside it lives in the sheet.
fn layout_rules() -> String {
    let src = std::fs::read_to_string(repo(COMPOSE)).expect("compose.py");
    src.split("LAYOUT_RULES = (")
        .nth(1)
        .expect("compose.py carries LAYOUT_RULES")
        .split("\n)")
        .next()
        .expect("and it is one parenthesised block")
        .to_string()
}

fn uncommented(sheet: &str) -> String {
    let mut out = String::with_capacity(sheet.len());
    let mut rest = sheet;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        match rest[start..].find("*/") {
            Some(end) => rest = &rest[start + end + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

fn rules(sheet: &str) -> Vec<(String, String)> {
    let plain = uncommented(sheet);
    let mut out = Vec::new();
    let mut rest = plain.as_str();
    while let Some(open) = rest.find('{') {
        let (head, tail) = rest.split_at(open);
        let tail = &tail[1..];
        let Some(close) = tail.find('}') else { break };
        let (decls, after) = tail.split_at(close);
        out.push((
            head.rsplit('}').next().unwrap_or(head).trim().to_string(),
            decls.to_string(),
        ));
        rest = &after[1..];
    }
    out
}

fn rule_named(sheet: &str, selector: &str) -> String {
    rules(sheet)
        .into_iter()
        .find(|(sel, _)| sel == selector)
        .unwrap_or_else(|| panic!("the sheet has no rule `{selector}`"))
        .1
}

#[test]
fn the_sheet_says_where_the_arrangement_lives() {
    if !library_ships() {
        return;
    }
    assert!(
        sheet().contains(SECTION),
        "{SHEET} has no section `{SECTION}` -- a rendering sentence of the \
         description is a place in this sheet, not a habit"
    );
}

#[test]
fn the_canvas_region_is_a_grid_that_scrolls_inside() {
    if !library_ships() {
        return;
    }
    let region = rule_named(&sheet(), CANVAS);
    for decl in [
        "display: grid",
        "min-block-size: 0",
        "overflow-y: auto",
        "overscroll-behavior: contain",
    ] {
        assert!(
            region.contains(decl),
            "the canvas region is missing `{decl}` -- § 6.3: what does not fit \
             scrolls INSIDE the canvas, and § 5.9: never the page. Found: {region}"
        );
    }
    // And `aside` is not a second canvas: its box generates nothing, so its
    // windows stand in the column rather than in a grid of their own.
    assert!(
        rule_named(&sheet(), ".display-columns > [data-region=\"aside\"]")
            .contains("display: contents"),
        "`aside` draws a second grid with a second scroller -- § 6.3 knows one \
         canvas (OR-D3: aside is drawn AS canvas)"
    );
    // ... and because that makes an aside WINDOW a flex item of the column,
    // whose floor is its own content, it has to be allowed to shrink: one long
    // aside window would otherwise take the room the canvas needs and leave
    // `flex: 1 1 auto` nothing to take.
    assert!(
        rule_named(
            &sheet(),
            ".display-columns > [data-region=\"aside\"] \
             :where(.display-pane, .display-panel, .display-overlay)"
        )
        .contains("min-block-size: 0"),
        "an `aside` window can press the canvas to nothing"
    );
}

/// The half without which `overflow-y: auto` is a word and nothing else: a box
/// scrolls only when its height is BOUNDED. `body` is as tall as the output and
/// `overflow: hidden` (§ 5.9), the column under it takes that height, and the
/// canvas takes what the column has left. Break any link and the canvas grows
/// past the screen and is cut off -- a window missing (§ 6.2) instead of a
/// canvas that scrolls (§ 6.3). Measured in the browser by B-30 (H5).
#[test]
fn the_height_chain_reaches_the_canvas() {
    if !library_ships() {
        return;
    }
    let body = rule_named(&sheet(), "body");
    assert!(
        body.contains("height: 100dvh") && body.contains("overflow: hidden"),
        "the page is not bounded any more, so nothing under it can be: {body}"
    );
    let columns = layout_rules();
    for decl in [
        "block-size: 100vh",
        "block-size: 100dvh",
        "box-sizing: border-box",
    ] {
        assert!(
            columns.contains(decl),
            "the column is missing `{decl}`: it hands the page's bound on to the \
             canvas, and with `content-box` the dock's gutter would make it \
             taller than the screen. Found: {columns}"
        );
    }
    assert!(
        columns.contains("align-items: stretch"),
        "the column centres its children again -- a shrink-to-fit canvas divides \
         a width nobody set, and three equal tracks are then three thirds of \
         the widest window (§ 6.3). Found: {columns}"
    );
    assert!(
        !columns.contains("[data-region] { display: contents; }"),
        "the layout rules still say `display: contents` for every region while \
         the sheet says `display: grid` for the canvas -- same weight, and only \
         the order inside one `<style>` decides (development-rules § 2d)"
    );
    let canvas = rule_named(&sheet(), CANVAS);
    assert!(
        canvas.contains("flex: 1 1 auto") && canvas.contains("min-block-size: 0"),
        "the canvas neither takes the room the column has left nor may be shorter \
         than its content -- the two halves of a scroller: {canvas}"
    );
}

/// § 6.3: "a modal lies over the canvas; an urgent over everything". Both stand
/// out of flow, and both are reached by DESCENT: an application's window hangs
/// in a wrapper that generates no box, so a child selector would leave an app's
/// urgent lying in the grid while a prose view stood over everything.
#[test]
fn the_two_over_levels_stand_over_the_canvas() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    let over = rule_named(
        &sheet,
        ".display-columns > [data-region] :is([data-level=\"2\"], [data-level=\"3\"])",
    );
    for decl in [
        "position: fixed",
        "inset-block-start: 50%",
        "translate: -50% -50%",
    ] {
        assert!(
            over.contains(decl),
            "a window over the canvas is missing `{decl}`: {over}"
        );
    }
    for (selector, _) in rules(&sheet) {
        assert!(
            !selector.contains("> [data-level="),
            "`{selector}` reaches a window by the CHILD combinator -- a window \
             inside a wrapper (§ 11, `display: contents`) is a grid item without \
             being a child, so such a rule holds for prose views only"
        );
    }
}

#[test]
fn the_page_itself_still_never_scrolls() {
    if !library_ships() {
        return;
    }
    let body = rule_named(&sheet(), "body");
    assert!(
        body.contains("overflow: hidden"),
        "`body` has to stay `overflow: hidden` (§ 5.9): the canvas took over \
         the scrolling, and a page that scrolls as well is the rubber band \
         #705/2 was about. Found: {body}"
    );
}

#[test]
fn each_type_arranges_the_canvas_its_own_way() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    // The count is the sentence of § 6.3 and it has not moved; what the tracks
    // SAY has. `1fr` is the correct spelling and WebKit resolves it against the
    // content box without taking the gaps out first, so three tracks and two
    // gaps came to 40px more than the box that holds them and the canvas could
    // be dragged sideways over an empty screen (measured against the twin,
    // 18.09.: monitor 1868 against 1828, television 3696 against 3676; Chromium
    // right on both). The subtraction stands in the sheet now, and both engines
    // lay out the same pixels. A phone has one track and no gap, so it keeps
    // `1fr` and is the control in this very list.
    for (exit, columns, why) in [
        (
            "monitor",
            "repeat(3, minmax(0, calc((100% - 2 * var(--gap)) / 3)))",
            "up to three in a row",
        ),
        (
            "tv",
            "repeat(2, minmax(0, calc((100% - var(--gap)) / 2)))",
            "up to two side by side",
        ),
        ("phone", "minmax(0, 1fr)", "stacked, one column"),
    ] {
        let rule = rule_named(
            &sheet,
            &format!(".display-columns[data-exit=\"{exit}\"] > [data-region=\"main\"]"),
        );
        assert!(
            rule.contains(&format!("grid-template-columns: {columns}")),
            "a {exit} arranges the canvas {why} (§ 6.3): expected \
             `grid-template-columns: {columns}`, found: {rule}"
        );
    }
}

#[test]
fn the_leading_window_on_a_phone_is_first_and_larger() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    let lead = rule_named(
        &sheet,
        ".display-columns[data-exit=\"phone\"] > [data-region=\"main\"] \
         [data-level=\"1\"][data-rung=\"focus\"]",
    );
    assert!(
        lead.contains("order: -1"),
        "the leading window stands on top on a phone (§ 6.3). Found: {lead}"
    );
    assert!(
        lead.contains("min-block-size: var(--lead-min-phone)"),
        "and it is larger than its siblings -- as a token, never as a pixel in \
         a rule (§ 2 `Token`). Found: {lead}"
    );
    // A minimum under the leading window says nothing about the others: a
    // conversation with forty lines grew to the cap of § 5.9 and stood taller
    // than the window it was meant to stand behind (B-11, F-1). So the
    // siblings carry a cap of their own, under that minimum.
    let siblings = rule_named(
        &sheet,
        ".display-columns[data-exit=\"phone\"] > [data-region=\"main\"] \
         [data-level=\"1\"]:not([data-rung=\"focus\"])",
    );
    assert!(
        siblings.contains("max-block-size: var(--sibling-max-phone)"),
        "a window that is not leading keeps UNDER the leading one on a phone \
         (§ 6.3); what does not fit scrolls inside it (§ 5.9). Found: {siblings}"
    );
    // The two quantities only mean "larger" as long as one is under the other.
    let plain = uncommented(&sheet);
    let read = |token: &str| -> f64 {
        let at = plain
            .rfind(token)
            .unwrap_or_else(|| panic!("the token `{token}` is not declared"));
        let value = plain[at + token.len()..]
            .split(';')
            .next()
            .expect("a declaration ends")
            .trim()
            .trim_end_matches("dvh")
            .trim_end_matches("vh")
            .to_string();
        value
            .parse()
            .unwrap_or_else(|_| panic!("`{token}` is not a length: {value}"))
    };
    let (lead_min, sibling_max) = (read("--lead-min-phone:"), read("--sibling-max-phone:"));
    assert!(
        sibling_max < lead_min,
        "the siblings' cap ({sibling_max}) has to stand under the leading \
         window's floor ({lead_min}), or the leading window is not the largest \
         (§ 6.3, B-11)"
    );
    // #705/4: a modal on a phone is not the whole screen either. § 5.9 allows
    // the full height, § 6.3 asks for a size relative to the others.
    let modal = rule_named(
        &sheet,
        ".display-columns[data-exit=\"phone\"] > [data-region=\"main\"] [data-level=\"2\"]",
    );
    assert!(
        modal.contains("max-block-size: var(--modal-max-phone)"),
        "the modal on a phone keeps a size relative to the others (§ 6.3, \
         acceptance #705/4). Found: {modal}"
    );
    for token in [
        "--lead-min-phone:",
        "--sibling-max-phone:",
        "--modal-max-phone:",
    ] {
        assert!(
            uncommented(&sheet).contains(token),
            "the token `{token}` is not declared -- a quantity lives in the \
             sheet under a name (§ 2)"
        );
    }
}

/// § 6.6: "The sheet scales only through tokens; no width query overrides a
/// profile." Not one query, every query: a profiled screen takes its air from
/// `--scale`, and a far television is not a small monitor. The gallery page has
/// no shell and therefore no profile -- that is what the queries that remain
/// are for, and they say so in their own selector.
#[test]
fn no_width_query_reaches_a_profiled_screen() {
    if !library_ships() {
        return;
    }
    let plain = uncommented(&sheet());
    assert!(
        !plain.contains("@media (max-width: 60rem)"),
        "the 60rem query is back -- it laid out the canvas by width (§ 6.3/§ 6.6)"
    );
    let mut rest = plain.as_str();
    let mut seen = 0;
    while let Some(at) = rest.find("@media (max-width:") {
        let body = &rest[at..];
        let end = body.find("\n}\n").map(|i| i + 3).unwrap_or(body.len());
        seen += 1;
        for (selector, _) in rules(&body[..end]) {
            assert!(
                !selector.contains(".display-columns")
                    || selector.contains(":not(.display-columns *)"),
                "`{selector}` stands in a width query -- § 6.6: no width query \
                 overrides a profile. Either it belongs to the gallery, which \
                 has no shell, or it does not belong in a query at all."
            );
        }
        rest = &rest[at + "@media (max-width:".len()..];
    }
    assert!(
        seen > 0,
        "no width query at all -- the gallery lost its air"
    );
}
