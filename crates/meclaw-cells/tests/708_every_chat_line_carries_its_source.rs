//! GH #708 -- every chat line carries its source AND its clock time (§ 8.4),
//! and the mark's dot and glow follow the profile (§ 6.8).
//!
//! "Every line carries its source and its clock time: at every turn and every
//! answer it is visible which channel it came from (glyph or word: voice,
//! telephone, Telegram, typed) and when the line arrived. With the clock time
//! of the line, beside its source" (§ 8.4, R-26-1, owner ruling 18.09.).
//!
//! The time was here once, then it was taken away, and now it is back with a
//! reason on both sides. It was taken away because the line said a time and NOT
//! which channel it came from, and the chat is the whole conversation across
//! every channel (§ 8.1) -- provenance was the missing word, not the clock. It
//! is back because a conversation that spans a day is unreadable without it:
//! "chat -- why is there no time on a turn any more?! that is silly" (owner,
//! human test 18.09.). Both now stand, side by side; the TILE keeps no time
//! (§ 7.2), because a tile is a face, not a transcript.
//!
//! The time is a detail beside the source (`display-detail`): the markup
//! carries `<time data-at>` with the epoch milliseconds of the line, and the
//! client writes HH:MM into it in the viewer's time zone (§ 8.4) -- the screen
//! state has no time zone and the device has one. A line without `at` renders
//! exactly as it did before: no node, no empty box.
//!
//! The mark, § 6.8: "The dot on it is visible as long as `unseen > 0` and the
//! dock on this output is actually closed, whether by profile or by press" --
//! and "its listening state -- the glow while a hold runs and the client
//! records audio -- exists only where a hold exists: with `audio` in the
//! profile (§ 6.4)". The glow was ungated: an output that can never hold had a
//! rule for the state it can never be in.
//!
//! The template half belongs to the curator strand (plan § 6, H1-T6 writes
//! `{{source}}` and `data-channel`); this file styles it and judges the
//! template as soon as `ATTRS` is in the tree.
//!
//! The browser proof is B-27 (every line names its channel and carries a visible
//! HH:MM out of `time[data-at]`, and the chat TILE carries none). Since R-26-1 it
//! rides the sheet line too: the sheet and the scene are both in the page a sheet
//! run builds, so the ruling is measurable without a colony.
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

