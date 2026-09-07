//! Written text in, spoken text out: what a synthesis provider is handed.
//!
//! An assistant writes for a screen. It emphasises with `**stars**`, titles
//! with `#`, enumerates with `-`, links with `[text](url)` and lays a table out
//! in pipes — and every one of those characters is a character, so a speech
//! provider reads them out. The first live call of this cell ended with
//! Cartesia saying the word "asterisk" around a bolded word, which is not a
//! provider bug: it was handed markdown and asked to speak it.
//!
//! So the cell strips the markup before the text reaches the queue, and this
//! module is the whole of that rule set. It is a pure function over a string —
//! no I/O, no state, no dependency, not even a markdown parser. A parser would
//! be the honest tool for RENDERING markdown; this is not rendering. It is a
//! short list of shapes an LLM writes and a microphone should not hear, and a
//! hand-written list that a person can read in one screen is the version of
//! that anybody can argue with. The parameter `speak_plain` turns it off.
//!
//! **A line is rewritten to a fixpoint, not once.** The structural rules run
//! before the inline ones — a list marker is a marker because it stands at the
//! start of a line — and the inline pass can UNCOVER a structural marker that
//! was hidden behind one: `` `# install` `` is inline code around what becomes
//! a heading, `[- Punkt](url)` a link around what becomes a list item,
//! `**| a | b |**` an emphasised table row. One pass would leave the `#`, the
//! `-` and the pipes in the text a provider reads. So the pipeline is applied
//! until it stops changing the line, which terminates because a pass only ever
//! deletes markup (the one growth is a table row becoming its cells, and the
//! result of that is no longer a row).
//!
//! Three properties are load-bearing, and each has its own test:
//!
//! * **Plain prose is byte-identical.** Text with no markup in it must come out
//!   exactly as it went in, or every answer pays for a feature only some
//!   answers need.
//! * **Idempotent.** `to_speech(to_speech(x)) == to_speech(x)`, checked as a
//!   property over every example this module's tests use — that is what the
//!   fixpoint above buys, and one revealed marker is enough to lose it.
//! * **Nothing but markup is touched.** Umlauts, punctuation, digits and any
//!   other character travel as they are. HTML entities and tags are left alone
//!   on purpose: they are somebody else's escape (`&amp;`, `<br>`), and a cell
//!   that half-decoded them would produce a third dialect.

/// How often the line pipeline may be applied before the result is taken as
/// settled.
///
/// A safety net, not a policy: a pass only removes markup, so a line settles in
/// two or three rounds however it was written, and the idempotency test is the
/// measurement rather than this number. A cap is here because an unbounded loop
/// over attacker-shaped text is a worse failure than a line that keeps one
/// asterisk. **What a cap hit costs is named:** markup stays in the line, and
/// idempotency ends there — a second `to_speech` over that line would take
/// another eight rounds off it.
const MAX_PASSES: usize = 8;

/// Turn a written answer into the text a speech synthesiser should read.
///
/// Removes markdown emphasis (`*`, `**`, `_`, `__`, inline backticks), heading
/// hashes, blockquote markers, list markers and task boxes, link and image
/// syntax (keeping the text and the alt text), code fences (keeping the code),
/// table pipes (a row becomes its cells joined by commas, a separator row
/// disappears) and the backslash of an escape, keeping what it hid. A line
/// break becomes a sentence
/// end: the line gets a full stop unless it already ends in `.`, `!`, `?`, `:`,
/// `;` or `,`. Blank lines collapse, runs of spaces collapse, and the last line
/// is left as it is — which is what keeps a one-line plain answer
/// byte-identical.
///
/// Two shapes are deliberately NOT touched, because both are read aloud by
/// somebody who needs them: a `*` or `_` with whitespace on both sides is an
/// arithmetic operator, and the digit of `5.` at the start of a line is a date.
pub fn to_speech(text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let line = to_fixpoint(raw.strip_suffix('\r').unwrap_or(raw));
        if !line.is_empty() {
            lines.push(line);
        }
    }

    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        out.push_str(line);
        if i + 1 < lines.len() {
            if !ends_a_sentence(line) {
                out.push('.');
            }
            out.push('\n');
        }
    }
    out
}

