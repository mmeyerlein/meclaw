//! GH #203 — an endpoint a shipped doc writes down is one the boundary admits.
//!
//! The boundary seals retired every interior address, and the sub-unit renames
//! that came with them were applied by replacing the LEADING segment of a path
//! and keeping the trailing one — which is exactly the segment that stopped
//! being addressable. So `templates/README.md` went on telling readers to wire
//! `./agent/session-keeper/stamp`, and `summarizer`'s own port recipe went on
//! offering `{"from": "<collector>/assemble", "to": "<summarizer>/prep"}`. Both
//! parse, both read as instructions, and both are refused by the mutation
//! validation with `hive_port_boundary` — correctly. `templates/README.md`
//! § Versioning presents these slots as the interface, so this is documentation
//! that participates in the public contract rather than describing it from the
//! outside; it had shipped twice by the time it was noticed, because prose is
//! the one surface no test was reading.
//!
//! **The question is asked of the substrate, never of a second opinion.** Same
//! reasoning as `gh196_shipped_hive_ports` and `gh202_shipped_drain_requirements`:
//! each candidate template's `config.json` is planted in a throwaway colony root,
//! read by the REAL `collect_sealed_hives`, and the documented endpoint is put
//! through the REAL `validate_hive_port_boundary` as an edge coming from outside.
//! A check that re-derived "what a port name looks like" could agree with itself
//! while disagreeing with the colony, which is the state this defect lived in.
//!
//! **What is scanned, and why that is not prose-parsing.** Only the literal
//! `from:` / `to:` keys are read — quoted as JSON in a fenced example, or bare
//! in the JSON-ish shorthand a `template.json` description slot uses. That token
//! names an edge endpoint wherever it appears, so an occurrence in a shipped doc
//! IS a wiring instruction and there is nothing to guess about it. Everything
//! else stays deliberately out of scope: a ports TABLE row or a sentence naming
//! an address is not distinguishable from prose ABOUT an address (this file's own
//! `hive_port_boundary` examples above would be false positives), and a check
//! that guessed there would be a check people learn to work around.
//!
//! **Conservative by construction, like the boundary itself.** An endpoint whose
//! segment names nothing the shipped tree carries is skipped rather than judged
//! — the same rule the port check follows ("it never rejects an edge it cannot
//! place"). The two counters at the end are what keep that conservatism from
//! degenerating into a test that passes by looking at nothing.

use meclaw_colony::config::HiveParams;
use meclaw_colony::mutation::port_boundary::validate_hive_port_boundary;
use meclaw_colony::mutation::port_boundary::{SealedHive, collect_sealed_hives};
use meclaw_core::serde_json::{Value, json};
use std::collections::HashMap;

/// Where the synthetic hive lives while it is being checked, and a caller
/// outside it. Any paths do; what matters is that "outside" really is.
const HIVE: &str = "/h";
const CALLER: &str = "/caller";

fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

// ───────────────────────────────────────────── what the shipped tree seals

/// One shipped hive template that declared `params.ports`, in the form the
/// scan needs: where its declaration lives, and where it lives on disk.
struct Sealed {
    /// Path relative to the repo root, for the failure message.
    rel: String,
    /// The declaration itself, read by the substrate on demand.
    config: std::path::PathBuf,
}

