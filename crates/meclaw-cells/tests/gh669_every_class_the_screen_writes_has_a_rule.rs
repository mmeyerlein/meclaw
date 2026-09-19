//! GH #669 -- every class the screen writes has a rule, and the page stays in
//! budget.
//!
//! A sheet and a catalogue that were taken from one kit can still drift apart
//! in the taking: a prefix swapped in one and not the other, a rule cut with
//! the section it stood in. So the thirty-four templates are read for the
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

use meclaw_cells::web::render::render_pieces_plain;
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
/// goes on this list only when `templates/web/seed/assets.jsonl` defines it
/// AND a template in the catalogue writes it: a borrowed name nobody writes
/// is a hole this lock would not see through.
const BORROWED: [&str; 5] = ["glass", "glass--thin", "glass--thick", "inner", "stack"];

/// How many classes the catalogue wrote when this lock was written. A lower
/// bound and not an equality: a catalogue that grows is not a red test, a
/// scanner that stopped reading is.
const CLASSES_AT_LEAST: usize = 126;

/// The classes the catalogue writes that the sheet has no rule for yet.
///
/// Empty since the sheet strand of this wave (H2-T12): `display-seat` is the empty seat
/// in the dock (display-hive.md § 4.29: empty space, no placeholder) and
/// `display-chat-line-source` the per-line source a chat line wears since § 8.4 put the
/// SOURCE on a line where a clock used to stand -- both have their rule now. The list
/// stays as the shape for the next wave that splits a class from its sheet across two
/// strands; it is held to its own claim by the two asserts below, so a name in it has to
/// be written by the catalogue AND still be without a rule.
const SHEET_PENDING: [&str; 0] = [];

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
/// for one object with no children (`render_pieces_plain`: the cell's parser,
/// the cell's walk, the cell's truthiness).
fn render(template: &str, props: &Value, schema: &Value) -> String {
    render_pieces_plain(template, props, schema)
        .unwrap_or_else(|e| panic!("the web cell would refuse this template: {e}"))
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
/// thirty-four templates writes has a rule in what the shell ships, unless
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
    for pending in SHEET_PENDING {
        assert!(
            classes.contains(pending),
            "`{pending}` is no class the catalogue writes -- drop it from SHEET_PENDING"
        );
        assert!(
            !has_rule(&rules, pending),
            "the sheet has a rule for `{pending}` now -- drop it from SHEET_PENDING"
        );
    }
    let missing: Vec<&String> = classes
        .iter()
        .filter(|c| {
            !BORROWED.contains(&c.as_str())
                && !SHEET_PENDING.contains(&c.as_str())
                && !has_rule(&rules, c)
        })
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
    let zipped = gzipped_len(page.as_bytes());
    eprintln!(
        "shell weighs {raw} B raw, {} B gzipped",
        zipped.map_or("?".to_string(), |z| z.to_string())
    );
    // Two ceilings, and they guard different things. The GZIPPED one is the
    // budget: it is what travels, and it is what a person waits for. The RAW
    // one is a guard against the single failure that would really hurt --
    // somebody pasting a foreign library into the sheet -- and it has been
    // raised three times, each time once and with a reason: 80 000 -> 92 000
    // in display 2.4.0, 92 000 -> 96 000 and 96 000 -> 98 000 both in 2.5.0 (one
    // unreleased block, two findings).
    //
    // 2.4.0's reason: four planes, a phone profile, the light on the mark, a
    // line to type into, a chat tile and a weather window. 2.5.0's: the sheet
    // was cut to the description (`display-hive.md`) -- an arrangement per
    // output type with the height chain that lets a canvas scroll (§ 6.3), a
    // cap and an inner scroller for every open level (§ 5.9), the dot's four
    // dock states written as one condition (§ 6.8), the empty seat (§ 4.29),
    // the source on a chat line (§ 8.4), and `:hover` behind the profile's
    // inputs (§ 6.4). Those are rules that do something, and each carries the
    // thought it came from. A ceiling that has stopped buying bytes from dead
    // weight starts buying them from the comments -- the worst trade this
    // sheet can make, because it is the one place where the WHY has to survive
    // the next reader.
    //
    // The second raise's reason: the three findings of strand H5 -- the height cap of
    // § 5.9 counted the content box and not the window, and ignored the 2%
    // the two loud rungs lift by (B-09); a phone's leading window was only
    // given a floor, so a sibling with forty chat lines grew past it (B-11);
    // and a refused mark changed colour where § 5.4 says it dims (B-05). Three
    // rules, four tokens, and the measurements that produced them -- a defect
    // nobody can reproduce is a defect that comes back. The comments of the
    // first two were trimmed once before this line moved (OR-H5.8).
    //
    // The gzipped ceiling was left where it is, and IT is the one to watch:
    // 27 493 of 30 000 after the first raise, 28 245 after the second, so the next wave that
    // adds to this sheet trims before it raises.
    assert!(raw < 98_000, "the shell weighs {raw} bytes raw");
    // The air under the raw ceiling is a floor of its own from wave G on, and
    // not whatever happens to be left. Wave H2 left 43 bytes, which is another
    // way of saying the next strand to write a rule breaks the ceiling. Wave G
    // needs about 2 500 for the page block (`display-browser`, six classes,
    // and the night and fallback lines that follow them), so 3 000 stay free.
    //
    // Two things bought them, in this order, and the order is the lesson. The
    // sheet was first read rule by rule against the catalogue -- a class nobody
    // writes, an attribute a component cannot put on itself, a declaration said
    // twice, a token nothing reads -- and that found 704 bytes, which took the
    // air from 43 to 747. Everything else in this sheet that is not a comment
    // carries its weight.
    //
    // The rest came from asking what the raw ceiling is FOR. It guards what
    // travels, and what travelled was 47 kB of reasoning that only the file's
    // reader ever needed. Since OR-G0.1 the source keeps every comment and the
    // copies carry the rules plus the nameplate, so this number is the sheet
    // and not the arguing about it. The ceiling itself has not moved and will
    // not (OR-H2.7); the comments were not cut, they simply stopped commuting.
    assert!(
        98_000 - raw >= 3_000,
        "the shell leaves {} bytes of air under the raw ceiling; wave G needs 3 000 for the page",
        98_000 - raw
    );
    let Some(zipped) = zipped else {
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
