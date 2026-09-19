//! display-hive.md § 0.3 and `docs/development-rules.md` § 10: the template's README is the
//! public rendering of the binding description, never a second source. A rendering drifts in
//! two ways. It carries a number the cell no longer holds, and it keeps a word the
//! description struck. Both are checked here against the shipped bytes: every number of the
//! README's settings and profile tables is read out of `compose.py` first and then looked
//! for in the README, and the vocabulary of the description is held against the words that
//! were struck with it.
use std::fs;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const README: &str = "templates/display/README.md";
const COMPOSE: &str = "templates/display/compose/compose.py";

/// The line that says which document rules. It stands third, under the title and one blank
/// line, so a reader meets it before the first claim.
const NORMATIVE: &str = "> **Normative source:**";

/// Every number the README's two tables give, with the constant of `compose.py` it is read
/// out of. The test does not trust this column: it reads the constant's definition and
/// asserts the literal stands in it, so a value that moves in the cell fails here before
/// anybody compares it to the README.
const NUMBERS: [(&str, &str); 14] = [
    ("DEFAULT_SETTINGS", "20000"),  // linger_ms
    ("DEFAULT_SETTINGS", "120000"), // fade_ms
    ("DEFAULT_SETTINGS", "0.3"),    // focus_default
    ("DEFAULT_SETTINGS", "3000"),   // judge_min_interval_ms
    ("PROFILE_DEFAULTS", "7"),      // dock_max, tv
    ("PROFILE_DEFAULTS", "8"),      // dock_max, monitor
    ("PROFILE_DEFAULTS", "5"),      // dock_max, phone
    ("SCREEN_BASE", "1.6"),         // the base scale of a tv
    ("SCREEN_REFERENCE", "3.0"),    // the reference distance of a tv
    ("SCREEN_REFERENCE", "0.7"),    // of a monitor
    ("SCREEN_REFERENCE", "0.35"),   // of a phone
    ("SCALE_MIN", "0.8"),
    ("SCALE_MAX", "2.2"),
    ("TILE_LINE_MAX", "24"), // the tile's line
];

/// The words the description uses, and the README therefore has to use.
const SAYS: [&str; 9] = [
    "tap",
    "hold",
    "rung",
    "level",
    "led_until",
    "turn_id",
    "dock_max",
    "default_screen",
    "screens",
];

/// The words the description struck (§ 2, § 10). A README that still carries one of them
/// promises a screen this template no longer is.
const SAYS_NOT: [&str; 6] = [
    "plane",
    "canvas_slots",
    "dock_overflow",
    "modal: true",
    "phx-click=\"tile\"",
    "pushEvent(\"touch\"",
];

/// The scenarios travel with the template, so the README can point at them.
const SCENARIOS: &str = "compose/scenarios/";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

/// The text of a module-level assignment in `compose.py`: from the name to the end of its
/// last line, a value spread over several lines included.
fn definition(src: &str, name: &str) -> String {
    let head = format!("\n{name} = ");
    let at = src
        .find(&head)
        .unwrap_or_else(|| panic!("`{name}` is not defined in compose.py"))
        + 1;
    let mut depth = 0i32;
    let mut out = String::new();
    for c in src[at..].chars() {
        if c == '\n' && depth <= 0 {
            break;
        }
        match c {
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => depth -= 1,
            _ => {}
        }
        out.push(c);
    }
    out
}

/// The README without its ordered-list markers. `7. **Rungs.**` numbers a step of the pass;
/// it is not a value the cell holds, and it would otherwise be counted as one.
fn prose(readme: &str) -> String {
    readme
        .lines()
        .map(|line| {
            let body = line.trim_start();
            match body.find(". ") {
                Some(i) if i > 0 && body[..i].chars().all(|c| c.is_ascii_digit()) => &body[i + 2..],
                _ => line,
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether the text running away from a hit continues the number: a digit does, and so does
/// a dot with a digit behind it. A dot with anything else behind it is a full stop, so a
/// number at the end of a sentence still counts.
fn continues(mut rest: impl Iterator<Item = char>) -> bool {
    match rest.next() {
        Some(c) if c.is_ascii_digit() => true,
        Some('.') => matches!(rest.next(), Some(c) if c.is_ascii_digit()),
        _ => false,
    }
}

/// How often `needle` stands in `haystack` as a number of its own: a hit that is part of a
/// longer number does not count. That is what keeps the `5` of a version line (`2.5.0`) and
/// the `0.3` inside `0.35` out of the count.
fn as_a_number(haystack: &str, needle: &str) -> usize {
    let mut count = 0;
    let mut from = 0;
    while let Some(i) = haystack[from..].find(needle) {
        let at = from + i;
        if !continues(haystack[..at].chars().rev())
            && !continues(haystack[at + needle.len()..].chars())
        {
            count += 1;
        }
        from = at + needle.len();
    }
    count
}

/// The reader is told which document rules before anything else on the page.
#[test]
fn the_readme_names_its_source_in_its_third_line() {
    if !library_ships() {
        return;
    }
    let readme = fs::read_to_string(repo(README)).expect("README");
    let line = readme.lines().nth(2).expect("a third line");
    assert!(
        line.starts_with(NORMATIVE),
        "the third line of the README is the normative source, not: {line}"
    );
    assert!(
        line.contains("display-hive.md"),
        "and it names the document: {line}"
    );
}

/// Every number in the README's tables is the number the cell holds, and it stands exactly
/// once: twice is two places to update and therefore one place to forget.
#[test]
fn every_number_in_the_readme_comes_from_the_cell() {
    if !library_ships() {
        return;
    }
    let readme = prose(&fs::read_to_string(repo(README)).expect("README"));
    let src = fs::read_to_string(repo(COMPOSE)).expect("compose.py");
    for (constant, literal) in NUMBERS {
        let def = definition(&src, constant);
        assert!(
            def.contains(literal),
            "`{constant}` does not hold {literal} any more: {def}"
        );
        assert_eq!(
            as_a_number(&readme, literal),
            1,
            "the README states {literal} (from `{constant}`) exactly once"
        );
    }
}

/// The vocabulary: what the description says, and what it struck.
#[test]
fn the_readme_speaks_the_words_of_the_description() {
    if !library_ships() {
        return;
    }
    let readme = fs::read_to_string(repo(README)).expect("README");
    for word in SAYS {
        assert!(readme.contains(word), "the README does not name `{word}`");
    }
    for word in SAYS_NOT {
        assert!(
            !readme.contains(word),
            "the README still carries the struck `{word}`"
        );
    }
    assert!(
        readme.contains(SCENARIOS),
        "the README does not point at the scenarios (`{SCENARIOS}`)"
    );
}