/// Every `config.json` under `templates/` whose cell type is `hive` AND which
/// declares `params.ports`, indexed by the NAMES a document could call it.
///
/// Two names per template, and only where they are unambiguous across the
/// sealed set:
///
/// - its directory basename (`session-keeper`, `collector`, `memory-drain`),
///   which is what an instance is named by the rule that an instance carries
///   its template's name; and
/// - the tail after the last dash (`memory-drain` -> `drain`), because a
///   shipped recipe writes the instance as `<drain>` and the whole point of
///   `memory-drain@2` is that `<drain>/drain` stopped resolving.
///
/// A name is ambiguous only when two templates that answer to it are sealed
/// DIFFERENTLY -- an accusation could then name the wrong one. The three
/// `collector` directories (`templates/collector` plus the copies inside
/// `talky` and `cogny`) are not ambiguous in that sense: they are byte
/// identical by `the_sub_unit_copies_are_byte_identical_to_their_templates`,
/// so the substrate reads one and the same seal from each, and the shallowest
/// path is the one worth naming in a failure message. That equality is decided
/// by the substrate's reader as well, never by comparing the files here.
fn sealed_by_name() -> HashMap<String, Sealed> {
    let root = core_root();
    let templates = root.join("templates");
    let mut found: Vec<Sealed> = Vec::new();
    walk_sealed(&root, &templates, &mut found);
    // Shallowest first, so a collapsed group is blamed on the template proper
    // rather than on a copy that lives inside a composite.
    found.sort_by_key(|s| (s.rel.matches('/').count(), s.rel.clone()));
    let mut by_name: HashMap<String, Vec<Sealed>> = HashMap::new();
    for s in found {
        let base = s
            .config
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let mut names = vec![base.clone()];
        if let Some((_, tail)) = base.rsplit_once('-') {
            names.push(tail.to_string());
        }
        for n in names {
            by_name.entry(n).or_default().push(Sealed {
                rel: s.rel.clone(),
                config: s.config.clone(),
            });
        }
    }
    by_name
        .into_iter()
        .filter_map(|(name, mut v)| {
            let head = seal_the_substrate_reads(&v[0].config);
            let one_seal = v
                .iter()
                .all(|s| seal_the_substrate_reads(&s.config).ports == head.ports);
            one_seal.then(|| (name, v.remove(0)))
        })
        .collect()
}

fn walk_sealed(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<Sealed>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let entry = entry.unwrap();
        let p = entry.path();
        if p.is_dir() {
            walk_sealed(root, &p, out);
            continue;
        }
        if p.file_name().and_then(|n| n.to_str()) != Some("config.json") {
            continue;
        }
        let raw = std::fs::read_to_string(&p).unwrap();
        let val: Value = meclaw_core::serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        if val
            .get("cell")
            .and_then(|c| c.get("type"))
            .and_then(|t| t.as_str())
            != Some("hive")
        {
            continue;
        }
        let params = val.get("params").cloned().unwrap_or(Value::Null);
        if params.is_null() {
            continue;
        }
        let hp: HiveParams = meclaw_core::serde_json::from_value(params)
            .unwrap_or_else(|e| panic!("{}: params: {e}", p.display()));
        if hp.ports.is_none() {
            continue; // key absent is the OPEN state — nothing is sealed here
        }
        out.push(Sealed {
            rel: p.strip_prefix(root).unwrap().display().to_string(),
            config: p.clone(),
        });
    }
}

