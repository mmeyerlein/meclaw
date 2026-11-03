//! GH #292 — a template's `requires.ctx` block is DERIVED from the template,
//! never duplicated beside it.
//!
//! Task 13 gave `template.json` a place to say which `${ctx.X}` keys an
//! instantiation must supply. A place to say it is also a place to say it
//! WRONG: a declaration that is written once and then never re-read drifts the
//! moment somebody adds a placeholder — and the failure mode of a stale
//! declaration is worse than none at all, because a builder that reads it is
//! confidently wrong instead of merely uninformed. So the declaration is gated
//! in BOTH directions, and neither direction alone would do:
//!
//! - `every_ctx_key_a_template_uses_is_declared` — a placeholder added to a
//!   config value without a matching entry is a template that still rejects a
//!   mutation for a key it never advertised.
//! - `every_declared_ctx_key_is_used` — an entry left behind by a placeholder
//!   that was removed or renamed is a leaflet: it asks a builder for a value
//!   nothing consumes, and nothing would ever notice.
//!
//! # What is read, and what deliberately is not
//!
//! **Config VALUES only.** Template prose carries illustrative `${VAR}` and
//! `${ctx.model}` placeholders — `templates/cogny/template.json` explains
//! the K-H2 model convention by quoting the token, and
//! `templates/cogny/README.md` does the same. Those are documentation about
//! a placeholder, not a placeholder, and a sweep that read them would demand a
//! declaration for every example anybody ever writes down. The same rule keeps
//! `templates/builder-librarian/store/seed/docs.jsonl` — a GENERATED corpus
//! that quotes other templates' descriptors verbatim — out of scope by
//! construction rather than by an exclusion list that could go stale.
//!
//! **A template's OWN directory.** `talky` and `cogny` name their sub-units
//! with `cell.type: "ref"` since GH #277; the referenced template declares its
//! own requirements, and the union across refs is enforced where a ref is
//! actually resolved — at mutation-validation time. Here a ref marker is just
//! another `config.json` with no `${ctx.*}` in it. What a composite declares is
//! therefore what the composite itself uses, and staying inside the directory
//! is what makes the two directions provable at all.
//!
//! **The question is asked of the substrate, never of a second opinion.** Same
//! discipline as `gh212_documented_override_params` and
//! `gh221_shipped_template_versions`: the keys come out of
//! `mutation::substitute::collect_ctx_keys`, which scans with the very function
//! (`expand_with`) that resolves them at instantiation, and the declaration
//! comes out of `templates::read_requires`, which is the reader the enforcement
//! point uses. A test with a regex of its own would be free to disagree with
//! both, which is exactly the state this gate exists to prevent.

use meclaw_colony::mutation::substitute::collect_ctx_keys;
use meclaw_colony::templates::read_requires;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn core_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every directory under `root` that carries a `template.json`, sorted.
///
/// The same rule `scan_templates_dir` uses for what a template IS (and the same
/// walk `gh221_shipped_template_versions` does for the same reason): the whole
/// subtree, because `templates/_cell-types/*/` carries descriptors one level
/// deeper than the rest.
fn template_dirs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_template_dirs(root, &mut out);
    out.sort();
    out
}

fn collect_template_dirs(dir: &Path, out: &mut Vec<PathBuf>) {
    if dir.join("template.json").is_file() {
        out.push(dir.to_path_buf());
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_template_dirs(&p, out);
        }
    }
}

/// Every `config.json` that belongs to the template rooted at `dir`.
///
/// The walk stops at a nested `template.json`: a directory that carries a
/// descriptor of its own is a template of its own and owns its own declaration,
/// so counting its placeholders against the parent would make one template
/// answer for another's parameter surface.
fn own_config_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_own_configs(dir, dir, &mut out);
    out.sort();
    out
}

fn collect_own_configs(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
    if dir != root && dir.join("template.json").is_file() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_own_configs(&p, root, out);
        } else if entry.file_name() == "config.json" {
            out.push(p);
        }
    }
}

/// The `${ctx.*}` keys the template rooted at `dir` uses in its own config
/// values, and the file each was first seen in (for the failure message).
fn used_ctx_keys(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    for path in own_config_files(dir) {
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let val: meclaw_core::serde_json::Value = meclaw_core::serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("parse {} as JSON: {e}", path.display()));
        let keys = collect_ctx_keys(&val)
            .unwrap_or_else(|e| panic!("scan {} for ctx placeholders: {e:?}", path.display()));
        for k in keys {
            if !out.iter().any(|(seen, _)| *seen == k) {
                out.push((k, path.clone()));
            }
        }
    }
    out.sort();
    out
}

/// The declared `ctx` keys of the template rooted at `dir`.
fn declared_ctx_keys(dir: &Path) -> BTreeSet<String> {
    read_requires(dir)
        .unwrap_or_else(|e| panic!("read the requires block of {}: {e}", dir.display()))
        .ctx
        .into_keys()
        .collect()
}

