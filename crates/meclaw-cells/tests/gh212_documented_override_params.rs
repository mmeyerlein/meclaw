//! GH #212 — an `override_params` key a shipped doc writes down is one that
//! reaches a cell which reads the params underneath it.
//!
//! Sibling of `gh203_documented_port_addresses`, and the same failure shape one
//! field over. #140 made `override_params` **addressed** on a subtree template:
//! a key is a cell's path inside the template, and a key that names no cell is
//! refused pre-destructively. The seals then turned three former cells into
//! hives (`collector`, `session-keeper`, `summarizer`), and a hive path is still
//! a perfectly valid cell path — so `{"collector": {"memory_tier": "1"}}` is
//! ACCEPTED, and then read by a cell whose params block knows only `graph`,
//! `ports`, `required_drains` and `contract`. The mutation commits, the colony
//! comes up configured differently from what the recipe asked for, and nothing
//! anywhere says a word. That is precisely R10's original complaint — an
//! override that commits and does nothing — arriving through a door #140 did
//! not close, because #140 could not have known which cells would later become
//! hives.
//!
//! **The question is asked of the substrate, never of a second opinion.** Two
//! substrate calls decide every finding and neither is re-derived here:
//!
//! - `parse_subtree` reads the template off disk and says which of its cells
//!   are hives. A list of "the hives we know about" maintained in this file
//!   would agree with itself and drift from the tree, which is the state this
//!   defect lived in.
//! - `HiveParams` (`#[serde(deny_unknown_fields)]`) says whether a hive reads
//!   the params the recipe sets. If deserialisation fails on an unknown field,
//!   that field is by definition not consumed — the type IS the answer to
//!   "would this have any effect", so a recipe that sets `ports` on a hive is
//!   correctly left alone while one that sets `memory_tier` is not.
//!
//! **What is scanned, and why that is not prose-parsing.** Only a literal
//! `override_params: {…}` whose enclosing object also carries a literal
//! `template: "<ref>"` — the recipe names its own template, so there is nothing
//! to guess about which tree the keys are addressed into. Both the params
//! object and the template reference have to be real (`serde_json` parses the
//! one, the templates directory carries the other) or the occurrence is skipped.
//! A sentence ABOUT an override (`retuned with override_params on
//! collector/assemble.params.<name>`) stays out of scope for the same reason
//! gh203 leaves ports TABLE rows alone: it is not distinguishable from prose,
//! and a check that guessed there is a check people learn to work around.

use meclaw_colony::config::HiveParams;
use meclaw_colony::mutation::subtree::parse_subtree;
use meclaw_core::serde_json::{Map, Value};

fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

// ─────────────────────────────────────── what the shipped docs write down

/// One `override_params` object found in a shipped document, together with the
/// template reference the recipe around it names.
struct Recipe {
    file: String,
    line: usize,
    /// The `template` value exactly as written, e.g. `cogny@2.0.0`.
    template_ref: String,
    /// The parsed `override_params` object.
    params: Map<String, Value>,
}

/// Pull every `override_params` recipe out of one file.
///
/// Deliberately literal, byte-wise (a shipped README is full of `→` and `⋮`,
/// and slicing a `str` at an arbitrary offset would panic on the first one):
/// the key has to be `override_params` (optionally quoted) followed by `:` and
/// an object. The enclosing object is found by walking back to the `{` that
/// opened it, and its `template` value is read the same literal way.
fn recipes_in(file: &str, text: &str) -> Vec<Recipe> {
    let b = text.as_bytes();
    let needle = b"override_params";
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + needle.len() <= b.len() {
        if &b[i..i + needle.len()] != needle {
            i += 1;
            continue;
        }
        let key_start = i;
        let mut j = i + needle.len();
        if j < b.len() && b[j] == b'"' {
            j += 1;
        }
        while j < b.len() && b[j] == b' ' {
            j += 1;
        }
        if j >= b.len() || b[j] != b':' {
            i += 1;
            continue;
        }
        j += 1;
        while j < b.len() && (b[j] == b' ' || b[j] == b'\n') {
            j += 1;
        }
        let Some(value) = balanced_object(b, j) else {
            i += 1;
            continue;
        };
        // A value that is not JSON is a schematic (`"override_params": { ... }`
        // in the overview's schema listing), not a recipe. Skipped, not judged.
        let Ok(Value::Object(params)) = meclaw_core::serde_json::from_str::<Value>(&text[j..value])
        else {
            i += 1;
            continue;
        };
        if let Some(template_ref) = enclosing_template(text, key_start) {
            out.push(Recipe {
                file: file.to_string(),
                line: text[..key_start].matches('\n').count() + 1,
                template_ref,
                params,
            });
        }
        i = value.max(i + 1);
    }
    out
}

