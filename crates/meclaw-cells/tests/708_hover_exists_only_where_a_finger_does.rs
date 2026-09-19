//! GH #708 -- `:hover` exists only where a finger does (`display-hive.md` § 6.4).
//!
//! "`inputs` gates what is bound and shown … without `pointer` and without
//! `touch` no `:hover` rules" (§ 6.4). A television is an output device (the
//! maintainer's ruling of 14.09.): its profile carries `inputs: []`, and a
//! hover state on it is a promise nothing can keep -- a kiosk browser with a
//! stray cursor lights a control nobody can press.
//!
//! The gate is the root's own word, `data-inputs` (plan § 4, `ATTRS`), so the
//! sheet needs no second sheet and no media query. The browser proof is B-13
//! (strand H5); this is the text lock.

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const SHEET: &str = "templates/display/compose/display-dna.css";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn without_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let b = src.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            match src[i + 2..].find("*/") {
                Some(end) => i = i + 2 + end + 2,
                None => break,
            }
        } else {
            out.push(b[i] as char);
            i += 1;
        }
    }
    out
}

/// One selector list into its parts. Only a comma OUTSIDE parentheses
/// separates two selectors: `:is(a, b)` is one condition, and splitting inside
/// it would read the closing half as a selector of its own.
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

/// Every selector of the sheet, one per comma-separated part, at-rules walked
/// into. The gate has to stand on the part itself: a selector list where only
/// one half is gated leaves the other half ungated.
fn selector_parts(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let b = src.as_bytes();
    let (mut i, mut start) = (0usize, 0usize);
    while i < b.len() {
        match b[i] {
            b'{' => {
                let head = src[start..i].trim().to_string();
                if !head.starts_with('@') {
                    for p in comma_parts(&head) {
                        if !p.is_empty() {
                            out.push(p);
                        }
                    }
                    let end = src[i..].find('}').map(|e| i + e).unwrap_or(b.len());
                    i = end;
                }
                i += 1;
                start = i;
            }
            b'}' => {
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    out
}

/// What the gate looks like: the root carries the profile's input words as a
/// space-joined list, so `~=` is the word match (§ 6.4, plan § 4).
const FINGER: [&str; 2] = ["[data-inputs~=\"pointer\"]", "[data-inputs~=\"touch\"]"];

#[test]
fn every_hover_rule_asks_for_a_finger_first() {
    if !library_ships() {
        eprintln!("skip: no template library in this tree");
        return;
    }
    let sheet = without_comments(&read(SHEET));
    let mut seen = 0;
    for selector in selector_parts(&sheet) {
        if !selector.contains(":hover") {
            continue;
        }
        seen += 1;
        assert!(
            FINGER.iter().all(|f| selector.contains(f)),
            "`{selector}` lights up without asking whether this output has a \
             finger -- § 6.4: without `pointer` and without `touch` no `:hover` \
             rules. Put the two `[data-inputs~=…]` conditions on an ancestor."
        );
    }
    assert!(
        seen > 0,
        "{SHEET} has no `:hover` rule at all -- the rules moved"
    );
}