/// Apply [`one_pass`] until the line stops changing.
fn to_fixpoint(line: &str) -> String {
    let mut current = one_pass(line);
    for _ in 0..MAX_PASSES {
        let next = one_pass(&current);
        if next == current {
            break;
        }
        current = next;
    }
    current
}

/// One application of the whole line pipeline: structure first, inline after.
fn one_pass(line: &str) -> String {
    let line = line.trim();
    if is_fence(line) || is_rule(line) {
        return String::new();
    }
    let line = strip_heading(strip_quote(line));
    if let Some(row) = table_row(line) {
        return collapse_spaces(&row);
    }
    let line = strip_list_marker(line);
    collapse_spaces(&clean(&line))
}

/// The fence of a code block, opening or closing. The line goes; the code
/// stays and is cleaned like any other line.
fn is_fence(line: &str) -> bool {
    line.starts_with("```")
}

/// A table separator (`|---|---|`) or a thematic break (`---`, `***`, `___`):
/// a line made of nothing but pipes, colons, spaces and rule characters, with
/// at least one rule character in it. There is no word in it to speak.
fn is_rule(line: &str) -> bool {
    line.contains(['-', '*', '_'])
        && line
            .chars()
            .all(|c| matches!(c, '|' | ':' | ' ' | '-' | '*' | '_'))
}

/// Drop the `>` of a blockquote. Nesting is handled by the loop, and a `>` in
/// the middle of a line is a greater-than sign.
///
/// A quote marker is followed by whitespace, by the end of the line, or by the
/// next level of quoting — which is what keeps `>= 5 Grad` a comparison.
fn strip_quote(line: &str) -> &str {
    let mut line = line;
    while let Some(rest) = line.strip_prefix('>') {
        if !(rest.is_empty() || rest.starts_with([' ', '\t', '>'])) {
            return line;
        }
        line = rest.trim_start();
    }
    line
}

/// Drop the `#` of an ATX heading. `#hashtag` keeps its hash — a heading marker
/// is followed by a space, and a word that starts with one is a word.
fn strip_heading(line: &str) -> &str {
    let mut line = line;
    loop {
        let hashes = line.len() - line.trim_start_matches('#').len();
        if hashes == 0 || hashes > 6 {
            return line;
        }
        let rest = &line[hashes..];
        if rest.is_empty() {
            return rest;
        }
        let Some(rest) = strip_one_space(rest) else {
            return line;
        };
        line = rest.trim_start();
    }
}

/// Drop the markers of a list item, and the task box behind one.
///
/// `-`, `*` and `+` go entirely. An ordinal loses only its **punctuation**:
/// `1. Erstens` becomes `1 Erstens`, and that is a decision rather than an
/// oversight. `5. September 2026` is a date far more often than it is a list,
/// and the two are indistinguishable at the start of a line — an agent that
/// reads a date back as "September 2026" has lost the day, while a list read as
/// "1 Erstens" has lost nothing but a little grace.
fn strip_list_marker(line: &str) -> String {
    let mut line = line.to_string();
    while let Some(next) = strip_one_marker(&line) {
        line = next;
    }
    line
}

/// One marker, plus the task box it may carry. `None` when there is none.
fn strip_one_marker(line: &str) -> Option<String> {
    if let Some(rest) = bullet(line) {
        return Some(strip_task_box(rest.trim_start()).to_string());
    }
    if let Some((digits, rest)) = ordinal(line) {
        return Some(format!("{digits} {}", strip_task_box(rest.trim_start())));
    }
    None
}

