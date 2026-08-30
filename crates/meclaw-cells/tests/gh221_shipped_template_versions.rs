//! GH #221 — every shipped `template.json` declares a version a builder can
//! reference.
//!
//! `template.json::version` is optional to the parser and mandatory to anybody
//! who writes `talky@2.0.0` in a mutation, and the gap between those two facts
//! is silent in both directions. `parse_template_json` reads the field with
//! `as_str()`, so a version that is not a STRING is not a parse error — it is
//! `None`, indistinguishable from a template that never declared one. The row
//! reaches the registry versionless, `resolve("name")` still finds it because
//! the unversioned entry wins when no versioned one exists, and the only thing
//! that breaks is the one form a document is most likely to write down:
//! `name@<version>` comes back `NotFound` for a template that is sitting right
//! there on disk.
//!
//! That is not hypothetical. `templates/llm-unit/template.json` shipped with
//! `"version": 1` — an integer — and was fixed in `1b99f284` after riding along
//! unnoticed, because nothing in the tree ever asked. `slack_template_smoke`
//! checks the shape of `slack-agent`'s descriptor and no other, and the
//! librarian corpus gate (#207) fails on a `template.json` that does not PARSE,
//! which an integer version does. So the file class that carries the library's
//! public version numbers — edited across a dozen templates in 0.14.0 alone —
//! had no library-wide check at all.
//!
//! **The question is asked of the substrate, never of a second opinion.** Same
//! reasoning as `gh196_shipped_hive_ports`, `gh202_shipped_drain_requirements`,
//! `gh203_documented_port_addresses` and `gh212_documented_override_params`:
//! three substrate calls decide every finding and none of them is re-derived
//! here.
//!
//! - `parse_template_json` — the function `scan_templates_dir` calls per file
//!   at boot and on every `/colony/templates/rescan`. It says whether the file
//!   parses and what version, if any, survives into the row. A test that read
//!   the JSON itself would be free to be more generous than the reader, which
//!   is exactly the state this defect lived in.
//! - `parse_simple_version` — the substrate's own `major.minor.patch` parser
//!   (R3, no `semver` crate). Whatever it rejects is by definition a version no
//!   reference can name, including the near-misses a hand-rolled check tends to
//!   wave through: `1.2`, `1.2.3-rc1`, `v1.0.0`.
//! - `TemplatesRegistry::resolve` — the consumer. The declaration is only worth
//!   anything if `name@version` gets back the directory it was read from, so
//!   the loop is closed at the surface a mutation actually uses rather than at
//!   the field.
//!
//! **What is scanned.** Every file literally named `template.json` under
//! `templates/`, found by walking the filesystem — that is the scanner's own
//! rule for what a template is (`scan_templates_dir` collects a directory iff
//! it carries that filename), so there is nothing to guess about which files
//! are in scope. The walk is done here rather than delegated whole because
//! `scan_templates_dir` returns on the FIRST parse failure: one broken
//! descriptor would hide every other finding behind it, and the per-file
//! function is the same one it would have called.

use meclaw_colony::templates::{
    ScannedTemplate, TemplateEntry, TemplatesRegistry, parse_simple_version, parse_template_json,
    scan_templates_dir,
};

fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every file named `template.json` at or below `dir`, sorted.
///
/// Deliberately the whole subtree and not just `templates/*/template.json`:
/// `templates/_cell-types/*/` carries descriptors one level deeper, and a
/// composite that ever kept a nested descriptor would be shipping a second
/// registry entry nobody looked at.
fn template_json_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    collect(dir, &mut out);
    out.sort();
    out
}

fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect(&p, out);
        } else if entry.file_name() == "template.json" {
            out.push(p);
        }
    }
}

