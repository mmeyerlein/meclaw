//! The template catalogue publishes what a template DEMANDS, not only what it is for.
//!
//! Measured, not supposed. With the mutation grammar in the briefing head, a
//! hosted model walked S12 and encoded every `add_nodes` entry correctly —
//! `name` plus `template`, three times wrong before, right now. The door refused
//! it one level higher (the S12 build runs, `CHANGELOG.md` § 0.26.0):
//!
//! ```text
//! cogny: ctx key "model" is required — the model the brain infers with
//! cogny: ctx key "model_fast" is required — the lookup lane's model
//! ```
//!
//! The model had picked templates out of the librarian's catalogue by name and
//! instantiated them with an empty `ctx`. It could not have done otherwise: a
//! catalogue row is `json.dumps(template.json)`, a `template.json` is serialised
//! description-first, and the retrieving cell hands the model
//! `row["text"][:1200]` (`templates/builder-librarian/retrieve/config.json`).
//! For `cogny@4.0.3` that meant 3760 characters of description in the base row
//! and the `requires` block not in that row at all — it began 467 characters
//! into the first CONTINUATION row. The contract existed, was enforced, and was
//! unreadable from the surface the choice is made on.
//!
//! So every catalogue row now LEADS with one line naming the keys an
//! instantiation owes, and this gate holds it there:
//!
//!   1. every template row opens with it, including the ones that owe nothing —
//!      "requires no ctx and no env key" is an answer, an absent line is not;
//!   2. it lies inside the retriever's own truncation, because a contract only
//!      legible past the cut is not published;
//!   3. it names every key the mutation door would refuse for, derived from the
//!      tree on each run rather than pinned to a list that ages;
//!   4. it spans the `ref`s — a composite owes what its parts declare
//!      (`requires_for_reference`, GH #292/#347), and a line built from the
//!      outer descriptor alone would under-report.
//!
//! **R2b guard.** Where the corpus or the catalogue does not ship, this skips
//! rather than fails on a dead reference.

use meclaw_core::serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../")
}

const CORPUS: &str = "templates/builder-librarian/store/seed/docs.jsonl";

/// The retrieving cell's cut, in characters. Mirrored rather than imported —
/// the point of this gate is to go red if the two ever disagree about how much
/// of a row a model actually sees.
const TRUNCATION: usize = 1200;

const LEAD: &str = "CONTRACT — instantiating ";

/// Every `kind == "template"` BASE row, by `section` (the catalogue name).
///
/// Continuations are excluded on purpose: the lead line belongs to the row a
/// retrieval can rank on its own, and a continuation carries the tail of a
/// descriptor, not a heading.
fn catalogue_rows() -> Option<BTreeMap<String, String>> {
    let text = std::fs::read_to_string(repo_root().join(CORPUS)).ok()?;
    let mut out = BTreeMap::new();
    for line in text.lines().skip(1) {
        let row: Value = meclaw_core::serde_json::from_str(line).ok()?;
        if row["kind"].as_str() != Some("template") {
            continue;
        }
        let id = row["id"].as_str().unwrap_or_default();
        if id.contains("-cont") {
            continue;
        }
        out.insert(
            row["section"].as_str().unwrap_or_default().to_string(),
            row["text"].as_str().unwrap_or_default().to_string(),
        );
    }
    (!out.is_empty()).then_some(out)
}

/// The `requires.ctx` keys one `template.json` declares as required.
///
/// A declared key WITHOUT `required` is required (`docs/meclaw-overview.md`
/// § `requires`), which is why the default is `true` — the same default
/// `check_declared_requirements` reads.
fn declared_ctx_keys(path: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(blob) = meclaw_core::serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    let Some(map) = blob["requires"]["ctx"].as_object() else {
        return Vec::new();
    };
    map.iter()
        .filter(|(_, decl)| decl["required"].as_bool().unwrap_or(true))
        .map(|(key, _)| key.clone())
        .collect()
}

/// `(catalogue name, template.json path)` for every shipped template.
fn shipped_templates() -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(repo_root().join("templates")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let descriptor = entry.path().join("template.json");
        if descriptor.is_file() {
            out.push((entry.file_name().to_string_lossy().to_string(), descriptor));
        }
    }
    out.sort();
    out
}

