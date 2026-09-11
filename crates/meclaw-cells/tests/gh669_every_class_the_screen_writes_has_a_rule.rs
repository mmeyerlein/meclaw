//! GH #669 -- every class the screen writes has a rule, and the page stays in
//! budget.
//!
//! A sheet and a catalogue that were taken from one kit can still drift apart
//! in the taking: a prefix swapped in one and not the other, a rule cut with
//! the section it stood in. So the thirty-one templates are read for the
//! classes they write, the way the `web` cell's own scanner reads them, and
//! every class that is not borrowed from `/vision.css` has to have a rule in
//! what the shell ships: its one `<style>` block, which is the screen's own
//! layout followed by the sheet. The second half is weight: the shell is the
//! one part of the page this wave lets grow, so it is rendered and measured,
//! raw and compressed.
//!
//! The shell is rendered the way the `web` cell renders one object: the cell's
//! own parser cuts the template, raw props stay raw only where `prop_schema`
//! says `html`. Skips when `python3` is absent or the templates do not ship,
//! like every other interpreter guard in this tree (R2b).

use std::collections::BTreeSet;
use std::io::Write;
use std::process::{Command, Stdio};

use meclaw_cells::web::render::{Piece, escape, parse_template};
use meclaw_core::serde_json::{Value, json};

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";
const SHEET: &str = "templates/display/compose/display-dna.css";

/// The classes the catalogue borrows from `/vision.css`, the base language of
/// every `web` cell, which the sheet re-tokenises rather than restates. A name
/// goes on this list only when `templates/web/seed/assets.jsonl` defines it.
const BORROWED: [&str; 8] = [
    "glass",
    "glass--thin",
    "glass--thick",
    "inner",
    "stack",
    "card",
    "title-3",
    "text",
];

/// How many classes the catalogue wrote when this lock was written. A lower
/// bound and not an equality: a catalogue that grows is not a red test, a
/// scanner that stopped reading is.
const CLASSES_AT_LEAST: usize = 115;

