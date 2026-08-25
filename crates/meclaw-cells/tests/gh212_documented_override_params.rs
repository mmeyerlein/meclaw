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
//! **Since GH #294 this file no longer stands in for a missing declaration.**
//! The param half is checked by the substrate — a mutation whose
//! `override_params` names a param the addressed cell does not have is refused,
//! for every cell and not just for a hive
//! (`crates/meclaw-colony/tests/gh294_an_override_names_a_param_that_exists.rs`,
//! ruling Q6). What was inferred here from `HiveParams`'
//! `#[serde(deny_unknown_fields)]` — a hive-shaped guess at "would this have
//! any effect" — is gone with the test that guarded it. A shipped DOCUMENT is
//! still never executed, so the sweep stays: it asks the substrate's own
//! question of every recipe a reader could copy.
//!
//! **The question is asked of the substrate, never of a second opinion.** Two
//! substrate calls decide every finding and neither is re-derived here:
//!
//! - `parse_subtree` reads the template off disk and says which cells it has.
//!   A list of "the cells we know about" maintained in this file would agree
//!   with itself and drift from the tree, which is the state this defect lived
//!   in.
//! - `check_override_params` is the very function the mutation validator calls.
//!   A recipe this file passes is a recipe a colony accepts, by construction.
//!
//! **The plan archive is out of scope (owner ruling 2026-08-23 R7).**
//! `docs/superpowers/plans/` is not scanned. By the project's authority
//! hierarchy a plan is historical and non-authoritative — it records what was
//! true while its wave ran, the same reason `docs/archive/` was already carved
//! out. Holding a frozen plan to today's templates and cells made every
//! renumbering and every seal edit somebody else's archived plan: history
//! falsification, not conformance. Those edits stand; from here the archive
//! freezes. The one plan that must stay true is the one about to be driven,
//! and it is kept true by the re-baseline step at the end of every wave (the
//! wave meta-plan, private tree, § „Folgewelle re-baselinen"), not by a gate
//! pulling the whole archive forward.
//! The ruling was written for the template-reference gate
//! (`a_documented_template_reference_resolves`) and extended to this one by
//! controller ruling, because the reason it gives is about `plans/`, not about
//! that gate.
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

use meclaw_colony::mutation::subtree::{check_override_params, parse_subtree};
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use meclaw_core::serde_json::{Map, Value};

fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The shipped `templates/` directory as a registry snapshot — what a real
/// mutation resolves a `cell.type: "ref"` sub-unit against (GH #277).
///
/// Since `talky` and `cogny` reference their sub-units instead of carrying
/// copies, an empty snapshot would make [`parse_subtree`] fail on exactly the
/// two composites this check exists for, and a failed parse is SKIPPED here —
/// the sweep would go quietly vacuous on its main subjects. So the scan reads
/// the same library the colony reads.
fn shipped_registry() -> TemplatesRegistry {
    let scanned = scan_templates_dir(&core_root().join("templates")).unwrap_or_default();
    TemplatesRegistry::from_entries(
        scanned
            .into_iter()
            .map(|s| TemplateEntry {
                // The registry key a reference resolves by is `name@version`;
                // the surrogate id is never read back.
                template_id: format!("scan-{}", s.name),
                name: s.name,
                version: s.version,
                filesystem_path: s.filesystem_path,
            })
            .collect(),
    )
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

/// The archived wave plans — frozen history, never scanned (see the header).
const FROZEN_SUBTREE: &str = "docs/superpowers/plans";

/// Every shipped `.md` and `.json` under `templates/`, `examples/` and `docs/`.
///
/// `builder-librarian`'s seed is excluded (a GENERATED corpus embedding other
/// files verbatim — a finding there duplicates the finding in the source,
/// reported against a file nobody edits by hand), and so is `docs/archive`,
/// which is a record of what used to be true. The wave-plan archive
/// [`FROZEN_SUBTREE`] is excluded for that same second reason (R7 — see the
/// header).
fn shipped_docs() -> Vec<(String, String)> {
    let root = core_root();
    let frozen = root.join(FROZEN_SUBTREE);
    let mut out = Vec::new();
    for base in ["templates", "examples", "docs"] {
        collect_docs(&root, &frozen, &root.join(base), &mut out);
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn collect_docs(
    root: &std::path::Path,
    frozen: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<(String, String)>,
) {
    if dir == frozen {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let entry = entry.unwrap();
        let p = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if name != "builder-librarian" && name != "archive" {
                collect_docs(root, frozen, &p, out);
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

/// Every documented `override_params` key the substrate would refuse, as one
/// sentence each. `resolved` counts the recipes that were actually placed
/// against a template on disk, so a caller can tell "nothing wrong" from
/// "nothing read".
fn findings(docs: &[(String, String)], resolved: &mut usize) -> Vec<String> {
    let templates = core_root().join("templates");
    let registry = shipped_registry();
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
            // GH #277: the registry is what resolves a `cell.type: "ref"`
            // sub-unit, and `talky`/`cogny` declare theirs — so the tree parsed
            // here is the tree the mutation would produce, sub-units included.
            let Ok(parsed) = parse_subtree(&dir, &registry) else {
                continue;
            };
            if parsed.cells.len() <= 1 {
                continue; // single-cell template: the flat form, no addressing
            }
            *resolved += 1;
            for (key, value) in &r.params {
                // A key that names no cell at all is the GH #140 half, refused
                // by the same validator; this file's subject is the recipe that
                // places itself correctly and sets the wrong thing.
                let Some(cell) = parsed.cells.iter().find(|c| &c.rel_path == key) else {
                    continue;
                };
                // GH #294: the substrate's own answer, not a second opinion.
                let Err(why) = check_override_params(cell, Some(key), &r.template_ref, value)
                else {
                    continue; // every key names a param the cell actually has
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
                    "{}:{}: override_params[{shown}] on template '{}' sets what the cell it \
                     addresses does not have ({why:?}). A colony refuses this mutation, so the \
                     recipe cannot be copied. Address the cell that reads them; this template's \
                     non-hive cells are: {}.",
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

/// The archived wave plans are out of the sweep, and stay out.
///
/// See the header: `docs/superpowers/plans/` is historical by the authority
/// hierarchy, and a gate that held a frozen plan to today's templates was
/// rewriting history rather than checking it. The scan therefore has to come
/// back from there empty-handed.
///
/// **Two trees, and only one of them has an archive.** The export carries
/// `crates/` wholesale but pulls `docs/` through an explicit map that lists no
/// `docs/superpowers/*`, so this file ships into a clone where the archive does
/// not exist — and a missing directory is a subset, not a defect (the rule
/// `shipped_docs` above already follows). The private tree is recognised by
/// root `plans/`, the directory that never travels (the marker
/// `gh80_shipped_conditions_are_guarded` uses, because a marker on an
/// allow-list can be promoted and a forbidden prefix cannot).
#[test]
fn the_plan_archive_is_frozen_history() {
    let root = core_root();
    if !root.join("plans").is_dir() {
        return; // public tree: no archive, nothing to leak
    }
    let archive = root.join(FROZEN_SUBTREE);
    assert!(
        archive.is_dir(),
        "expected the wave-plan archive at {} — if it moved, this test has to \
         follow it rather than pass by absence",
        archive.display()
    );
    let prefix = format!("{FROZEN_SUBTREE}/");
    let leaked: Vec<String> = shipped_docs()
        .into_iter()
        .map(|(f, _)| f)
        .filter(|f| f.starts_with(&prefix))
        .collect();
    assert!(
        leaked.is_empty(),
        "the sweep still collects archived wave plans, so a frozen plan can be \
         made to fail over a recipe that was current when it was written:\n  {}",
        leaked.join("\n  ")
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

// GH #294 retired `a_key_that_sets_what_a_hive_actually_reads_is_not_a_finding`
// together with the `HiveParams` inference it guarded. The property it held —
// a key that sets what the addressed cell DOES declare must stay silent — is
// now held for every shipped template and every one of their cells at once, by
// `gh294_an_override_names_a_param_that_exists.rs`'s
// `every_shipped_template_instantiates_unchanged`.
