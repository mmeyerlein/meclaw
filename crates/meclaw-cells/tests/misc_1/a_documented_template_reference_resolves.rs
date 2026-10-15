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
//! **The plan archive is out of scope.** `docs/superpowers/plans/` is not
//! scanned. By the project's authority hierarchy a plan is historical and
//! non-authoritative — it records what was true while its wave ran. Holding a
//! frozen plan to today's version numbers made every renumbering edit somebody
//! else's archived plan, and the waves W1–W5 did exactly that: history
//! falsification, not conformance (W5 receipt; owner ruling 2026-08-23 R7).
//! Those edits stand — from here the archive freezes. The gate therefore
//! covers `templates/`, `examples/` and living `docs/` prose only. The one
//! plan that must stay true is the one about to be driven, and it is kept true
//! by the re-baseline sweep at the end of every wave (the wave meta-plan,
//! private tree, § „Folgewelle re-baselinen") — by a human-run pass over the
//! next plan, not by a test pulling the whole archive forward.
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

/// The archived wave plans — frozen history, never scanned (see the header).
const FROZEN_SUBTREE: &str = "docs/superpowers/plans";

/// Every `.md` and `.json` under the three document roots, minus the frozen
/// plan archive.
fn documents(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let frozen = root.join(FROZEN_SUBTREE);
    for base in DOC_ROOTS {
        collect_docs(&root.join(base), &frozen, &mut out);
    }
    out.sort();
    out
}

fn collect_docs(
    dir: &std::path::Path,
    frozen: &std::path::Path,
    out: &mut Vec<std::path::PathBuf>,
) {
    if dir == frozen {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_docs(&p, frozen, out);
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

/// The archived wave plans are out of the sweep, and stay out.
///
/// See the header: `plans/` is historical by the project's authority
/// hierarchy (its lowest level: historical, non-authoritative), and a gate
/// that forced a frozen plan to name a current version was rewriting history,
/// not checking it. The scan therefore has to come back from
/// `docs/superpowers/plans/` empty-handed — an occurrence there is a record of
/// what was true when the wave ran.
///
/// **Two trees, and only one of them has an archive.** The export carries
/// `crates/` wholesale but pulls `docs/` through an explicit map that lists no
/// `docs/superpowers/*`, so this file ships into a clone where the archive does
/// not exist; demanding it there would make this gate red on a subset, and a
/// subset is not a defect. The private tree is recognised the same way the
/// floor below recognises it — by root `plans/`, the directory that never
/// travels.
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

    let leaked: Vec<String> = documents(&root)
        .into_iter()
        .filter(|p| p.starts_with(&archive))
        .map(|p| p.strip_prefix(&root).unwrap_or(&p).display().to_string())
        .collect();

    assert!(
        leaked.is_empty(),
        "the sweep still collects archived wave plans, so a frozen plan can be \
         made to fail over a version that was current when it was written:\n  {}",
        leaked.join("\n  ")
    );
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
