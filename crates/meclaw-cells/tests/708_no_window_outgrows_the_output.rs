//! GH #708 -- every OPEN window has the cap and scrolls inside (§ 5.9), and
//! the blur follows the level (§ 6.9).
//!
//! This file replaces `gh703_a_window_never_outgrows_the_screen.rs` as the text
//! lock of § 5.9 and carries its four sentences on. That one capped exactly one
//! window -- the modal, `[data-plane="2"]` -- because until now only the modal
//! stood over the canvas. § 4.24 knows three OPEN levels (1 canvas, 2 modal,
//! 3 the front urgent) and § 5.9 speaks about all of them: "an open window is
//! never taller than the output's visible height minus the safe area; its
//! content scrolls inside; its input line stays at its foot". A canvas window
//! without a cap is cut off, because `body` is `overflow: hidden` -- the page
//! itself never scrolls (§ 5.9, measured on e24: the field stood
//! `offscreen(+528px)` at 393x852 before the modal got its cap).
//!
//! Every assertion names the RULE it wants, never the sheet as one string: the
//! first draft of the file this one replaces joined the rules and searched the
//! join for two words, and a rule on `.display-pane-kicker` carrying them made
//! it green while nothing that matters could shrink or scroll. A declaration
//! means nothing without the element it stands on.
//!
//! The browser proofs are B-08 (the page never scrolls) and B-09 (every open
//! window fits, scrolls inside, and keeps its field reachable), strand H5.
//!
//! Skips when the templates do not ship (R2b).

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const SHEET: &str = "templates/display/compose/display-dna.css";

/// The three open levels, as one condition (§ 4.24).
const OPEN: &str = ":is([data-level=\"1\"], [data-level=\"2\"], [data-level=\"3\"])";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

fn sheet() -> String {
    std::fs::read_to_string(repo(SHEET)).expect("the sheet ships")
}

/// The sheet without its comments -- a `{` inside one would split a rule in
/// the wrong place, and a word inside one would answer a question about code.
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

/// Every rule of the sheet as `(selector list, declarations)`.
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

/// The ONE rule whose selector list carries `needle`, selector and all.
fn only_rule(sheet: &str, needle: &str) -> (String, String) {
    let hits: Vec<(String, String)> = rules(sheet)
        .into_iter()
        .filter(|(sel, _)| sel.contains(needle))
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "exactly one rule is expected to name `{needle}`, found {}: {:?}",
        hits.len(),
        hits.iter().map(|(s, _)| s).collect::<Vec<_>>()
    );
    hits.into_iter().next().unwrap()
}

fn rule_named(sheet: &str, selector: &str) -> String {
    rules(sheet)
        .into_iter()
        .find(|(sel, _)| sel == selector)
        .unwrap_or_else(|| panic!("the sheet has no rule `{selector}`"))
        .1
}

#[test]
fn every_open_window_is_capped_against_the_visible_height() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    let cap = rule_named(&sheet, &format!(".display-columns > [data-region] {OPEN}"));
    assert!(
        cap.contains("max-block-size:"),
        "no cap on the open windows -- a window taller than the output is cut \
         off, because the page itself never scrolls (§ 5.9). Found: {cap}"
    );
    assert!(
        cap.contains("100dvh"),
        "the cap measures the VISIBLE height: `100vh` on a phone is the height \
         without the address bar, so a window laid out against it ends under a \
         toolbar (§ 6.7, R-23-10). Found: {cap}"
    );
    assert!(
        cap.contains("100vh"),
        "and it keeps a `vh` fallback line for an engine that drops the `dvh` \
         one -- two declarations, no `@supports`, like `body`"
    );
    assert!(
        cap.contains("env(safe-area-inset-bottom"),
        "the safe area comes off the cap: the home bar is part of the screen \
         and not of the room a window may take (§ 5.9, § 6.7)"
    );
    assert!(
        cap.contains("box-sizing: border-box"),
        "the cap counts the whole box: `content-box` is the default, so the \
         window's own padding stood on top of the cap and the window outgrew \
         the output by it (B-09 on a television, F-3). Found: {cap}"
    );
    assert!(
        cap.contains("/ var(--rung-scale)"),
        "and it divides by the lift the same window carries: a scaled box \
         RENDERS taller than it is laid out, so focus and urgent pushed the \
         capped height back over the output (B-09, F-3). Found: {cap}"
    );
}

