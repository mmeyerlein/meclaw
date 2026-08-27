//! GH #408 (Ruling S2, Option 2): a prose cross-reference names a template,
//! never a version.
//!
//! The library has two kinds of cross-reference and only one was defended. A
//! container level that derives its lanes from its occupant names the version it
//! derived from, and `gh302_*` reads both files and refuses to be green until
//! they agree — that chain works, and it lives in `config.json`, which this gate
//! does not touch. A README or `template.json` that mentions another template
//! with a full version is read by NOTHING: `a_documented_template_reference_resolves`
//! scans for the literal key `"template"`, so a version inside a sentence is
//! invisible to it, and § 4a's greps run at bump time and rule that "hits naming
//! other templates are none of that commit's business" — which is right, and is
//! exactly why these rot.
//!
//! Measured at the ruling: four such lines were stale, one of them naming a
//! version the registry no longer holds. The fix with no cascade is to stop
//! writing the number: a cross-reference almost never means a particular
//! version, it means "that template over there".
//!
//! FOUR exceptions, each earned by a real pattern in the tree rather than
//! guessed. See the constants.

use std::path::{Path, PathBuf};

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// (b) The library file. Two of its hits are DIDACTIC — `talky@1.2.0` is there
/// to be unresolvable, because the sentence explains what an exact reference
/// does when it misses. A gate that fixed those would make the docs wrong.
const LIBRARY_FILE: &str = "templates/README.md";

/// (c2) A line that places its reference in the PAST. `name@N` shorthand is
/// already ruled historical (§ 4a); a full version in such a sentence is the
/// same thing spelled out, and it is legitimate — it names a version that IS
/// past, so it cannot go stale.
const HISTORY_MARKERS: &[&str] = &[
    "Since ",
    "since ",
    "Until ",
    "until ",
    "Up to ",
    "up to ",
    "RETRACTED",
    "migration",
    "used to",
    "no longer",
    "before ",
    "Before ",
];

/// (d1) Delegated to `a_documented_template_reference_resolves`, which resolves
/// these against the registry and REQUIRES the version. Two gates on one line
/// would contradict each other.
const RECIPE_KEY: &str = "\"template\"";

/// Every `name@x.y.z` on the line, with the name.
fn references(line: &str) -> Vec<String> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'@' {
            i += 1;
            continue;
        }
        // walk back over the name
        let mut s = i;
        while s > 0 {
            let c = bytes[s - 1];
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'_' {
                s -= 1;
            } else {
                break;
            }
        }
        // walk forward over x.y.z
        let mut e = i + 1;
        let mut dots = 0;
        while e < bytes.len() && (bytes[e].is_ascii_digit() || bytes[e] == b'.') {
            if bytes[e] == b'.' {
                dots += 1;
            }
            e += 1;
        }
        if s < i && dots == 2 && e > i + 1 {
            out.push(String::from_utf8_lossy(&bytes[s..i]).to_string());
        }
        i = e.max(i + 1);
    }
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.ends_with(".md") || name == "template.json" {
                out.push(p);
            }
        }
    }
}

/// The template directory a file belongs to: `templates/<this>/…`.
fn owning_template(rel: &str) -> Option<&str> {
    rel.strip_prefix("templates/")?.split('/').next()
}

#[test]
fn no_prose_cross_reference_carries_a_version() {
    let root = repo("templates");
    if !root.is_dir() {
        return; // not a tree that ships templates
    }
    let mut files = Vec::new();
    walk(&root, &mut files);
    assert!(
        files.len() >= 20,
        "the sweep found almost nothing ({}) — the walk broke, the tree did not",
        files.len(),
    );

    let mut findings = Vec::new();
    let mut checked = 0usize;
    for path in &files {
        let rel = path
            .strip_prefix(repo("."))
            .unwrap_or(path)
            .to_string_lossy()
            .replace("./", "");
        if rel.ends_with(LIBRARY_FILE) {
            continue; // (b)
        }
        let own = owning_template(&rel).unwrap_or("");
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        for (n, line) in text.lines().enumerate() {
            if line.contains(RECIPE_KEY) {
                continue; // (d1)
            }
            if HISTORY_MARKERS.iter().any(|m| line.contains(m)) {
                continue; // (c2)
            }
            for name in references(line) {
                checked += 1;
                if name == own {
                    continue; // (a) + (c1)
                }
                findings.push(format!(
                    "{rel}:{}  `{name}@…` — a cross-reference names the template, \
                     not a version of it (line: {})",
                    n + 1,
                    line.trim(),
                ));
            }
        }
    }
    assert!(
        checked > 0,
        "the pattern matched nothing at all — it was mistyped, the tree is not clean \
         (§ 2c: an empty result and a forgotten call must never look alike)",
    );
    assert!(
        findings.is_empty(),
        "prose cross-references still carry versions ({}):\n  {}",
        findings.len(),
        findings.join("\n  "),
    );
}

#[test]
fn the_exceptions_are_load_bearing_and_not_a_silence() {
    // Each exception must actually FIRE on the tree — an exception nothing hits
    // is a hole somebody widened by accident.
    let lib = std::fs::read_to_string(repo(LIBRARY_FILE)).expect("library file");
    assert!(
        lib.lines().any(|l| !references(l).is_empty()),
        "the library-file exception no longer covers anything",
    );
    let hits = std::fs::read_to_string(repo("templates/collector/README.md"))
        .expect("collector README")
        .lines()
        .filter(|l| HISTORY_MARKERS.iter().any(|m| l.contains(m)) && !references(l).is_empty())
        .count();
    assert!(
        hits > 0,
        "the history-marker exception no longer covers anything",
    );
}
