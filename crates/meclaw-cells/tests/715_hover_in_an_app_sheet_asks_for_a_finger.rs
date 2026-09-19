//! GH #715 -- an application's sheet asks for a finger too (`display-hive.md`
//! § 6.4).
//!
//! "`inputs` gates what is bound and shown … Not in the model (client):
//! without `pointer` and without `touch` no `:hover` rules" (§ 6.4). A
//! television is an output device (the maintainer's ruling of 14.09.): its
//! profile carries `inputs: []`, so a hover state on it is a colour nobody
//! asked for and nobody can leave.
//!
//! The screen's own sheet has been held to that by
//! `708_hover_exists_only_where_a_finger_does` since strand H4. `colony-view`
//! is an application, its `client_css` goes into the same page raw (ADR 0036),
//! and it was never held to anything: measured against `e25`, B-13 found one of
//! seven `:hover` rules on the page unguarded, and it was this sheet's. The two
//! others in it were invisible to the proof only because nothing on that stage
//! matched them.
//!
//! The gate is the screen's own word, `data-inputs` on `.display-columns`, so
//! the sheet needs no second sheet and no media query. The browser proof is
//! B-13 (strand H5); this is the text lock, and it reads the file a person
//! edits AND the copy the runtime is handed.

use meclaw_core::serde_json::Value;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const SHEET: &str = "templates/colony-view/layout/colony-view.css";
const CONFIG: &str = "templates/colony-view/layout/config.json";

fn library_ships() -> bool {
    repo("templates/colony-view/template.json").is_file()
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

/// The sheet as the runtime gets it: `CLIENT_CSS` cut out of the `layout.py`
/// that `config.json` carries in `script_inline`. Same dumb extraction as the
/// three-copies lock.
fn shipped_sheet() -> String {
    let cfg: Value = meclaw_core::serde_json::from_str(&read(CONFIG))
        .unwrap_or_else(|e| panic!("{CONFIG}: {e}"));
    let script = cfg["params"]["script_inline"]
        .as_str()
        .expect("layout/config.json carries script_inline");
    let open = "CLIENT_CSS = r\"\"\"";
    let start = script.find(open).expect("script_inline carries CLIENT_CSS") + open.len();
    let end = script[start..].find("\"\"\"").expect("CLIENT_CSS closes") + start;
    script[start..end].to_string()
}

/// What the gate looks like: the screen's root carries the profile's input
/// words as a space-joined list, so `~=` is the word match (§ 6.4).
const FINGER: [&str; 2] = ["[data-inputs~=\"pointer\"]", "[data-inputs~=\"touch\"]"];

#[test]
fn every_hover_rule_in_the_app_sheet_asks_for_a_finger_first() {
    if !library_ships() {
        eprintln!("skip: no template library in this tree");
        return;
    }
    for (name, source) in [
        ("the file", read(SHEET)),
        ("the shipped copy", shipped_sheet()),
    ] {
        let sheet = without_comments(&source);
        let mut seen = 0;
        for selector in selector_parts(&sheet) {
            if !selector.contains(":hover") {
                continue;
            }
            seen += 1;
            assert!(
                FINGER.iter().all(|f| selector.contains(f)),
                "{name}: `{selector}` lights up without asking whether this output has a \
                 finger -- § 6.4: without `pointer` and without `touch` no `:hover` rules. \
                 Put the two `[data-inputs~=…]` conditions on an ancestor."
            );
        }
        assert!(
            seen > 0,
            "{name}: {SHEET} has no `:hover` rule at all -- the rules moved"
        );
    }
}
