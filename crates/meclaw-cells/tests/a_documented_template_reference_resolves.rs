//! A `"template": "name@version"` in a shipped document is an instruction, and
//! the registry has to be able to carry it out.
//!
//! `templates/channel/README.md` offered two copy-paste mutations naming
//! `talky@3.1.0`, and `templates/cogny/README.md` an `instantiate` naming
//! `cogny@2.0.0`. Neither version is on disk: `talky` never had a `3.1.0` at
//! all, and the `3.1.0` those documents were written against was renumbered to
//! `3.0.1` when the wave of 2026-08-19 put repairs back on the third digit. The
//! prose moved with neither. A reader who pastes such a block gets
//! `TemplateMissing` for a template that is sitting right there, which is the
//! same failure `gh221_shipped_template_versions` guards from the declaring end
//! — this file guards it from the referencing end.
//!
//! # Why this is not prose-parsing
//!
//! Only the literal key `"template"` with a `name@version` value is read. That
//! token names a template to instantiate wherever it appears — in `add_nodes`,
//! in `swap_nodes[].with`, in an `instantiate` op — so an occurrence in a
//! shipped document IS an instruction and there is nothing to guess about it.
//! Prose ABOUT a version ("the `collector@1.2.0` migration") carries no such
//! key and is deliberately out of scope: naming a past version in a sentence is
//! history, and a check that guessed there would be one people learn to work
//! around. Same line `gh203_documented_port_addresses` draws for endpoints.
//!
//! **The question is asked of the substrate.** `parse_template_json` reads each
//! descriptor, `TemplatesRegistry::resolve` answers each reference — the same
//! two calls a mutation goes through. A check that compared version strings
//! itself could agree with itself while disagreeing with the colony.
//!
//! **Conservative by construction.** A reference whose NAME the tree does not
//! carry is skipped rather than judged: the library ships in two sizes (this
//! tree has every template, the published one a subset), so a document may name
//! a template that is legitimately absent from the checkout in hand. What is
//! judged is the case where the name resolves and the version does not — a
//! template that IS here under a version nobody can ask for.

use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, parse_template_json};

fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Where a shipped document may live. Same three roots `gh203` scans.
const DOC_ROOTS: [&str; 3] = ["templates", "examples", "docs"];

/// The registry a mutation would consult, built from every descriptor on disk.
fn shipped_registry(root: &std::path::Path) -> TemplatesRegistry {
    let mut files = Vec::new();
    collect(&root.join("templates"), "template.json", &mut files);
    files.sort();
    TemplatesRegistry::from_entries(
        files
            .iter()
            .filter_map(|p| parse_template_json(p).ok())
            .map(|s| TemplateEntry {
                template_id: format!("doc-ref:{}", s.filesystem_path.display()),
                name: s.name,
                version: s.version,
                filesystem_path: s.filesystem_path,
            })
            .collect(),
    )
}

fn collect(dir: &std::path::Path, name: &str, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect(&p, name, out);
        } else if entry.file_name() == name {
            out.push(p);
        }
    }
}

/// Every `.md` and `.json` under the three document roots.
fn documents(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for base in DOC_ROOTS {
        collect_docs(&root.join(base), &mut out);
    }
    out.sort();
    out
}

fn collect_docs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_docs(&p, out);
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".md") || name.ends_with(".json") {
            out.push(p);
        }
    }
}

/// Every `"template": "<value>"` in `text`, in order.
///
/// Written as a scan rather than a JSON parse on purpose: most of these live in
/// fenced examples inside Markdown, and half of the `.json` hits sit inside an
/// escaped string in a `description` slot. The key spelling is what identifies
/// them, in both shapes.
fn template_references(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("\"template\"") {
        rest = &rest[at + "\"template\"".len()..];
        // `:` then whitespace then the opening quote, tolerating the backslash
        // an escaped block puts in front of it.
        let Some(colon) = rest.find(':') else { break };
        let after: &str = rest[colon + 1..].trim_start();
        let after = after.strip_prefix('\\').unwrap_or(after);
        let Some(body) = after.strip_prefix('"') else {
            continue;
        };
        let end = body.find(['"', '\\']);
        if let Some(end) = end {
            out.push(body[..end].to_string());
        }
    }
    out
}

#[test]
fn every_documented_template_reference_resolves_in_the_registry() {
    let root = core_root();
    let registry = shipped_registry(&root);
    let mut checked = 0usize;
    let mut findings: Vec<String> = Vec::new();

    for doc in documents(&root) {
        let Ok(text) = std::fs::read_to_string(&doc) else {
            continue;
        };
        let shown = doc
            .strip_prefix(&root)
            .unwrap_or(&doc)
            .display()
            .to_string();
        for reference in template_references(&text) {
            let Some((name, _version)) = reference.split_once('@') else {
                // An unversioned reference asks for whatever ships; that is a
                // different question and `resolve` answers it by design.
                continue;
            };
            // Not in this checkout — see the header. Never judged.
            if registry.resolve(name).is_err() {
                continue;
            }
            checked += 1;
            if let Err(e) = registry.resolve(&reference) {
                findings.push(format!(
                    "{shown}: instantiates `{reference}`, which the registry cannot resolve \
                     ({e}) — `{name}` ships under a different version, so a reader who pastes \
                     this block gets an error for a template that is on disk."
                ));
            }
        }
    }

    assert!(
        findings.is_empty(),
        "shipped documents instantiate template versions that do not ship:\n  {}",
        findings.join("\n  ")
    );
    // "Nothing wrong" and "nothing looked at" are the same green from outside,
    // so the sweep carries a floor. It differs per tree, and it is read off the
    // tree rather than declared: the published subset ships fewer templates and
    // therefore fewer recipes, and `plans/` is the directory that never travels
    // (the same marker `gh80_shipped_conditions_are_guarded` uses, because a
    // marker on an allow-list can be promoted and a forbidden prefix cannot).
    let private_tree = root.join("plans").is_dir();
    let floor = if private_tree { 5 } else { 1 };
    assert!(
        checked >= floor,
        "the sweep read {checked} versioned template references, below the floor of {floor} for \
         this tree — it is meant to read the instantiation examples the library ships, so a \
         near-zero count means the scan stopped seeing them, not that the library stopped \
         having them"
    );
}