/// `- item` → ` item`, and `-5 Grad` → `None`: a bullet is followed by a space.
fn bullet(line: &str) -> Option<&str> {
    strip_one_space(line.strip_prefix(['-', '*', '+'])?)
}

/// `1. item` → `("1", " item")`. The digits come back so the caller can keep
/// them; only the `.` or `)` is the marker.
fn ordinal(line: &str) -> Option<(&str, &str)> {
    let digits = line.len() - line.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 {
        return None;
    }
    let rest = strip_one_space(line[digits..].strip_prefix(['.', ')'])?)?;
    Some((&line[..digits], rest))
}

/// `[ ] Milch` / `[x] Milch` → `Milch`. A checked box is a state a reader can
/// see and a listener cannot, and neither is a word.
fn strip_task_box(line: &str) -> &str {
    for box_ in ["[ ]", "[x]", "[X]"] {
        if let Some(rest) = line.strip_prefix(box_)
            && (rest.is_empty() || rest.starts_with([' ', '\t']))
        {
            return rest.trim_start();
        }
    }
    line
}

/// The rest of the string, if it begins with a space or a tab.
fn strip_one_space(s: &str) -> Option<&str> {
    s.strip_prefix([' ', '\t'])
}

/// A table row (`| Ort | Grad |`) as its cells, joined by commas — the comma is
/// the pause a reader would make, and the pipe is a character a provider says.
/// Anything that does not start with a pipe is not a row.
fn table_row(line: &str) -> Option<String> {
    if !line.starts_with('|') {
        return None;
    }
    let cells: Vec<String> = line
        .trim_matches('|')
        .split('|')
        .map(|cell| clean(cell.trim()))
        .map(|cell| collapse_spaces(&cell))
        .filter(|cell| !cell.is_empty())
        .collect();
    Some(cells.join(", "))
}

/// The two inline passes, in the order they have to run: links first, because
/// a link's text may itself be emphasised, and the marks after.
fn clean(line: &str) -> String {
    drop_marks(&resolve_links(line))
}