/// End offset (exclusive) of the balanced `{…}` that starts at `from`, or
/// `None` if `from` is not a `{` or the object never closes. Braces inside
/// strings are honoured, because a `script_inline` param is full of them.
fn balanced_object(b: &[u8], from: usize) -> Option<usize> {
    if from >= b.len() || b[from] != b'{' {
        return None;
    }
    let mut depth = 0usize;
    let mut in_str = false;
    let mut esc = false;
    for (k, &c) in b.iter().enumerate().skip(from) {
        if in_str {
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == b'"' {
                in_str = false;
            }
            continue;
        }
        match c {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(k + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// The `template` value of the object that encloses `at`.
///
/// Walks back to the `{` that opened the enclosing object, then reads the first
/// literal `template: "<ref>"` inside it. An `override_params` with no template
/// beside it is not a recipe this check can place, and is skipped.
fn enclosing_template(text: &str, at: usize) -> Option<String> {
    let b = text.as_bytes();
    let mut depth = 0i32;
    let mut k = at;
    let open = loop {
        if k == 0 {
            return None;
        }
        k -= 1;
        match b[k] {
            b'}' => depth += 1,
            b'{' => {
                if depth == 0 {
                    break k;
                }
                depth -= 1;
            }
            _ => {}
        }
    };
    let end = balanced_object(b, open)?;
    let scope = &text[open..end];
    let idx = scope.find("template")?;
    let rest = scope[idx + "template".len()..].trim_start();
    let rest = rest.strip_prefix('"').unwrap_or(rest).trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let stop = rest.find('"')?;
    Some(rest[..stop].to_string())
}

/// Every shipped `.md` and `.json` under `templates/`, `examples/` and `docs/`.
///
/// `builder-librarian`'s seed is excluded (a GENERATED corpus embedding other
/// files verbatim — a finding there duplicates the finding in the source,
/// reported against a file nobody edits by hand), and so is `docs/archive`,
/// which is a record of what used to be true.
fn shipped_docs() -> Vec<(String, String)> {
    let root = core_root();
    let mut out = Vec::new();
    for base in ["templates", "examples", "docs"] {
        collect_docs(&root, &root.join(base), &mut out);
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn collect_docs(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let entry = entry.unwrap();
        let p = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if name != "builder-librarian" && name != "archive" {
                collect_docs(root, &p, out);
            }
            continue;
        }
        if !(name.ends_with(".md") || name.ends_with(".json")) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        out.push((p.strip_prefix(root).unwrap().display().to_string(), text));
    }
}

// ─────────────────────────────────────────────────────────────── the check

/// Every documented `override_params` key that lands on a hive which does not
/// read what is set there, as one sentence each. `resolved` counts the recipes
/// that were actually placed against a template on disk, so a caller can tell
/// "nothing wrong" from "nothing read".
fn findings(docs: &[(String, String)], resolved: &mut usize) -> Vec<String> {
    let templates = core_root().join("templates");
    let mut out = Vec::new();
    for (file, text) in docs {
        for r in recipes_in(file, text) {
            // `name` or `name@version` — the version picks a registry entry, not
            // a different directory, and a ref naming nothing the tree carries
            // is skipped rather than judged (the same conservatism the boundary
            // itself follows).
            let name = r.template_ref.split('@').next().unwrap_or_default();
            let dir = templates.join(name);
            if !dir.join("config.json").is_file() {
                continue;
            }
            let Ok(parsed) = parse_subtree(&dir) else {
                continue;
            };
            if parsed.cells.len() <= 1 {
                continue; // single-cell template: the flat form, no addressing
            }
            *resolved += 1;
            for (key, value) in &r.params {
                if !parsed.hives.contains(key) {
                    continue;
                }
                // The hive's own params type decides. An unknown field is, by
                // the type's `deny_unknown_fields`, one the hive does not read.
                let Err(why) = meclaw_core::serde_json::from_value::<HiveParams>(value.clone())
                else {
                    continue; // `graph`/`ports`/`contract` — set on a hive on purpose
                };
                let shown = if key.is_empty() {
                    "\"\" (the subtree root)".to_string()
                } else {
                    format!("'{key}'")
                };
                let cells: Vec<&str> = parsed
                    .cells
                    .iter()
                    .map(|c| c.rel_path.as_str())
                    .filter(|p| !p.is_empty() && !parsed.hives.iter().any(|h| h == p))
                    .collect();
                out.push(format!(
                    "{}:{}: override_params[{shown}] on template '{}' addresses a HIVE, and a \
                     hive reads only graph/ports/required_drains/contract ({why}). The key is \
                     accepted, the params are never read, and the instance comes up \
                     unconfigured. Address the cell that reads them; this template's non-hive \
                     cells are: {}.",
                    r.file,
                    r.line,
                    r.template_ref,
                    cells.join(", ")
                ));
            }
        }
    }
    out
}

#[test]
fn every_documented_override_params_key_reaches_a_cell_that_reads_it() {
    let mut resolved = 0usize;
    let found = findings(&shipped_docs(), &mut resolved);
    assert!(
        found.is_empty(),
        "shipped documentation configures a hive that reads nothing:\n  {}",
        found.join("\n  ")
    );
    // "Nothing wrong" and "nothing read" look identical from the outside, and
    // the second is the failure mode of a scan this conservative.
    assert!(
        resolved >= 2,
        "the scan placed almost no recipe against a template: {resolved}"
    );
}

/// The test of the test, and the reason the sweep above may stay silent.
///
/// A correct tree contains few `override_params` recipes at all, so the sweep
/// resolves a handful and would be green whether it worked or not. This feeds
/// it `templates/cogny/README.md`'s recipe verbatim from the state it shipped
/// in, and requires it to be reported, to name the hive, and to say which cells
/// would have worked.
#[test]
fn the_scan_reports_the_recipe_this_issue_was_filed_about() {
    let shipped_before_the_fix = [(
        "templates/cogny/README.md".to_string(),
        r#"{"op": "instantiate", "template": "cogny@2.0.0", "at": "/cores/deep",
 "override_params": {"collector": {"memory_tier": "1",
                                            "context_window": 200000,
                                            "recoverability": "lookup:repeatable,write:env"}}}"#
            .to_string(),
    )];
    let mut resolved = 0usize;
    let found = findings(&shipped_before_the_fix, &mut resolved);
    assert_eq!(
        found.len(),
        1,
        "the scan missed the known-bad key (or invented one): {found:#?}"
    );
    assert!(
        found[0].contains("override_params['collector']") && found[0].contains("cogny@2.0.0"),
        "the finding does not name the key and the template: {}",
        found[0]
    );
    assert!(
        found[0].contains("collector/assemble"),
        "the finding does not offer the cell that reads the params: {}",
        found[0]
    );
}

/// A key that sets a hive's OWN params is legitimate and must stay silent —
/// otherwise the check would push people away from the one thing addressing a
/// hive is good for.
#[test]
fn a_key_that_sets_what_a_hive_actually_reads_is_not_a_finding() {
    let deliberate = [(
        "synthetic".to_string(),
        r#"{"name": "core", "template": "cogny",
            "override_params": {"collector": {"ports": ["assemble"]}}}"#
            .to_string(),
    )];
    let mut resolved = 0usize;
    let found = findings(&deliberate, &mut resolved);
    assert!(found.is_empty(), "a hive param was flagged: {found:#?}");
    assert_eq!(resolved, 1, "the synthetic recipe was not placed at all");
}