fn uncommented(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
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

/// One selector list into its parts. Only a comma OUTSIDE parentheses
/// separates two selectors: `:not(html[data-dock-open="1"] *)` is one
/// condition, and splitting inside it would read half of it as a selector.
fn comma_parts(head: &str) -> Vec<String> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, 0usize);
    for (i, c) in head.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(head[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(head[start..].trim().to_string());
    out
}

fn rules(src: &str) -> Vec<(String, String)> {
    let plain = uncommented(src);
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

fn block(name: &str) -> String {
    let src = read(COMPOSE);
    let open = format!("{name} = (");
    let start = src.find(&open).unwrap_or_else(|| panic!("{name} is gone"));
    let end = src[start..]
        .find("\n)")
        .unwrap_or_else(|| panic!("{name} is not one parenthesised block"));
    src[start..start + end].to_string()
}

#[test]
fn the_sheet_draws_a_source_and_the_time_beside_it() {
    if !library_ships() {
        return;
    }
    let sheet = read(SHEET);
    assert!(
        rules(&sheet)
            .iter()
            .any(|(sel, _)| sel.contains(".display-chat-line-source")),
        "nothing styles the source of a line: § 8.4 asks that it be VISIBLE \
         which channel every turn and every answer came from"
    );
    assert!(
        rules(&sheet)
            .iter()
            .any(|(sel, _)| sel.contains(".display-chat-line-time")),
        "nothing styles the time of a line: § 8.4 (R-26-1) asks that every turn \
         and every answer say WHEN it arrived, beside its source"
    );
    // Beside, not under: source and time share one row, and on a phone that row
    // does not break (§ 8.4 -- "a detail beside the source", one space apart).
    let meta: Vec<(String, String)> = rules(&sheet)
        .into_iter()
        .filter(|(sel, _)| sel.contains(".display-chat-line-meta"))
        .collect();
    assert!(
        meta.iter().any(|(_, body)| body.contains("flex")),
        "the source and the time do not share a row: § 8.4 puts the time BESIDE \
         the source, not under it"
    );
    assert!(
        meta.iter()
            .any(|(_, body)| body.contains("nowrap") || body.contains("no-wrap")),
        "the row of source and time may break on a narrow exit: § 8.4 keeps them \
         together"
    );
}

#[test]
fn the_dot_needs_a_closed_dock_and_something_unseen() {
    if !library_ships() {
        return;
    }
    let sheet = read(SHEET);
    let dot: Vec<(String, String)> = rules(&sheet)
        .into_iter()
        .filter(|(sel, _)| sel.contains(".display-os") && sel.contains("::after"))
        .collect();
    assert!(!dot.is_empty(), "the mark has no dot at all (§ 6.8)");
    let lit: Vec<String> = dot
        .iter()
        .filter(|(_, decls)| decls.contains("content: \"\""))
        .flat_map(|(sel, _)| comma_parts(sel))
        .collect();
    assert!(!lit.is_empty(), "no rule lights the dot");
    for part in &lit {
        assert!(
            part.contains("[data-unseen]") && part.contains(":not([data-unseen=\"0\"])"),
            "`{part}` lights the dot without asking for `unseen > 0` (§ 6.8, step 10)"
        );
    }
    // The four states of one output, and the sheet has to answer all four
    // (§ 6.8: "as long as `unseen > 0` AND the dock on this output is actually
    // closed, whether by profile or by press"). Closed has two shapes, so two
    // selectors, and both ask <html> POSITIVELY: no attribute at all means
    // nothing was pressed and the profile decides; `"0"` means a finger closed
    // it, whatever the profile ships. Written that way on purpose -- a complex
    // `:not(html[…] *)` needs Selectors 4, and an engine that cannot parse it
    // drops the whole rule, which here is the dot on every output at once.
    let by_profile = lit
        .iter()
        .any(|p| p.contains("html:not([data-dock-open])") && p.contains("[data-dock=\"hidden\"]"));
    assert!(
        by_profile,
        "the dot is dark where the PROFILE ships the dock closed and no finger \
         opened it: {lit:?}"
    );
    let by_press = lit
        .iter()
        .any(|p| p.contains("html[data-dock-open=\"0\"] .display-columns"));
    assert!(
        by_press,
        "the dot is dark where the profile ships the dock OPEN and a finger \
         closed it -- the fourth case, and the one the two switch-off rules of \
         2.4.0 could not see: they read `[data-dock=\"shown\"]` and stopped, \
         while `tapDock()` writes `html[data-dock-open=\"0\"]` and leaves the \
         root's own word alone. {lit:?}"
    );
    for part in &lit {
        assert!(
            !part.contains("] *)"),
            "`{part}` asks a complex `:not()` (Selectors 4). An engine that \
             cannot parse it drops the rule -- and this rule IS the dot."
        );
    }
    // And nothing takes it back afterwards: a rule that lit the dot and two
    // that unlit it is how the fourth case went missing.
    let off: Vec<String> = dot
        .iter()
        .filter(|(_, decls)| decls.contains("content: none"))
        .map(|(sel, _)| sel.clone())
        .collect();
    assert!(
        off.is_empty(),
        "the dot is lit and then switched off again ({off:?}) -- § 6.8 is one \
         condition, and it is easier to read as one rule than as a rule and \
         its exceptions"
    );
}

#[test]
fn the_glow_exists_only_where_a_hold_does() {
    if !library_ships() {
        return;
    }
    let sheet = read(SHEET);
    for (selector, _) in rules(&sheet) {
        if !selector.contains("[data-phase=\"listening\"]") {
            continue;
        }
        assert!(
            selector.contains("[data-inputs~=\"audio\"]"),
            "`{selector}` draws the listening state on an output that cannot \
             hold -- § 6.8: the glow exists only where a hold exists, which is \
             `audio` in the profile (§ 6.4)"
        );
    }
}

#[test]
fn a_line_says_which_channel_it_came_from() {
    if !library_ships() {
        return;
    }
    let compose = read(COMPOSE);
    // The curator strand writes the line (plan § 6, H1-T6); until its `ATTRS`
    // is in this tree there is nothing here to judge.
    if !compose.contains("ATTRS = {") {
        eprintln!("skip: the curator's ATTRS is not in this tree yet (H1)");
        return;
    }
    let line = block("CHAT_LINE_TEMPLATE");
    assert!(
        line.contains("data-channel=\"{{channel}}\""),
        "the line does not carry its channel (§ 8.4)"
    );
    assert!(
        line.contains("display-chat-line-source"),
        "the line has no source element for the sheet to draw (§ 8.4)"
    );
    assert!(
        !line.contains("{{time}}"),
        "the line carries a rendered time string: since R-26-1 the markup carries \
         the raw `at` and the CLIENT formats it for the viewer's time zone (§ 8.4)"
    );
    // R-26-1: the time is there, it is raw, and it is conditional. `{{#if at}}`
    // is what keeps a line that has no time from drawing an empty box.
    assert!(
        line.contains("display-chat-line-time") && line.contains("data-at=\"{{at}}\""),
        "the line has no time element carrying `at` (§ 8.4, § 8.5, R-26-1)"
    );
    assert!(
        line.contains("{{#if at}}"),
        "the time is unconditional: a line without `at` would draw an empty box \
         (§ 8.4, R-26-1)"
    );
    // The catalogue has to KNOW the prop, or the renderer drops it before the
    // template ever sees it.
    let start = compose
        .find("_c(\"display-chat-line\"")
        .expect("`display-chat-line` is not in the catalogue");
    let rest = &compose[start..];
    let end = rest.find("}),").map(|i| i + 3).unwrap_or(rest.len());
    let entry = &rest[..end];
    assert!(
        entry.contains("\"at\": \"int\""),
        "`display-chat-line` does not declare `at` as an int prop: § 3.3 -- time \
         points are epoch milliseconds (R-26-1)"
    );
}