/// § 5.9 with § 7: the two loud rungs spend a token the cap can read back.
#[test]
fn the_lift_of_a_loud_rung_is_a_token_the_cap_can_divide_by() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    let root = rule_named(&sheet, ":root");
    for token in ["--rung-lift-scale: 1.02", "--rung-scale: 1"] {
        assert!(
            root.contains(token),
            "the lift is a quantity and quantities stand in `:root` (§ 2 \
             `Token`) -- `{token}` is missing"
        );
    }
    for rung in ["focus", "urgent"] {
        let lift = rule_named(
            &sheet,
            &format!(
                ":where(.display-pane, .display-panel, .display-overlay)[data-rung=\"{rung}\"]"
            ),
        );
        assert!(
            lift.contains("--rung-scale: var(--rung-lift-scale)"),
            "rung `{rung}` lifts its window, so it says so on the element the \
             cap is computed on -- otherwise the cap divides by 1 and the \
             window outgrows the output again (§ 5.9). Declarations: {lift}"
        );
        assert!(
            lift.contains("scale(var(--rung-scale))"),
            "and it scales by that same value rather than by a number of its \
             own (§ 2 `Token`). Declarations: {lift}"
        );
    }
}

#[test]
fn the_column_inside_every_open_window_may_shrink() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    let (selector, shrink) = only_rule(
        &sheet,
        ":is(.inner, .display-pane-body, .display-panel-body)",
    );
    assert!(
        selector.contains(OPEN),
        "the rule that lets the column shrink reaches every open level, not \
         one of them: found on `{selector}`"
    );
    assert!(
        shrink.contains("min-block-size: 0"),
        "a flex item's floor is its content unless it is told otherwise, so \
         without this the cap is a cap on nothing -- the box would say one \
         height and the column inside keep another. Declarations: {shrink}"
    );
}

#[test]
fn what_does_not_fit_scrolls_inside_every_open_window() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    // The scroller is named by what it EXCLUDES: everything in the body of an
    // open window except the input line.
    let (selector, scroller) = only_rule(&sheet, "> :not(.display-input)");
    assert!(
        selector.contains(OPEN),
        "the scroller reaches every open level: found on `{selector}`"
    );
    for decl in [
        "min-block-size: 0",
        "overflow-y: auto",
        "overscroll-behavior: contain",
    ] {
        assert!(
            scroller.contains(decl),
            "what does not fit scrolls INSIDE the window rather than off the \
             screen, and the gesture stays in the window -- `{decl}` is \
             missing. Declarations: {scroller}"
        );
    }
}

#[test]
fn the_input_line_stays_at_the_foot_of_the_window() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    let input = rule_named(&sheet, ".display-input");
    assert!(
        input.contains("flex: 0 0 auto"),
        "the input line neither grows nor shrinks: it is the foot of the \
         window (§ 5.9), and a conversation that could squeeze it would take \
         it off the screen one line at a time. Found: {input}"
    );
    // OR-F49 is not a casualty of this change: § 6.7 asks for the same floor.
    let field = rule_named(&sheet, ".display-input-field");
    assert!(
        field.contains("font-size: max(16px, var(--t-body))"),
        "the field keeps its 16px floor (§ 6.7, OR-F49): a mobile engine zooms \
         the whole page when a smaller field takes focus, and never zooms \
         back. Found: {field}"
    );
}

#[test]
fn the_blur_follows_the_level_and_the_os_stays_sharp() {
    if !library_ships() {
        return;
    }
    let sheet = sheet();
    // § 6.9: level 2 takes level 1 back a little, level 3 takes 1 and 2 back
    // further, and nothing else blurs.
    let modal = rule_named(
        &sheet,
        ".display-columns:has([data-level=\"2\"]) [data-level=\"1\"]",
    );
    assert!(
        modal.contains("var(--f-back-2)") && sheet.contains("--f-back-2: blur(4px);"),
        "the modal's grade moved: {modal}"
    );
    let urgent = rule_named(
        &sheet,
        ".display-columns:has([data-level=\"3\"]) :is([data-level=\"1\"], [data-level=\"2\"])",
    );
    assert!(
        urgent.contains("var(--f-back-3)") && sheet.contains("--f-back-3: blur(10px);"),
        "the urgent's grade moved: {urgent}"
    );
    for (selector, decls) in rules(&sheet) {
        if !decls.contains("blur(") || !selector.contains("data-level") {
            continue;
        }
        for furniture in [".display-dock", ".display-os"] {
            assert!(
                !selector.contains(furniture),
                "`{selector}` blurs the OS level -- § 6.9: dock and mark lie \
                 above everything and react to no change of focus, modal or \
                 urgent"
            );
        }
    }
}