/// A template's display name for a message: its path under `templates/`.
fn shown(root: &Path, dir: &Path) -> String {
    dir.strip_prefix(root)
        .unwrap_or(dir)
        .display()
        .to_string()
        .replace('\\', "/")
}

/// Direction A — every used key is declared. One sentence per (template, key).
fn undeclared(root: &Path, swept: &mut usize) -> Vec<String> {
    let mut out = Vec::new();
    for dir in template_dirs(root) {
        *swept += 1;
        let declared = declared_ctx_keys(&dir);
        for (key, first_seen) in used_ctx_keys(&dir) {
            if !declared.contains(&key) {
                out.push(format!(
                    "template '{}' USES ${{ctx.{key}}} (in {}) and does not DECLARE it: add \
                     requires.ctx.{key} to {}/template.json, or a builder can only learn the key \
                     by being rejected for it.",
                    shown(root, &dir),
                    shown(root, &first_seen),
                    shown(root, &dir),
                ));
            }
        }
    }
    out
}

/// Direction B — every declared key is used. The reverse inclusion, so the
/// declaration cannot become a leaflet nobody consumes.
fn unused(root: &Path, swept: &mut usize) -> Vec<String> {
    let mut out = Vec::new();
    for dir in template_dirs(root) {
        *swept += 1;
        let used: BTreeSet<String> = used_ctx_keys(&dir).into_iter().map(|(k, _)| k).collect();
        for key in declared_ctx_keys(&dir) {
            if !used.contains(&key) {
                out.push(format!(
                    "template '{}' DECLARES requires.ctx.{key} and no config value of its own \
                     USES ${{ctx.{key}}}: drop the entry, or the declaration asks a builder for a \
                     value nothing reads.",
                    shown(root, &dir),
                ));
            }
        }
    }
    out
}

/// The sweep must not be able to pass by finding nothing. Derived, not
/// declared: the library ships in two sizes (this tree carries every template,
/// the published tree a subset), so a count that is honest in one is vacuous in
/// the other. The floor sits below the smaller tree and far above zero.
const MIN_TEMPLATES: usize = 15;

#[test]
fn every_ctx_key_a_template_uses_is_declared() {
    let root = core_root().join("templates");
    let mut swept = 0usize;
    let found = undeclared(&root, &mut swept);
    assert!(
        found.is_empty(),
        "shipped templates use ctx keys their descriptor does not declare:\n  {}",
        found.join("\n  ")
    );
    assert!(
        swept >= MIN_TEMPLATES,
        "the sweep read {swept} template descriptors under {} — that is not the shipped tree",
        root.display()
    );
}

#[test]
fn every_declared_ctx_key_is_used() {
    let root = core_root().join("templates");
    let mut swept = 0usize;
    let found = unused(&root, &mut swept);
    assert!(
        found.is_empty(),
        "shipped templates declare ctx keys nothing of theirs uses:\n  {}",
        found.join("\n  ")
    );
    assert!(
        swept >= MIN_TEMPLATES,
        "the sweep read {swept} template descriptors under {} — that is not the shipped tree",
        root.display()
    );
}

// ─────────────────────────────────────────────────── the tests of the test

/// Build a throwaway templates root from `(relative path, bytes)` pairs.
fn planted(files: &[(&str, &str)]) -> tempfile::TempDir {
    let td = tempfile::TempDir::new().unwrap();
    for (rel, body) in files {
        let p = td.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
    }
    td
}

/// The state this gate was built for: a placeholder in a config value with no
/// entry beside it. The finding has to name the template, the key and the
/// direction, or it sends the reader to the wrong file.
#[test]
fn an_undeclared_key_is_a_finding_naming_template_key_and_direction() {
    let td = planted(&[
        (
            "talky/template.json",
            r#"{"name":"talky","version":"1.0.0"}"#,
        ),
        (
            "talky/brain/config.json",
            r#"{"cell":{"type":"llm"},"params":{"model":"${ctx.model}"}}"#,
        ),
    ]);
    let mut swept = 0usize;
    let found = undeclared(td.path(), &mut swept);
    assert_eq!(
        found.len(),
        1,
        "the sweep missed it (or invented): {found:#?}"
    );
    assert!(
        found[0].contains("talky") && found[0].contains("ctx.model") && found[0].contains("USES"),
        "the finding does not name template, key and direction: {}",
        found[0]
    );
    assert_eq!(swept, 1, "the planted descriptor was not read");
    assert!(
        unused(td.path(), &mut 0).is_empty(),
        "an undeclared key must not also be reported as an unused declaration"
    );
}