#[test]
fn every_template_row_opens_with_its_contract() {
    let Some(rows) = catalogue_rows() else {
        eprintln!("skip: no catalogue rows in {CORPUS}");
        return;
    };
    for (name, text) in &rows {
        assert!(
            text.starts_with(LEAD),
            "the catalogue row for `{name}` does not open with its contract. A \
             row that says only what a template is FOR is the surface a model \
             picked `cogny` from and was refused at the door for"
        );
    }
}

#[test]
fn the_contract_survives_the_retrievers_truncation() {
    let Some(rows) = catalogue_rows() else {
        eprintln!("skip: no catalogue rows in {CORPUS}");
        return;
    };
    for (name, text) in &rows {
        let head: String = text.chars().take(TRUNCATION).collect();
        let line = head.lines().next().unwrap_or_default();
        assert!(
            line.starts_with(LEAD) && line.ends_with('.'),
            "the contract of `{name}` does not fit whole inside the {TRUNCATION} \
             characters the retrieve cell hands over — half a contract names \
             half the keys, which is the failure with extra steps"
        );
    }
}

#[test]
fn the_contract_names_every_key_the_door_would_refuse_for() {
    let Some(rows) = catalogue_rows() else {
        eprintln!("skip: no catalogue rows in {CORPUS}");
        return;
    };
    let templates = shipped_templates();
    assert!(
        !templates.is_empty(),
        "no templates to check the catalogue against"
    );
    let mut checked = 0;
    for (name, descriptor) in &templates {
        let Some(text) = rows.get(name) else { continue };
        for key in declared_ctx_keys(descriptor) {
            checked += 1;
            assert!(
                text.lines().next().unwrap_or_default().contains(&key),
                "`{name}` declares ctx key `{key}` as required and its catalogue \
                 row never says so; the model that names this template will not \
                 pass it, and the mutation is refused with requirement_missing"
            );
        }
    }
    assert!(
        checked > 0,
        "no shipped template declares a required ctx key any more — this gate \
         is measuring nothing, and the reason has to be looked at before it is \
         deleted"
    );
}

#[test]
fn a_template_owing_nothing_says_so_rather_than_saying_nothing() {
    let Some(rows) = catalogue_rows() else {
        eprintln!("skip: no catalogue rows in {CORPUS}");
        return;
    };
    let silent = rows
        .values()
        .filter(|t| {
            t.lines()
                .next()
                .unwrap_or_default()
                .contains("requires no ctx and no env key")
        })
        .count();
    assert!(
        silent > 0,
        "not one catalogue row states an EMPTY contract. A model cannot tell \
         'this template asks for nothing' from 'nobody wrote the line' unless \
         the empty case is written out"
    );
}

#[test]
fn the_contract_spans_the_refs_it_is_built_from() {
    let Some(rows) = catalogue_rows() else {
        eprintln!("skip: no catalogue rows in {CORPUS}");
        return;
    };
    // A composite is refused for what its PARTS declare (`requires_for_reference`
    // walks every `ref` hop). So somewhere in the catalogue there must be a row
    // naming a ctx key its own descriptor does not declare — otherwise the line
    // was built from the outer `template.json` alone and under-reports exactly
    // the composites a builder reaches for first.
    //
    // GH #516 — the question is INHERITANCE, not silence, and the two used to be
    // the same sentence here because `assistant` was the one composite declaring
    // nothing at all. It declares `model_surface` now (a level's own model key,
    // which no ref can own), and an "own declaration is empty" proxy would read
    // that as the union having disappeared. So the row is asked what it actually
    // has to prove: a key it NAMES that its own descriptor does not DECLARE.
    let inherited = shipped_templates().into_iter().any(|(name, descriptor)| {
        let Some(text) = rows.get(&name) else {
            return false;
        };
        let lead = text.lines().next().unwrap_or_default();
        let Some((_, listed)) = lead.split_once("ctx keys:") else {
            return false;
        };
        let declared = declared_ctx_keys(&descriptor);
        listed
            .split(" — ")
            .next()
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .any(|key| !declared.iter().any(|d| d == key))
    });
    assert!(
        inherited,
        "no catalogue row carries a requirement it inherited through a `ref`. \
         The door enforces the union across refs; a catalogue that reports only \
         the outer declaration tells a builder a composite is free when it is not"
    );
}