/// The five blocks that carry the design language onto a screen that cannot
/// render it as drawn (E15).
const FALLBACK_BLOCKS: [&str; 5] = [
    "@supports not ((backdrop-filter: blur(1px)) or (-webkit-backdrop-filter: blur(1px)))",
    "@media (prefers-reduced-transparency: reduce)",
    "@media (prefers-contrast: more)",
    "@media (forced-colors: active)",
    "@media (prefers-reduced-motion: reduce)",
];

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// `components()` as the shipped script defines them, asked of the script
/// itself. `None` when there is no `python3` on this host.
fn components() -> Option<Vec<Value>> {
    let out = Command::new("python3")
        .arg("-c")
        .arg(
            "import importlib.util, json, sys\n\
             spec = importlib.util.spec_from_file_location('compose', sys.argv[1])\n\
             m = importlib.util.module_from_spec(spec)\n\
             spec.loader.exec_module(m)\n\
             print(json.dumps(m.components()))",
        )
        .arg(repo(COMPOSE))
        .output()
        .ok()?;
    assert!(
        out.status.success(),
        "compose.py did not load:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = meclaw_core::serde_json::from_slice(&out.stdout).expect("components() is JSON");
    Some(v.as_array().expect("a list").clone())
}

/// `src` with its CSS comments cut: a class named only in a comment is not a
/// rule.
fn without_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(open) = rest.find("/*") {
        out.push_str(&rest[..open]);
        out.push(' ');
        match rest[open + 2..].find("*/") {
            Some(close) => rest = &rest[open + 2 + close + 2..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The `display-shell`, rendered with the given faces, the way it reaches a
/// page.
fn shell_with(all: &[Value], faces: &str) -> String {
    let shell = all
        .iter()
        .find(|c| c["name"] == "display-shell")
        .expect("components() defines no display-shell");
    render(
        shell["template"].as_str().expect("a template"),
        &json!({"stylesheet": true, "faces": faces, "vocab": "000000000000"}),
        &shell["prop_schema"],
    )
}

/// The rules the screen ships: the inside of the shell's one `<style>` block,
/// which is the layout (where the columns and the microphone sit) followed by
/// the sheet (what everything looks like). Comments cut.
fn shipped_rules(all: &[Value]) -> String {
    let page = shell_with(all, "");
    let start = page
        .find("<style>")
        .expect("the shell carries a style block")
        + "<style>".len();
    let end = page[start..]
        .find("</style>")
        .expect("the style block closes")
        + start;
    without_comments(&page[start..end])
}

/// The class names a template writes, by the `web` cell's own rule: every
/// `{{…}}` becomes a space first, then every `class="…"` is split on
/// whitespace. A token that does not begin with a letter is a fragment --
/// `glass{{#if thin}}--thin{{/if}}` leaves `--thin` standing alone -- and never
/// reaches a page that way.
fn class_tokens(template: &str) -> Vec<String> {
    let mut plain = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        plain.push_str(&rest[..open]);
        plain.push(' ');
        match rest[open + 2..].find("}}") {
            Some(close) => rest = &rest[open + 2 + close + 2..],
            None => {
                rest = "";
                break;
            }
        }
    }
    plain.push_str(rest);

    let mut out = Vec::new();
    let mut hay = plain.as_str();
    while let Some(at) = hay.find("class=") {
        let after = &hay[at + "class=".len()..];
        let Some(quote) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            hay = after;
            continue;
        };
        let body = &after[1..];
        let Some(end) = body.find(quote) else {
            break;
        };
        out.extend(
            body[..end]
                .split_whitespace()
                .filter(|t| t.starts_with(|c: char| c.is_ascii_alphabetic()))
                .map(str::to_string),
        );
        hay = &body[end + 1..];
    }
    out
}

/// Whether `rules` has a rule for `.name` -- the selector followed by
/// something that is not part of a class name, so `.display-document-pages`
/// does not answer for `display-document-page`.
fn has_rule(rules: &str, name: &str) -> bool {
    let needle = format!(".{name}");
    let mut from = 0;
    while let Some(at) = rules[from..].find(&needle) {
        let end = from + at + needle.len();
        let next = rules[end..].chars().next();
        if !next.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return true;
        }
        from = end;
    }
    false
}

/// Render one component template with its props, the way the `web` cell does
/// for one object with no children: the cell's parser, the cell's escaping,
/// `html` as the only raw type, `{{children}}` empty.
fn render(template: &str, props: &Value, schema: &Value) -> String {
    let pieces = parse_template(template)
        .unwrap_or_else(|e| panic!("the web cell would refuse this template: {e}"));
    let mut out = String::new();
    render_pieces(&pieces, props, schema, &mut out);
    out
}

fn render_pieces(pieces: &[Piece], props: &Value, schema: &Value, out: &mut String) {
    let text = |name: &str| -> String {
        match props.get(name) {
            None | Some(Value::Null) => String::new(),
            Some(Value::String(s)) => s.clone(),
            Some(other) => other.to_string(),
        }
    };
    for piece in pieces {
        match piece {
            Piece::Text(t) => out.push_str(t),
            Piece::Prop(name) => out.push_str(&escape(&text(name))),
            Piece::Raw(name) => {
                if schema.get(name).and_then(Value::as_str) == Some("html") {
                    out.push_str(&text(name));
                } else {
                    out.push_str(&escape(&text(name)));
                }
            }
            Piece::Children => {}
            Piece::If { prop, body } => {
                let on = match props.get(prop) {
                    None | Some(Value::Null) => false,
                    Some(Value::Bool(b)) => *b,
                    Some(Value::String(s)) => !s.is_empty(),
                    Some(other) => !other.is_null(),
                };
                if on {
                    render_pieces(body, props, schema, out);
                }
            }
        }
    }
}

/// The bytes of `data` after gzip at level 6, the level a server sends at.
/// `None` when there is no `python3` on this host.
fn gzipped_len(data: &[u8]) -> Option<usize> {
    let mut child = Command::new("python3")
        .arg("-c")
        .arg("import gzip, sys; print(len(gzip.compress(sys.stdin.buffer.read(), 6)))")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .ok()?;
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(data)
        .expect("the bytes reach python");
    let out = child.wait_with_output().expect("python ends");
    assert!(out.status.success(), "gzip did not answer");
    Some(
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse()
            .expect("a byte count"),
    )
}

/// Check 3 of the kit's own suite, in this tree: every class any of the
/// thirty-one templates writes has a rule in what the shell ships, unless
/// `/vision.css` owns it. Target: none without a rule.
#[test]
fn every_class_the_catalogue_writes_has_a_rule() {
    if !library_ships() {
        return;
    }
    let Some(all) = components() else {
        return;
    };
    let rules = shipped_rules(&all);
    let mut classes = BTreeSet::new();
    for c in &all {
        classes.extend(class_tokens(c["template"].as_str().expect("a template")));
    }
    assert!(
        classes.len() >= CLASSES_AT_LEAST,
        "the scanner read {} classes, fewer than the {CLASSES_AT_LEAST} the catalogue wrote when this lock was written: {classes:?}",
        classes.len()
    );
    let missing: Vec<&String> = classes
        .iter()
        .filter(|c| !BORROWED.contains(&c.as_str()) && !has_rule(&rules, c))
        .collect();
    assert!(
        missing.is_empty(),
        "{} of {} classes have no rule in what the shell ships: {missing:?}",
        missing.len(),
        classes.len()
    );
}

/// The shell at `font_base=""` -- the shipped default -- is the whole of what
/// this wave adds to the page. Under 80,000 bytes raw and under 30,000 gzipped,
/// because the page is measured and the shell is what grew.
#[test]
fn the_shell_stays_inside_its_weight_budget() {
    if !library_ships() {
        return;
    }
    let Some(all) = components() else {
        return;
    };
    let page = shell_with(&all, "");
    let raw = page.len();
    assert!(raw < 80_000, "the shell weighs {raw} bytes raw");
    let Some(zipped) = gzipped_len(page.as_bytes()) else {
        return;
    };
    assert!(zipped < 30_000, "the shell weighs {zipped} bytes gzipped");
    assert!(
        !page.contains("@font-face"),
        "an empty base declares no face"
    );
}

/// The five fallback blocks the design language carries, each at least once
/// in the sheet (E15).
#[test]
fn the_sheet_carries_all_five_fallback_blocks() {
    if !library_ships() {
        return;
    }
    let sheet = without_comments(&read(SHEET));
    for block in FALLBACK_BLOCKS {
        assert!(
            sheet.matches(block).count() >= 1,
            "the sheet carries no `{block}` block"
        );
    }
}