/// Plant one template's `config.json` as the hive `/h` of a throwaway colony
/// root and let the substrate's own reader say what it seals. Only
/// `config.json` is needed: that file IS the declaration, and reading it per
/// mutation is how a live colony learns a hive's boundary.
fn seal_the_substrate_reads(config: &std::path::Path) -> SealedHive {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    std::fs::create_dir_all(root.join("main/h")).unwrap();
    std::fs::write(root.join("main/config.json"), r#"{"cell":{"type":"hive"}}"#).unwrap();
    std::fs::copy(config, root.join("main/h/config.json")).unwrap();
    let paths = [meclaw_core::Path::new(HIVE)];
    let mut sealed = collect_sealed_hives(root, paths.iter());
    assert_eq!(
        sealed.len(),
        1,
        "{}: the reader saw no seal",
        config.display()
    );
    sealed.remove(0)
}

/// Does the REAL boundary let an edge from outside land on `<hive>/<child>`?
fn boundary_admits(seal: &SealedHive, child: &str) -> bool {
    let diff = json!({"add_edges": [{"from": CALLER, "to": format!("{HIVE}/{child}")}]});
    validate_hive_port_boundary(&diff, "/", std::slice::from_ref(seal)).is_ok()
}

// ─────────────────────────────────────── what the shipped docs write down

/// One `from:` / `to:` value found in a shipped document.
struct Endpoint {
    file: String,
    line: usize,
    raw: String,
}

/// Pull every `from:` / `to:` value out of one file.
///
/// Deliberately literal: the key has to be `from` or `to` (optionally quoted),
/// followed by `:`. That is the JSON edge shape and the shorthand a description
/// slot writes it in; it is not an attempt to recognise an address in a
/// sentence. A value without a `/` cannot cross any boundary and is dropped
/// here rather than resolved later.
fn endpoints_in(file: &str, text: &str) -> Vec<Endpoint> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        // Byte-wise throughout: a shipped README is full of `→`, `─` and `⋮`,
        // and slicing a `str` at an arbitrary offset would panic on the first
        // one. Every token this scan cares about is ASCII, so a non-ASCII byte
        // simply ends an endpoint.
        let b = line.as_bytes();
        let mut i = 0usize;
        while i < b.len() {
            let key = if b[i..].starts_with(b"from") {
                4
            } else if b[i..].starts_with(b"to") {
                2
            } else {
                i += 1;
                continue;
            };
            // A left boundary, so `into:` is not `to:` and `platform:` is not
            // `from:`. Without it the scan reads English as JSON.
            let left_ok = i == 0 || !is_word(b[i - 1]);
            let mut j = i + key;
            if j < b.len() && b[j] == b'"' {
                j += 1;
            }
            while j < b.len() && b[j] == b' ' {
                j += 1;
            }
            if !left_ok || j >= b.len() || b[j] != b':' {
                i += 1;
                continue;
            }
            j += 1;
            while j < b.len() && b[j] == b' ' {
                j += 1;
            }
            if j < b.len() && b[j] == b'"' {
                j += 1;
            }
            let start = j;
            while j < b.len() && is_endpoint_byte(b[j]) {
                j += 1;
            }
            // ASCII by construction, so this cannot fail — but an endpoint that
            // somehow was not is skipped rather than panicked on.
            if let Ok(raw) = std::str::from_utf8(&b[start..j])
                && raw.contains('/')
            {
                out.push(Endpoint {
                    file: file.to_string(),
                    line: n + 1,
                    raw: raw.to_string(),
                });
            }
            i = j.max(i + 1);
        }
    }
    out
}

fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// The bytes an endpoint may be spelled with — including `<` and `>`, because
/// a template README writes the caller's instance as `<talky>` and the segment
/// inside the brackets is the name that has to resolve.
fn is_endpoint_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'.' | b'/' | b'-' | b'_' | b'<' | b'>')
}