/// What the raw bytes put in the `version` slot, for the failure message only.
///
/// The JUDGEMENT is always the substrate's; this exists so that "no version"
/// and "a version of the wrong type" — which `ScannedTemplate::version` maps
/// onto the same `None` — do not read as the same problem to whoever has to
/// fix it.
fn raw_version_shape(path: &std::path::Path) -> String {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return "the file could not be read".into();
    };
    let Ok(val) = meclaw_core::serde_json::from_str::<meclaw_core::serde_json::Value>(&raw) else {
        return "the file does not parse as JSON".into();
    };
    match val.get("version") {
        None => "there is no 'version' key".into(),
        Some(v) if v.is_string() => format!("'version' is the string {v}"),
        Some(v) => format!(
            "'version' is {v} — a {}, and the reader takes strings only, so the row reaches the \
             registry with no version at all",
            match v {
                meclaw_core::serde_json::Value::Number(_) => "number",
                meclaw_core::serde_json::Value::Bool(_) => "boolean",
                meclaw_core::serde_json::Value::Null => "null",
                meclaw_core::serde_json::Value::Array(_) => "array",
                _ => "object",
            }
        ),
    }
}

/// Every shipped descriptor that fails one of the three questions, as one
/// sentence each. `checked` counts the files the sweep actually read, so a
/// caller can tell "nothing wrong" from "nothing looked at".
fn findings(templates_root: &std::path::Path, checked: &mut usize) -> Vec<String> {
    let files = template_json_files(templates_root);
    let mut out = Vec::new();
    let mut scanned: Vec<(std::path::PathBuf, ScannedTemplate)> = Vec::new();

    for path in files {
        *checked += 1;
        let shown = path
            .strip_prefix(templates_root)
            .unwrap_or(&path)
            .display()
            .to_string();
        // 1. It parses. The reader that runs at boot is the one that decides.
        let parsed = match parse_template_json(&path) {
            Ok(p) => p,
            Err(e) => {
                out.push(format!(
                    "templates/{shown}: the descriptor does not parse ({e}). The boot scan and \
                     every /colony/templates/rescan abort here, so NO template is indexed."
                ));
                continue;
            }
        };
        // 2. A version survives into the row.
        let Some(version) = parsed.version.clone() else {
            out.push(format!(
                "templates/{shown}: '{}' reaches the registry with no version — {}. A builder \
                 writing '{}@<version>' gets TemplateMissing for a template that is on disk.",
                parsed.name,
                raw_version_shape(&path),
                parsed.name
            ));
            continue;
        };
        // 3. It is a version the substrate's own parser accepts.
        if let Err(e) = parse_simple_version(&version) {
            out.push(format!(
                "templates/{shown}: '{}' declares a version the resolver cannot read ({e}). It \
                 is indexed as unversioned, so '{}@{version}' resolves to nothing.",
                parsed.name, parsed.name
            ));
            continue;
        }
        scanned.push((path, parsed));
    }

    // 4. The loop closed at the consumer: the reference a document would write
    //    down gets back the directory the version was read from. Two templates
    //    claiming one (name, version) land here too — the registry keeps both
    //    rows and `resolve` hands out whichever it met first.
    let registry = TemplatesRegistry::from_entries(
        scanned
            .iter()
            .map(|(_, s)| TemplateEntry {
                template_id: format!("gh221:{}", s.filesystem_path.display()),
                name: s.name.clone(),
                version: s.version.clone(),
                filesystem_path: s.filesystem_path.clone(),
            })
            .collect(),
    );
    for (path, s) in &scanned {
        let shown = path
            .strip_prefix(templates_root)
            .unwrap_or(path)
            .display()
            .to_string();
        let reference = format!("{}@{}", s.name, s.version.as_deref().unwrap_or_default());
        match registry.resolve(&reference) {
            Err(e) => out.push(format!(
                "templates/{shown}: the registry cannot resolve '{reference}' ({e}), although \
                 that reference is what this very file declares."
            )),
            Ok(entry) if entry.filesystem_path != s.filesystem_path => out.push(format!(
                "templates/{shown}: '{reference}' resolves to {} instead — two shipped \
                 descriptors claim the same name and version.",
                entry.filesystem_path.display()
            )),
            Ok(_) => {}
        }
    }
    out
}