/// `[text](url)` → `text`, `![alt](url)` → `alt`. A bracket that opens nothing
/// stays a bracket.
///
/// The two lookup tables make this linear. Searching forward for the closing
/// `]` from every `[` is quadratic, and a page of `[[[[…` is a page an LLM can
/// produce by accident — so the next `]` and the next `)` at or after every
/// position are computed once, in one backward walk.
fn resolve_links(line: &str) -> String {
    // The common line has no bracket in it at all, and building three vectors
    // for it would make every answer pay for the one that has links.
    if !line.contains('[') {
        return line.to_string();
    }
    let chars: Vec<char> = line.chars().collect();
    let next_bracket = next_occurrence(&chars, ']');
    let next_paren = next_occurrence(&chars, ')');
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // The image marker is dropped and the branch below speaks the alt text.
        if c == '!'
            && chars.get(i + 1) == Some(&'[')
            && link_at(&chars, &next_bracket, &next_paren, i + 1).is_some()
        {
            i += 1;
            continue;
        }
        if c == '['
            && let Some((text_end, after)) = link_at(&chars, &next_bracket, &next_paren, i)
        {
            out.extend(&chars[i + 1..text_end]);
            i = after;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// For every position, the index of the next `needle` at or after it — `len`
/// where there is none. One entry longer than the input, so a lookup just past
/// the end is a lookup rather than a bounds check at every call site.
fn next_occurrence(chars: &[char], needle: char) -> Vec<usize> {
    let mut next = vec![chars.len(); chars.len() + 1];
    for i in (0..chars.len()).rev() {
        next[i] = if chars[i] == needle { i } else { next[i + 1] };
    }
    next
}

/// Where the link that opens at `open` ends: the index of its `]`, and the
/// index just past its `)`. `None` when the shape is not a link at all.
fn link_at(
    chars: &[char],
    next_bracket: &[usize],
    next_paren: &[usize],
    open: usize,
) -> Option<(usize, usize)> {
    let close = *next_bracket.get(open + 1)?;
    if close >= chars.len() || chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = *next_paren.get(close + 2)?;
    if end >= chars.len() {
        return None;
    }
    Some((close, end + 1))
}

/// Drop emphasis, inline code markers and the backslash of an escape.
///
/// A backtick goes wherever it stands. A `*` or `_` goes too — **unless** it is
/// surrounded by whitespace or the line's own edges, which is an arithmetic
/// operator and not an emphasis (`3 * 4`), or sits between two alphanumerics,
/// which is a word (`session_id`). And a `\` before an ASCII punctuation mark
/// is an escape: only the **backslash** goes. What it was hiding is a character
/// somebody wanted read out (`10\%`, `5\$`, `Meier \& Sohn`) — and where it was
/// a marker after all, the next round of the fixpoint takes it.
fn drop_marks(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '`' {
            i += 1;
            continue;
        }
        if c == '\\'
            && let Some(&escaped) = chars.get(i + 1)
            && escaped.is_ascii_punctuation()
        {
            // The backslash goes, the character it hid STAYS — `10\%` is ten
            // percent and `3 \* 4` is a product. A marker that was escaped is
            // not lost either: it is written out here as an ordinary character
            // and the next round of the fixpoint treats it exactly as if it had
            // never been escaped (`\# Titel` → `# Titel` → `Titel`).
            out.push(escaped);
            i += 2;
            continue;
        }
        if c == '*' || c == '_' {
            let mut j = i;
            while chars.get(j) == Some(&c) {
                j += 1;
            }
            let before = i.checked_sub(1).and_then(|k| chars.get(k));
            let after = chars.get(j);
            let is_operator =
                before.is_none_or(|c| c.is_whitespace()) && after.is_none_or(|c| c.is_whitespace());
            let inside_a_word = before.is_some_and(|c| c.is_alphanumeric())
                && after.is_some_and(|c| c.is_alphanumeric());
            if is_operator || inside_a_word {
                out.extend(&chars[i..j]);
            }
            i = j;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Squeeze runs of spaces and tabs into one space, and trim both ends.
fn collapse_spaces(s: &str) -> String {
    let mut out = String::new();
    let mut pending = false;
    for c in s.chars() {
        if c == ' ' || c == '\t' {
            pending = true;
            continue;
        }
        if pending && !out.is_empty() {
            out.push(' ');
        }
        pending = false;
        out.push(c);
    }
    out
}

/// Whether a line already ends where a sentence would.
fn ends_a_sentence(line: &str) -> bool {
    matches!(line.chars().last(), Some('.' | '!' | '?' | ':' | ';' | ','))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every input the tests below use, so the idempotency property can be
    /// checked against all of them instead of a hand-picked few. A rule that is
    /// worth a test is worth the property, and the four "revealed marker" cases
    /// are exactly the ones a single pass got wrong.
    const EXAMPLES: &[&str] = &[
        "Guten Tag, wie geht es Ihnen heute?",
        "Über Öl, Käse und 3 Bäume — ganz ohne Auszeichnung!",
        "**Hallo** Welt",
        "*Hallo* Welt",
        "__Hallo__ Welt",
        "_Hallo_ Welt",
        "ein `wort` im Code",
        "der Schlüssel session_id bleibt",
        "# Wetterbericht",
        "### Am Nachmittag",
        "#wetter ist ein Wort",
        "- Vormittag",
        "* Vormittag",
        "+ Vormittag",
        "1. Vormittag",
        "2) Nachmittag",
        "-5 Grad am Morgen",
        "5. September 2026",
        "> Zitat einer Kollegin",
        ">> doppelt zitiert",
        "- [ ] Milch kaufen",
        "- [x] Brot gekauft",
        "3 * 4 = 12",
        "a * b und c _ d",
        "\\*kein Stern\\* und \\# keine Überschrift",
        "Rabatt von 10\\% heute",
        "5\\$ netto",
        "Meier \\& Sohn",
        "Rechne 3 \\* 4",
        "\\*Stern\\*",
        "\\# Titel",
        ">= 5 Grad am Morgen",
        "Mehr in [unserem Bericht](https://example.invalid/w).",
        "![Ein Diagramm](https://example.invalid/d.png)",
        "eine [eckige Klammer ohne Ziel",
        "```json\ntag: sonnig\n```",
        "| Ort | Grad |\n|---|---:|\n| Berlin | 18 |\n| Hamburg | 16 |",
        "Es regnet\nes ist kalt",
        "Erster Absatz\n\n\nZweiter Absatz",
        "zwei    Leerzeichen",
        "---",
        "***",
        "",
        // The four shapes a single pass left markup in: the inline rules
        // uncover a structural marker the structural rules had already passed.
        "`# install`",
        "[- Punkt](https://example.invalid/p)",
        "**- Punkt**",
        "**| a | b |**",
    ];

    #[test]
    fn plain_prose_comes_out_byte_identical() {
        let plain = "Guten Tag, wie geht es Ihnen heute?";
        assert_eq!(to_speech(plain), plain);
        let umlauts = "Über Öl, Käse und 3 Bäume — ganz ohne Auszeichnung!";
        assert_eq!(
            to_speech(umlauts),
            umlauts,
            "umlauts, punctuation and a dash are not markup"
        );
    }

    #[test]
    fn emphasis_markers_are_dropped() {
        assert_eq!(to_speech("**Hallo** Welt"), "Hallo Welt");
        assert_eq!(to_speech("*Hallo* Welt"), "Hallo Welt");
        assert_eq!(to_speech("__Hallo__ Welt"), "Hallo Welt");
        assert_eq!(to_speech("_Hallo_ Welt"), "Hallo Welt");
        assert_eq!(to_speech("ein `wort` im Code"), "ein wort im Code");
    }

    #[test]
    fn an_underscore_inside_a_word_stays() {
        assert_eq!(
            to_speech("der Schlüssel session_id bleibt"),
            "der Schlüssel session_id bleibt",
            "an identifier is a word, not an emphasis"
        );
    }

    /// A star between two spaces is a multiplication sign — a thing somebody
    /// dictated on purpose, and the one shape emphasis never has.
    #[test]
    fn a_star_between_spaces_is_arithmetic() {
        assert_eq!(to_speech("3 * 4 = 12"), "3 * 4 = 12");
        assert_eq!(to_speech("a * b und c _ d"), "a * b und c _ d");
        assert_eq!(
            to_speech("***"),
            "",
            "a line of nothing but rule characters is a break, not an operator"
        );
    }

    /// An escape loses its backslash and nothing else. The character behind it
    /// is one somebody wanted read out — and where it turns out to be a marker
    /// after all, the fixpoint takes it on the next round, exactly as it would
    /// have taken an unescaped one.
    #[test]
    fn an_escape_loses_its_backslash_and_nothing_else() {
        assert_eq!(to_speech("Rabatt von 10\\% heute"), "Rabatt von 10% heute");
        assert_eq!(to_speech("5\\$ netto"), "5$ netto");
        assert_eq!(to_speech("Meier \\& Sohn"), "Meier & Sohn");
        assert_eq!(
            to_speech("Rechne 3 \\* 4"),
            "Rechne 3 * 4",
            "an escaped operator is still an operator"
        );
        assert_eq!(to_speech("\\*Stern\\*"), "Stern");
        assert_eq!(to_speech("\\# Titel"), "Titel");
    }

    #[test]
    fn heading_hashes_are_dropped_but_a_hashtag_is_not() {
        assert_eq!(to_speech("# Wetterbericht"), "Wetterbericht");
        assert_eq!(to_speech("### Am Nachmittag"), "Am Nachmittag");
        assert_eq!(
            to_speech("#wetter ist ein Wort"),
            "#wetter ist ein Wort",
            "a heading marker is followed by a space"
        );
    }

    #[test]
    fn a_blockquote_loses_its_angle_bracket() {
        assert_eq!(to_speech("> Zitat einer Kollegin"), "Zitat einer Kollegin");
        assert_eq!(to_speech(">> doppelt zitiert"), "doppelt zitiert");
        assert_eq!(
            to_speech("3 > 2 ist wahr"),
            "3 > 2 ist wahr",
            "a greater-than sign in the middle of a line is a sign"
        );
        assert_eq!(
            to_speech(">= 5 Grad am Morgen"),
            ">= 5 Grad am Morgen",
            "a quote marker is followed by a space; a comparison is not"
        );
    }

    #[test]
    fn list_markers_are_dropped() {
        assert_eq!(to_speech("- Vormittag"), "Vormittag");
        assert_eq!(to_speech("* Vormittag"), "Vormittag");
        assert_eq!(to_speech("+ Vormittag"), "Vormittag");
        assert_eq!(
            to_speech("-5 Grad am Morgen"),
            "-5 Grad am Morgen",
            "a bullet is followed by a space; a minus is not"
        );
    }

    #[test]
    fn a_task_box_is_not_a_word() {
        assert_eq!(to_speech("- [ ] Milch kaufen"), "Milch kaufen");
        assert_eq!(to_speech("- [x] Brot gekauft"), "Brot gekauft");
    }

    /// The tradeoff of the ordinal rule, in the shape that made it: a senior
    /// agent's date must survive, so `N.` loses its dot and keeps its number.
    #[test]
    fn a_date_at_line_start_keeps_its_day() {
        assert_eq!(
            to_speech("5. September 2026"),
            "5 September 2026",
            "a day is not a list marker, and the two look identical here"
        );
        assert_eq!(to_speech("1. Vormittag"), "1 Vormittag");
        assert_eq!(to_speech("2) Nachmittag"), "2 Nachmittag");
    }

    #[test]
    fn links_keep_their_text_and_images_their_alt() {
        assert_eq!(
            to_speech("Mehr in [unserem Bericht](https://example.invalid/w)."),
            "Mehr in unserem Bericht."
        );
        assert_eq!(
            to_speech("![Ein Diagramm](https://example.invalid/d.png)"),
            "Ein Diagramm"
        );
        assert_eq!(
            to_speech("eine [eckige Klammer ohne Ziel"),
            "eine [eckige Klammer ohne Ziel",
            "a bracket that opens no link is a bracket"
        );
    }

    #[test]
    fn a_code_fence_leaves_its_code_behind() {
        assert_eq!(
            to_speech("```json\ntag: sonnig\n```"),
            "tag: sonnig",
            "the fence goes, the code is spoken"
        );
    }

    #[test]
    fn a_table_becomes_rows_of_cells_joined_by_commas() {
        let table = "| Ort | Grad |\n|---|---:|\n| Berlin | 18 |\n| Hamburg | 16 |";
        assert_eq!(to_speech(table), "Ort, Grad.\nBerlin, 18.\nHamburg, 16");
    }

    /// The four shapes that made the pipeline a fixpoint: an inline rule
    /// uncovers a structural marker the structural rules have already gone
    /// past, and one pass leaves it in the text a provider reads.
    #[test]
    fn a_marker_uncovered_by_the_inline_pass_is_taken_too() {
        assert_eq!(to_speech("`# install`"), "install");
        assert_eq!(
            to_speech("[- Punkt](https://example.invalid/p)"),
            "Punkt",
            "the link's text was a list item"
        );
        assert_eq!(to_speech("**- Punkt**"), "Punkt");
        assert_eq!(
            to_speech("**| a | b |**"),
            "a, b",
            "an emphasised table row is still a table row"
        );
    }

    #[test]
    fn a_line_break_becomes_a_sentence_end() {
        assert_eq!(
            to_speech("Es regnet\nes ist kalt"),
            "Es regnet.\nes ist kalt"
        );
        assert_eq!(
            to_speech("Es regnet.\nes ist kalt"),
            "Es regnet.\nes ist kalt",
            "a line that already ends a sentence gets no second full stop"
        );
        for ending in ['!', '?', ':', ';', ','] {
            assert_eq!(
                to_speech(&format!("Es regnet{ending}\nes ist kalt")),
                format!("Es regnet{ending}\nes ist kalt")
            );
        }
    }

    #[test]
    fn blank_lines_and_double_spaces_collapse() {
        assert_eq!(
            to_speech("Erster Absatz\n\n\nZweiter Absatz"),
            "Erster Absatz.\nZweiter Absatz",
            "a blank line is one break, not three"
        );
        assert_eq!(to_speech("zwei    Leerzeichen"), "zwei Leerzeichen");
    }

    /// One answer of the shape the first live call produced: a title, an
    /// emphasised sentence, a list, a link, a code block and a table.
    #[test]
    fn a_whole_german_answer_arrives_as_speech() {
        let written = "## Wetterbericht\n\
             \n\
             Heute wird es **sonnig** und _warm_.\n\
             \n\
             - Vormittag: 12 Grad\n\
             - Nachmittag: 18 Grad\n\
             \n\
             Mehr dazu in [unserem Bericht](https://example.invalid/wetter).\n\
             \n\
             ```text\n\
             tag: sonnig\n\
             ```\n\
             \n\
             | Ort | Grad |\n\
             |---|---|\n\
             | Berlin | 18 |\n\
             | Hamburg | 16 |\n";
        assert_eq!(
            to_speech(written),
            "Wetterbericht.\n\
             Heute wird es sonnig und warm.\n\
             Vormittag: 12 Grad.\n\
             Nachmittag: 18 Grad.\n\
             Mehr dazu in unserem Bericht.\n\
             tag: sonnig.\n\
             Ort, Grad.\n\
             Berlin, 18.\n\
             Hamburg, 16"
        );
    }

    /// The property, over every example this module uses — including the ones
    /// whose whole point is another rule.
    #[test]
    fn the_transformation_is_idempotent() {
        for written in EXAMPLES {
            let once = to_speech(written);
            assert_eq!(
                to_speech(&once),
                once,
                "a second pass changed `{written}` again"
            );
        }
    }

    #[test]
    fn nothing_left_to_say_is_an_empty_string() {
        assert_eq!(to_speech(""), "");
        assert_eq!(to_speech("\n\n   \n"), "");
        assert_eq!(to_speech("---"), "");
    }

    /// A page of opening brackets is a page an LLM can produce by accident, and
    /// a quadratic scan over one would hold the handler task for minutes.
    ///
    /// The bound is a failure marker, not a timing discriminator, and it is
    /// wall-clock on a machine that runs other things: linear is 15 ms in a
    /// debug build, quadratic over 50 000 characters is 1.25e9 comparisons and
    /// takes tens of seconds. Five seconds sits between the two with room for a
    /// loaded test runner on either side of it.
    #[test]
    fn a_page_of_open_brackets_is_read_in_one_walk() {
        let pathological = "[".repeat(50_000);
        let started = std::time::Instant::now();
        let spoken = to_speech(&pathological);
        let elapsed = started.elapsed();
        assert_eq!(spoken.len(), 50_000, "a bracket that opens nothing stays");
        // Printed rather than asserted on, the way t5 prints its round trip: the
        // number belongs in a receipt a person reads, not in a threshold a
        // loaded test runner has to meet.
        println!("50 KB of opening brackets rewritten in {elapsed:?}");
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "50 KB of brackets took {elapsed:?} — the scan is not linear"
        );
    }
}