/// Every shipped `.md` and `.json` under `templates/` and `examples/`.
///
/// `builder-librarian`'s seed is excluded: it is a GENERATED corpus that embeds
/// other files verbatim, so a finding there is a duplicate of the finding in the
/// source, reported against a file nobody edits by hand.
fn shipped_docs() -> Vec<(String, String)> {
    let root = core_root();
    let mut out = Vec::new();
    for base in ["templates", "examples"] {
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
            if name != "builder-librarian" {
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

/// Every documented endpoint in `docs` that the boundary refuses, as one
/// sentence each. `scanned` counts the endpoints looked at, so a caller can
/// tell "nothing wrong" from "nothing read".
fn findings(docs: &[(String, String)], scanned: &mut usize) -> Vec<String> {
    let sealed = sealed_by_name();
    assert!(
        sealed.len() >= 8,
        "the sweep found almost no sealed hive templates: {}",
        sealed.len()
    );
    // The substrate is asked once per template, not once per endpoint.
    let mut seals: HashMap<String, SealedHive> = HashMap::new();
    let mut out = Vec::new();
    for (file, text) in docs {
        for ep in endpoints_in(file, text) {
            *scanned += 1;
            let segs: Vec<&str> = ep
                .raw
                .split('/')
                .map(|s| s.trim_matches(|c| c == '<' || c == '>' || c == '.'))
                .filter(|s| !s.is_empty())
                .collect();
            // Every adjacent pair: a hive name followed by what the document
            // asks to reach inside it. Deeper endpoints are pairs too, which is
            // why `./talky-<key>/session-keeper/stamp` is caught on its second.
            for pair in segs.windows(2) {
                let (name, child) = (pair[0], pair[1]);
                let Some(t) = sealed.get(name) else { continue };
                let seal = seals
                    .entry(name.to_string())
                    .or_insert_with(|| seal_the_substrate_reads(&t.config));
                if !boundary_admits(seal, child) {
                    let ports = if seal.ports.is_empty() {
                        "none — the hive path itself is the only address".to_string()
                    } else {
                        seal.ports.join(", ")
                    };
                    out.push(format!(
                        "{}:{}: '{}' reaches '{name}/{child}', and the boundary of the sealed \
                         template '{}' refuses it. Declared ports: {ports}. Write the hive path \
                         and put the meaning of the dropped segment on the edge's lane.",
                        ep.file, ep.line, ep.raw, t.rel
                    ));
                }
            }
        }
    }
    out
}

#[test]
fn every_documented_edge_endpoint_is_one_the_boundary_admits() {
    let mut scanned = 0usize;
    let found = findings(&shipped_docs(), &mut scanned);
    assert!(
        found.is_empty(),
        "shipped documentation wires past a hive boundary:\n  {}",
        found.join("\n  ")
    );
    // "Nothing wrong" and "nothing read" look identical from the outside, and
    // the second is the failure mode of a scan this conservative.
    assert!(
        scanned >= 300,
        "the scan read almost no endpoints: {scanned}"
    );
}

/// The test of the test, and the reason the sweep above may stay silent.
///
/// A correct tree contains, by construction, no endpoint that names a sealed
/// hive followed by anything — the fix drops that segment entirely — so the
/// sweep resolves almost nothing and would be green whether it worked or not.
/// This feeds it the exact lines #203 was filed about, verbatim from the state
/// they shipped in, and requires each one to be reported and to name the
/// template that refuses it.
#[test]
fn the_scan_reports_the_addresses_this_issue_was_filed_about() {
    let shipped_before_the_fix = [
        // templates/README.md § the canonical instantiation example
        (
            "templates/README.md".to_string(),
            r#"  "add_edges":[{"from":"./ingress","to":"./agent/session-keeper/stamp"}]"#
                .to_string(),
        ),
        // templates/summarizer/README.md § Ports — both ends breached at once
        (
            "templates/summarizer/README.md".to_string(),
            r#"{"from": "<collector>/assemble", "to": "<summarizer>/prep",}"#.to_string(),
        ),
        // templates/memory-drain/template.json — the JSON-ish description slot
        (
            "templates/memory-drain/template.json".to_string(),
            "wired as {from: <talky>/collector/assemble, to: <drain>/drain, condition:".to_string(),
        ),
        // templates/receptionist/README.md — the per-channel mutation recipe
        (
            "templates/receptionist/README.md".to_string(),
            r#"{"from": "./reception/greet", "to": "./talky-<key>/session-keeper/stamp","#
                .to_string(),
        ),
    ];
    let mut scanned = 0usize;
    let found = findings(&shipped_before_the_fix, &mut scanned);
    assert_eq!(
        found.len(),
        6,
        "the scan missed a known-bad address (or invented one): {found:#?}"
    );
    for (needle, whose) in [
        (
            "session-keeper/stamp",
            "templates/session-keeper/config.json",
        ),
        ("collector/assemble", "templates/collector/config.json"),
        ("summarizer/prep", "templates/summarizer/config.json"),
        ("drain/drain", "templates/memory-drain/config.json"),
    ] {
        assert!(
            found
                .iter()
                .any(|f| f.contains(needle) && f.contains(whose)),
            "no finding blamed '{whose}' for '{needle}': {found:#?}"
        );
    }
}