/// The top-level template directories that carry a descriptor of their own —
/// read straight out of the root, with none of the recursion the sweep uses.
///
/// This is the sweep's floor, and it is derived rather than declared. A number
/// cannot be: the library ships in two sizes (this tree carries every template,
/// the published tree a subset — `PUBLIC_TEMPLATES` in the maintainers'
/// export script), so a count that is honest in one is
/// either red or vacuous in the other. What the floor is for survives the
/// derivation intact — a sweep must not be able to pass by finding nothing —
/// and it gets there by comparing the recursive walk against a second,
/// independent read of the same directory.
fn root_descriptors(templates_root: &std::path::Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(templates_root) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().join("template.json").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

#[test]
fn every_shipped_template_declares_a_referenceable_version() {
    let root = core_root().join("templates");
    let mut checked = 0usize;
    let found = findings(&root, &mut checked);
    assert!(
        found.is_empty(),
        "shipped template descriptors carry a version nothing can reference:\n  {}",
        found.join("\n  ")
    );
    // "Nothing wrong" and "nothing looked at" are the same green from outside,
    // and the second is how a sweep over a directory quietly stops working.
    let roots = root_descriptors(&root);
    assert!(
        !roots.is_empty(),
        "no template descriptor under {} at all — wrong root",
        root.display()
    );
    let swept: std::collections::HashSet<std::path::PathBuf> =
        template_json_files(&root).into_iter().collect();
    let missed: Vec<&String> = roots
        .iter()
        .filter(|n| !swept.contains(&root.join(n).join("template.json")))
        .collect();
    assert!(
        missed.is_empty(),
        "the sweep walked past descriptors this tree carries: {missed:?} \
         (it read {checked} in total)"
    );
}

/// The boot path itself, in one line: no shipped descriptor makes the scan
/// abort. The sweep above reads the files one by one on purpose, so this is the
/// only place the REAL walker gets asked, and it is the form a live colony
/// runs.
#[test]
fn the_boot_scan_indexes_the_whole_shipped_tree() {
    let root = core_root().join("templates");
    let scanned = scan_templates_dir(&root).expect("the shipped templates directory scans");
    assert_eq!(
        scanned.len(),
        template_json_files(&root).len(),
        "the scanner indexed a different number of templates than there are descriptors on disk"
    );
}

// ─────────────────────────────────────────────────── the tests of the test

/// Build a throwaway templates root from `(dir, bytes)` pairs.
fn planted(files: &[(&str, &str)]) -> tempfile::TempDir {
    let td = tempfile::TempDir::new().unwrap();
    for (dir, body) in files {
        let d = td.path().join(dir);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("template.json"), body).unwrap();
    }
    td
}

/// The defect this issue was filed about, in the bytes it shipped in.
///
/// `templates/llm-unit/template.json` before `1b99f284`: a version that is a
/// JSON integer. The sweep has to report it, name the file, and say what the
/// reader did with it — a finding that only said "no version" would send
/// somebody looking for a missing key that is right there.
#[test]
fn the_sweep_reports_the_integer_version_this_issue_was_filed_about() {
    let td = planted(&[(
        "llm-unit",
        r#"{"name": "llm-unit", "version": 1, "description": {"purpose": "Composite unit."}}"#,
    )]);
    let mut checked = 0usize;
    let found = findings(td.path(), &mut checked);
    assert_eq!(
        found.len(),
        1,
        "the sweep missed the known-bad descriptor (or invented one): {found:#?}"
    );
    assert!(
        found[0].contains("llm-unit/template.json") && found[0].contains("no version"),
        "the finding does not name the file and what happened: {}",
        found[0]
    );
    assert!(
        found[0].contains("number"),
        "the finding does not say the version was of the wrong type: {}",
        found[0]
    );
    assert_eq!(checked, 1, "the sweep did not read the planted descriptor");
}