/// The other direction, which is the one a one-way gate would let rot: an entry
/// whose placeholder is gone.
#[test]
fn a_declared_key_nothing_uses_is_a_finding() {
    let td = planted(&[
        (
            "talky/template.json",
            r#"{"name":"talky","version":"1.0.0",
                "requires":{"ctx":{"model":{"type":"string","required":true,"because":"why"}}}}"#,
        ),
        (
            "talky/brain/config.json",
            r#"{"cell":{"type":"llm"},"params":{"model":"fixed-literal"}}"#,
        ),
    ]);
    let mut swept = 0usize;
    let found = unused(td.path(), &mut swept);
    assert_eq!(found.len(), 1, "the leaflet was not reported: {found:#?}");
    assert!(
        found[0].contains("talky")
            && found[0].contains("ctx.model")
            && found[0].contains("DECLARES"),
        "the finding does not name template, key and direction: {}",
        found[0]
    );
    assert!(
        undeclared(td.path(), &mut 0).is_empty(),
        "a leaflet must not also be reported as an undeclared key"
    );
}

/// The positive control: a matched pair is silent in both directions. Without
/// it every assertion above would also hold for a sweep that reported the whole
/// tree.
#[test]
fn a_matched_declaration_is_silent_in_both_directions() {
    let td = planted(&[
        (
            "talky/template.json",
            r#"{"name":"talky","version":"1.0.0",
                "requires":{"ctx":{"model":{"type":"string","required":true,"because":"why"}}}}"#,
        ),
        (
            "talky/brain/config.json",
            r#"{"cell":{"type":"llm"},"params":{"model":"${ctx.model}"}}"#,
        ),
    ]);
    let mut swept = 0usize;
    assert!(undeclared(td.path(), &mut swept).is_empty());
    assert!(unused(td.path(), &mut swept).is_empty());
    assert_eq!(swept, 2, "both directions read the planted descriptor");
}

/// Prose is not a placeholder. `template.json` and `README.md` both quote
/// `${ctx.*}` in the shipped tree to EXPLAIN it; only a config VALUE is a use.
#[test]
fn a_ctx_token_in_prose_is_not_a_use() {
    let td = planted(&[
        (
            "llm-unit/template.json",
            r#"{"name":"llm-unit","version":"1.0.0",
                "description":{"purpose":"params.model is ${ctx.model} (strict)."}}"#,
        ),
        ("llm-unit/README.md", "`params.model` is `${ctx.model}`.\n"),
        (
            "llm-unit/llm/config.json",
            r#"{"cell":{"type":"llm"},"params":{"model":"literal"}}"#,
        ),
    ]);
    assert!(
        undeclared(td.path(), &mut 0).is_empty(),
        "a quoted token was counted as a use"
    );
    assert!(
        unused(td.path(), &mut 0).is_empty(),
        "a template with no ctx use and no declaration is not a finding"
    );
}

/// A nested template owns its own parameter surface. A composite that carries a
/// sub-template must not be made to answer for the sub-template's keys — that
/// is what turns a derived declaration back into a duplicated one.
#[test]
fn the_walk_stops_at_a_nested_descriptor() {
    let td = planted(&[
        (
            "outer/template.json",
            r#"{"name":"outer","version":"1.0.0"}"#,
        ),
        (
            "outer/config.json",
            r#"{"cell":{"type":"hive"},"params":{}}"#,
        ),
        (
            "outer/inner/template.json",
            r#"{"name":"inner","version":"1.0.0"}"#,
        ),
        (
            "outer/inner/llm/config.json",
            r#"{"cell":{"type":"llm"},"params":{"model":"${ctx.model}"}}"#,
        ),
    ]);
    let found = undeclared(td.path(), &mut 0);
    assert_eq!(
        found.len(),
        1,
        "the key belongs to exactly one template: {found:#?}"
    );
    assert!(
        found[0].contains("outer/inner") && !found[0].starts_with("template 'outer' "),
        "the nested template's key was charged to its parent: {}",
        found[0]
    );
}

/// The `ref` markers `talky` and `cogny` carry since GH #277 are ordinary
/// config files with no placeholder in them: the sweep reads them and finds
/// nothing, and the referenced template's own keys stay the referenced
/// template's business.
#[test]
fn a_ref_marker_contributes_nothing_and_is_not_expanded() {
    let td = planted(&[
        (
            "composite/template.json",
            r#"{"name":"composite","version":"1.0.0"}"#,
        ),
        (
            "composite/sub/config.json",
            r#"{"cell":{"type":"ref","template":"sub@1.0.0"}}"#,
        ),
        ("sub/template.json", r#"{"name":"sub","version":"1.0.0"}"#),
        (
            "sub/writer/config.json",
            r#"{"cell":{"type":"llm"},"params":{"model":"${ctx.model}"}}"#,
        ),
    ]);
    let found = undeclared(td.path(), &mut 0);
    assert_eq!(found.len(), 1, "expected exactly one finding: {found:#?}");
    assert!(
        found[0].contains("template 'sub'"),
        "the ref was expanded — the composite was charged for the target's key: {}",
        found[0]
    );
}