/// Why nobody noticed for a release: the integer version breaks exactly one
/// form, and it is not the one a smoke test reaches for.
///
/// This is the substrate speaking, not this file: the SAME registry the sweep
/// builds answers `llm-unit` happily and `llm-unit@1.0.0` not at all.
#[test]
fn an_unreadable_version_is_invisible_to_a_bare_name_reference() {
    let td = planted(&[(
        "llm-unit",
        r#"{"name": "llm-unit", "version": 1, "description": {}}"#,
    )]);
    let scanned = scan_templates_dir(td.path()).expect("an integer version is not a parse error");
    let registry = TemplatesRegistry::from_entries(
        scanned
            .iter()
            .map(|s| TemplateEntry {
                template_id: "t".into(),
                name: s.name.clone(),
                version: s.version.clone(),
                filesystem_path: s.filesystem_path.clone(),
            })
            .collect(),
    );
    assert!(
        registry.resolve("llm-unit").is_ok(),
        "the bare name stopped resolving — this test no longer describes the defect"
    );
    assert!(
        registry.resolve("llm-unit@1.0.0").is_err(),
        "the versioned reference resolved, so there would have been nothing to file"
    );
}

/// The near-misses. Each of these is a plausible thing to type and none of them
/// is a version `resolve` can be handed, so each has to be one finding.
#[test]
fn a_version_the_resolver_cannot_read_is_a_finding() {
    for bad in ["1.2", "1.2.3-rc1", "v1.0.0", "1.2.3+build", ""] {
        let td = planted(&[(
            "t",
            &format!(r#"{{"name": "t", "version": "{bad}", "description": {{}}}}"#),
        )]);
        let mut checked = 0usize;
        let found = findings(td.path(), &mut checked);
        assert_eq!(
            found.len(),
            1,
            "version {bad:?} was not reported exactly once: {found:#?}"
        );
        assert!(
            found[0].contains("cannot read"),
            "version {bad:?} was reported as the wrong problem: {}",
            found[0]
        );
    }
}

/// A descriptor with no `version` key at all, and one that is not JSON. Both
/// are separate failure modes from the integer, and both have to survive into
/// the message — the parse failure especially, because it is the one that stops
/// the boot scan dead.
#[test]
fn a_missing_version_and_a_broken_file_are_each_their_own_finding() {
    let td = planted(&[("t", r#"{"name": "t", "description": {}}"#)]);
    let mut checked = 0usize;
    let found = findings(td.path(), &mut checked);
    assert_eq!(
        found.len(),
        1,
        "a missing version was not reported: {found:#?}"
    );
    assert!(
        found[0].contains("no 'version' key"),
        "the finding does not say the key is absent: {}",
        found[0]
    );

    let td = planted(&[("t", r#"{"name": "t", "version": "1.0.0",}"#)]);
    let mut checked = 0usize;
    let found = findings(td.path(), &mut checked);
    assert_eq!(
        found.len(),
        1,
        "a broken descriptor was not reported: {found:#?}"
    );
    assert!(
        found[0].contains("does not parse"),
        "the finding does not say the file is unreadable: {}",
        found[0]
    );
}

/// The positive control, without which every assertion above would also pass
/// for a `findings` that reported everything it saw.
#[test]
fn a_well_formed_descriptor_is_silent() {
    let td = planted(&[
        ("talky", r#"{"name": "talky", "version": "2.0.0"}"#),
        ("canvy", r#"{"name": "canvy", "version": "0.2.0"}"#),
        ("access", r#"{"name": "access", "version": "1.0.1"}"#),
    ]);
    let mut checked = 0usize;
    let found = findings(td.path(), &mut checked);
    assert!(
        found.is_empty(),
        "a valid descriptor was flagged: {found:#?}"
    );
    assert_eq!(checked, 3, "not every planted descriptor was read");
}

/// Two directories claiming one `name@version`: both parse, both carry a
/// SemVer, and the reference is still ambiguous. Only the consumer-level
/// question catches this, which is why the sweep asks it.
#[test]
fn two_descriptors_claiming_one_reference_are_a_finding() {
    let td = planted(&[
        ("talky", r#"{"name": "talky", "version": "2.0.0"}"#),
        ("nested/talky", r#"{"name": "talky", "version": "2.0.0"}"#),
    ]);
    let mut checked = 0usize;
    let found = findings(td.path(), &mut checked);
    assert_eq!(
        found.len(),
        1,
        "a duplicated name@version was not reported exactly once: {found:#?}"
    );
    assert!(
        found[0].contains("same name and version"),
        "the finding does not say what is ambiguous: {}",
        found[0]
    );
}
