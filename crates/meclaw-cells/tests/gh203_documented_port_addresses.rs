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
//!
//! **`docs/` is scanned too (GH #229), and that needed a carve-out.** The scan
//! started at `templates/` and `examples/`, which left the specification — the
//! one document where a wrong address costs the most readers — ungated; it had
//! kept a refused address inside the very section that defines the rule the
//! address breaks. But that section now also teaches BY counter-example: a pair
//! of edges shown as wrong on purpose, one breaking requirement 1 and one
//! breaking requirement 2. A plain extension would flag the teaching material.
//!
//! So a fenced block may declare itself an intentional counter-example, on the
//! line directly above its opening fence:
//!
//! ```text
//! <!-- gate:counter-example refused=./talky/session-keeper/stamp -->
//! ```
//!
//! **The marker names the addresses rather than merely silencing the block**,
//! and that is the whole of the decision (the open question on GH #229). A bare
//! "ignore this block" would let the lesson rot in silence: the day the boundary
//! stops refusing an address, the block would quietly become a correct example
//! labelled as wrong, and nothing would say so. Naming them turns the exemption
//! into an assertion — the declared set must equal the set the REAL boundary
//! refuses, so a counter-example that stops being one fails here.
//!
//! Naming them also keeps the pair honest, which a block-level mute could not:
//! of the two edges taught as wrong, only the first is refused by the substrate.
//! The second addresses the hive correctly and breaks requirement 2 — a lane
//! named after an inner cell — which no validator can see. A mute would have
//! implied both are refused; the marker states, in the document itself, which
//! one the machine catches and which one only a reader can.
//!
//! **The plan archive is out of scope (owner ruling 2026-08-23 R7).**
//! `docs/superpowers/plans/` is not scanned. By the project's authority
//! hierarchy a plan is historical and non-authoritative — it records what was
//! true while its wave ran, which is the same reason `docs/archive/` was
//! already carved out above. Holding a frozen plan to today's addresses made
//! every rename edit somebody else's archived plan: history falsification, not
//! conformance. Those edits stand; from here the archive freezes. The one plan
//! that must stay true is the one about to be driven, and it is kept true by
//! the re-baseline step at the end of every wave (the wave meta-plan, private
//! tree, § „Folgewelle re-baselinen"), not by a gate pulling the whole archive
//! forward. The ruling was written for the template-reference gate
//! (`a_documented_template_reference_resolves`) and extended to this one by
//! controller ruling, because the reason it gives is about `plans/`, not about
//! that gate.
//!
//! Two more properties keep the marker from becoming a mode: it must sit
//! directly above a fenced block (it exempts that block and nothing else), and
//! `the_counter_example_marker_stays_a_carve_out` caps how many may exist at
//! all. Both language twins of a document carry their own marker, which is why
//! the cap is a handful rather than one.

use meclaw_colony::config::HiveParams;
use meclaw_colony::mutation::hive_contract::contract_from_cell_dir;
use meclaw_colony::mutation::port_boundary::{SealedHive, collect_sealed_hives};
use meclaw_colony::mutation::port_boundary::{
    collect_hive_port_boundary, validate_hive_port_boundary,
};
use meclaw_colony::mutation::rejection::MutationRejection;
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
#[derive(Clone)]
struct Sealed {
    /// Path relative to the repo root, for the failure message.
    rel: String,
    /// The declaration itself, read by the substrate on demand.
    config: std::path::PathBuf,
}

/// How a document came to name a template.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Naming {
    /// The segment IS the template's directory name.
    Own,
    /// The segment is the shortened instance name the template's own recipes
    /// write for themselves -- the tail after the last dash. GH #238.
    Convention,
}

/// The sealed templates a document's path segments can resolve to.
struct Library {
    /// Directory basename -> the template, where unambiguous across the set.
    by_name: HashMap<String, Sealed>,
    /// Tail after the last dash -> the templates that answer to it, shallowest
    /// first. Read ONLY for documents that live inside the template's own
    /// directory.
    by_tail: HashMap<String, Vec<Sealed>>,
}

impl Library {
    /// The sealed template a path segment names, or `None` when it names
    /// nothing this tree ships.
    ///
    /// **Equality first, and for a foreign document equality is all there is.**
    /// The convention below only reads inside the template it belongs to.
    fn resolve(&self, seg: &str, file: &str) -> Option<(&Sealed, Naming)> {
        if let Some(s) = self.by_name.get(seg) {
            return Some((s, Naming::Own));
        }
        self.by_tail
            .get(seg)?
            .iter()
            .find(|s| file.starts_with(&format!("{}/", dir_of(&s.rel))))
            .map(|s| (s, Naming::Convention))
    }

    /// How many distinct names resolve at all, for the "did it read anything"
    /// floor.
    fn len(&self) -> usize {
        self.by_name.len()
    }
}

/// The directory a template's `config.json` sits in, relative to the repo root.
fn dir_of(rel: &str) -> &str {
    rel.rsplit_once('/').map(|(d, _)| d).unwrap_or(rel)
}

/// Every `config.json` under `templates/` whose cell type is `hive` AND which
/// declares `params.ports`, indexed by the NAMES a document could call it.
///
/// **A segment resolves by EQUALITY with the directory basename**
/// (`session-keeper`, `collector`, `memory-drain`) -- the name an instance
/// carries by the rule that an instance is named after its template.
///
/// One convention sits beside it, and only inside the template's OWN
/// documents: the tail after the last dash (`memory-drain` -> `drain`),
/// because a shipped recipe writes its own instance shortened, and the whole
/// point of `memory-drain@2` is that `<drain>/drain` stopped resolving.
///
/// **That alias is deliberately not global (GH #238.)** `templates/README.md`
/// writes `./agent/session-keeper/stamp`, where `agent` stands for whatever
/// the reader named their own agent hive; a global tail match resolved it to
/// `slack-agent` and blamed a template the example has nothing to do with.
/// The verdict was right about the address and wrong about the file, which is
/// the failure mode that survives longest -- and it made this file's own
/// self-test count depend on whether a private template ships.
///
/// A name is ambiguous only when two templates that answer to it are sealed
/// DIFFERENTLY -- an accusation could then name the wrong one. The three
/// `collector` directories (`templates/collector` plus the copies inside
/// `talky` and `cogny`) are not ambiguous in that sense: they are byte
/// identical by `the_sub_unit_copies_are_byte_identical_to_their_templates`,
/// so the substrate reads one and the same seal from each, and the shallowest
/// path is the one worth naming in a failure message. That equality is decided
/// by the substrate's reader as well, never by comparing the files here.
fn sealed_library() -> Library {
    let root = core_root();
    let templates = root.join("templates");
    let mut found: Vec<Sealed> = Vec::new();
    walk_sealed(&root, &templates, &mut found);
    // Shallowest first, so a collapsed group is blamed on the template proper
    // rather than on a copy that lives inside a composite.
    found.sort_by_key(|s| (s.rel.matches('/').count(), s.rel.clone()));
    let mut by_name: HashMap<String, Vec<Sealed>> = HashMap::new();
    let mut by_tail: HashMap<String, Vec<Sealed>> = HashMap::new();
    for s in found {
        let base = dir_of(&s.rel)
            .rsplit_once('/')
            .map(|(_, b)| b.to_string())
            .unwrap_or_else(|| dir_of(&s.rel).to_string());
        by_name.entry(base.clone()).or_default().push(s.clone());
        if let Some((_, tail)) = base.rsplit_once('-') {
            by_tail.entry(tail.to_string()).or_default().push(s);
        }
    }
    let by_name = by_name
        .into_iter()
        .filter_map(|(name, mut v)| {
            let head = seal_the_substrate_reads(&v[0].config);
            let one_seal = v
                .iter()
                .all(|s| seal_the_substrate_reads(&s.config).ports == head.ports);
            one_seal.then(|| (name, v.remove(0)))
        })
        .collect();
    Library { by_name, by_tail }
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

/// GH #559/#560 — the second address a sealed template can admit: a **connect
/// point**.
///
/// A template may pronounce, about ITSELF, that one named lane docks at one
/// address inside it (`params.contract.…[].at`), and Stage 6 then waives the
/// seal for an edge that declares that lane. A document writing such an
/// endpoint is writing a v-lane, not a port bypass, and this gate would
/// otherwise refuse the one edge shape the substrate now sanctions.
///
/// **The lanes come from the file, the VERDICT comes from the substrate.** The
/// declared `at` list says which lanes are worth asking about; whether the
/// address is admitted is answered by the same `collect_hive_port_boundary`
/// the mutation door runs. What is deliberately NOT asked is which lane the
/// document's own edge carries — a `lane` key sits on a different line of the
/// same JSON object and reading it would be prose-parsing. So the granularity
/// is the address: an address the template opened for SOME declared lane is an
/// address the template opened.
fn boundary_admits_a_connect_point(
    config: &std::path::Path,
    seal: &SealedHive,
    child: &str,
) -> bool {
    let Some(dir) = config.parent() else {
        return false;
    };
    let Some(contract) = contract_from_cell_dir(dir, HIVE) else {
        return false;
    };
    let want = format!("./{child}");
    let lanes: Vec<String> = contract
        .accepts
        .iter()
        .chain(contract.emits.iter())
        .filter(|l| l.at.iter().any(|a| a == &want))
        .map(|l| l.route.clone())
        .collect();
    lanes.iter().any(|lane| {
        let diff = json!({"add_edges": [
            {"from": CALLER, "to": format!("{HIVE}/{child}"), "lane": lane}]});
        let mut rej = MutationRejection::new();
        collect_hive_port_boundary(
            &diff,
            "/",
            std::slice::from_ref(seal),
            std::slice::from_ref(&contract),
            &mut rej,
        );
        rej.is_empty()
    })
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

/// The archived wave plans — frozen history, never scanned (see the header).
const FROZEN_SUBTREE: &str = "docs/superpowers/plans";

/// Every shipped `.md` and `.json` under `templates/`, `examples/` and `docs/`.
///
/// `builder-librarian`'s seed is excluded: it is a GENERATED corpus that embeds
/// other files verbatim, so a finding there is a duplicate of the finding in the
/// source, reported against a file nobody edits by hand. `docs/archive/` is
/// excluded for the opposite reason: it is a record of a past state, and a past
/// state is allowed to contain the addresses of its own day. The wave-plan
/// archive [`FROZEN_SUBTREE`] is excluded for that same second reason (R7 —
/// see the header).
///
/// **The sweep is by directory, never by filename, because the tree it runs in
/// has two shapes.** In the working tree a document exists twice — `X.md`
/// (German) and `X.en.md` (English) — and only the English one travels, renamed
/// to `X.md` on the way out. A list of expected filenames would therefore be
/// wrong in one of the two trees; walking whatever `.md` is present is right in
/// both. Missing directories are skipped in silence for the same reason
/// `collect_docs` already skips an unreadable one: the public tree carries a
/// subset, and a subset is not a defect.
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
            if !matches!(name.as_str(), "builder-librarian" | "archive") {
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

// ────────────────────────────────── the carve-out: addresses that are wrong on purpose

/// What a document writes above a fenced block to declare it a counter-example.
/// The value after `=` is a comma-separated list of the endpoints the document
/// claims the boundary refuses; the closing `-->` ends it.
const COUNTER_EXAMPLE_MARKER: &str = "<!-- gate:counter-example refused=";

/// One fenced block a document declares to be an intentional counter-example.
struct CounterExample {
    /// Where the marker sits, for the failure message.
    marker_line: usize,
    /// First and last line of the fenced block it guards, 1-based, fences
    /// included. `None` when no fence opens on the very next line — a marker
    /// that guards nothing exempts nothing and is reported.
    block: Option<(usize, usize)>,
    /// The endpoints the document CLAIMS the boundary refuses in that block.
    declared: Vec<String>,
}

/// Read every counter-example marker in one file.
///
/// Deliberately positional: the fence has to open on the line immediately after
/// the marker. A marker that could float above a paragraph would be an exemption
/// whose reach a reader has to guess at, and the reach is the whole point.
fn counter_examples_in(text: &str) -> Vec<CounterExample> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        let Some(rest) = t.strip_prefix(COUNTER_EXAMPLE_MARKER) else {
            continue;
        };
        let declared: Vec<String> = rest
            .trim_end()
            .trim_end_matches("-->")
            .split(',')
            .map(|d| d.trim().to_string())
            .filter(|d| !d.is_empty())
            .collect();
        let block = lines
            .get(i + 1)
            .filter(|l| l.trim_start().starts_with("```"))
            .and_then(|_| {
                lines[i + 2..]
                    .iter()
                    .position(|l| l.trim_start().starts_with("```"))
                    .map(|off| (i + 2, i + 2 + off + 1))
            });
        out.push(CounterExample {
            marker_line: i + 1,
            block,
            declared,
        });
    }
    out
}

// ─────────────────────────────────────────────────────────────── the check

/// Every documented endpoint in `docs` that the boundary refuses, as one
/// sentence each. `scanned` counts the endpoints looked at, so a caller can
/// tell "nothing wrong" from "nothing read".
fn findings(docs: &[(String, String)], scanned: &mut usize) -> Vec<String> {
    let sealed = sealed_library();
    assert!(
        sealed.len() >= 8,
        "the sweep found almost no sealed hive templates: {}",
        sealed.len()
    );
    // The substrate is asked once per template, not once per endpoint.
    let mut seals: HashMap<String, SealedHive> = HashMap::new();
    let mut out = Vec::new();
    for (file, text) in docs {
        let marks = counter_examples_in(text);
        // Per marker: the endpoints the REAL boundary refused inside its block.
        let mut refused_inside: Vec<Vec<String>> = vec![Vec::new(); marks.len()];
        for ep in endpoints_in(file, text) {
            *scanned += 1;
            let guarded = marks.iter().position(|m| {
                m.block
                    .is_some_and(|(from, to)| ep.line >= from && ep.line <= to)
            });
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
                let Some((t, naming)) = sealed.resolve(name, file) else {
                    continue;
                };
                let seal = seals
                    .entry(t.rel.clone())
                    .or_insert_with(|| seal_the_substrate_reads(&t.config));
                if boundary_admits(seal, child)
                    || boundary_admits_a_connect_point(&t.config, seal, child)
                {
                    continue;
                }
                if let Some(k) = guarded {
                    // Taught on purpose — recorded rather than accused, and
                    // held against what the marker says a few lines below.
                    if !refused_inside[k].contains(&ep.raw) {
                        refused_inside[k].push(ep.raw.clone());
                    }
                    continue;
                }
                let ports = if seal.ports.is_empty() {
                    "none — the hive path itself is the only address".to_string()
                } else {
                    seal.ports.join(", ")
                };
                // A convention match says so, because it is a reading of the
                // name rather than the name itself (GH #238).
                let via = match naming {
                    Naming::Own => String::new(),
                    Naming::Convention => format!(
                        " ('{name}' is the shortened instance name this template's own recipes \
                         write for themselves)"
                    ),
                };
                out.push(format!(
                    "{}:{}: '{}' reaches '{name}/{child}'{via}, and the boundary of the sealed \
                     template '{}' refuses it. Declared ports: {ports}. Write the hive path \
                     and put the meaning of the dropped segment on the edge's lane.",
                    ep.file, ep.line, ep.raw, t.rel
                ));
            }
        }
        for (mark, mut refused) in marks.into_iter().zip(refused_inside) {
            let at = format!("{file}:{}", mark.marker_line);
            if mark.block.is_none() {
                out.push(format!(
                    "{at}: a counter-example marker that no fenced block opens under. It exempts \
                     nothing and states nothing; put it on the line directly above the fence, or \
                     remove it."
                ));
                continue;
            }
            if mark.declared.is_empty() {
                out.push(format!(
                    "{at}: a counter-example marker that names no refused address. The marker is \
                     an assertion about the boundary, not a mute button — name what it refuses, \
                     or drop the marker and fix the block."
                ));
                continue;
            }
            let mut declared = mark.declared.clone();
            declared.sort();
            declared.dedup();
            refused.sort();
            if declared != refused {
                out.push(format!(
                    "{at}: the counter-example claims the boundary refuses {declared:?}, and it \
                     refuses {refused:?}. An address that stopped being refused is no longer a \
                     counter-example — it is a correct example labelled as wrong."
                ));
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
    // And once more for `docs/` alone, because the whole of GH #229 is that a
    // directory can be absent from the sweep while the total still looks
    // healthy. The floor is set for the SMALLER of the two trees: the public
    // one carries a curated subset of `docs/` and no language twins.
    let in_docs: usize = shipped_docs()
        .iter()
        .filter(|(f, _)| f.starts_with("docs/"))
        .map(|(f, t)| endpoints_in(f, t).len())
        .sum();
    assert!(
        in_docs >= 20,
        "the sweep read almost no endpoints in docs/: {in_docs}"
    );
}

/// The archived wave plans are out of the sweep, and stay out.
///
/// See the header: `docs/superpowers/plans/` is historical by the authority
/// hierarchy, and a gate that held a frozen plan to today's addresses was
/// rewriting history rather than checking it. The scan therefore has to come
/// back from there empty-handed.
///
/// **Two trees, and only one of them has an archive.** The export carries
/// `crates/` wholesale but pulls `docs/` through an explicit map that lists no
/// `docs/superpowers/*`, so this file ships into a clone where the archive does
/// not exist. Demanding it there would be the very thing `shipped_docs` refuses
/// to do above — "missing directories are skipped in silence … a subset is not
/// a defect". The private tree is recognised by root `plans/`, the directory
/// that never travels (the marker `gh80_shipped_conditions_are_guarded` uses,
/// because a marker on an allow-list can be promoted and a forbidden prefix
/// cannot).
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
         made to fail over an address that was current when it was written:\n  {}",
        leaked.join("\n  ")
    );
}

/// The test of the carve-out: it exempts by asserting, not by silencing.
///
/// Four synthetic documents around the same two lines the specification teaches
/// with. They pin the decision behind the marker (GH #229): a marker that only
/// said "skip this block" would pass the first three of these identically, and
/// the third is exactly the day the lesson stopped being true.
#[test]
fn the_marker_asserts_the_refusal_it_exempts() {
    let bad = "{\"from\": \"./proxy\", \"to\": \"./talky/session-keeper/stamp\"}";
    let case = |body: &str| {
        let mut scanned = 0usize;
        findings(&[("docs/x.md".to_string(), body.to_string())], &mut scanned)
    };

    // 1. Unmarked, the block is an accusation — the state GH #229 was filed in.
    //
    // Two accusations for one address since GH #228 sealed the whole library:
    // `./talky/session-keeper/stamp` crosses TWO boundaries, `talky`'s and the
    // sub-unit's, and each seal reports the crossing it sees. The address is
    // what the marker names, so exempting it (case 2) still silences both.
    let plain = case(&format!("```json\n{bad}\n```"));
    assert_eq!(plain.len(), 2, "{plain:#?}");
    assert!(
        plain.iter().all(|f| f.contains("session-keeper/stamp")),
        "{plain:#?}"
    );

    // 2. Marked with what the boundary really refuses: taught, not accused.
    let taught = case(&format!(
        "<!-- gate:counter-example refused=./talky/session-keeper/stamp -->\n```json\n{bad}\n```"
    ));
    assert!(taught.is_empty(), "{taught:#?}");

    // 3. The point of naming the address. A declaration the boundary no longer
    //    backs is louder than the silence a block-level mute would have kept.
    let stale = case(&format!(
        "<!-- gate:counter-example refused=./talky/session-keeper/nowhere -->\n```json\n{bad}\n```"
    ));
    assert_eq!(stale.len(), 1, "{stale:#?}");
    assert!(
        stale[0].contains("no longer a counter-example"),
        "{stale:#?}"
    );

    // 4. The reach is one fenced block, and a marker that guards none says so.
    let loose = case(&format!(
        "<!-- gate:counter-example refused=./a/b -->\n\n```json\n{bad}\n```"
    ));
    assert_eq!(loose.len(), 3, "{loose:#?}");
    assert!(
        loose
            .iter()
            .any(|f| f.contains("no fenced block opens under")),
        "{loose:#?}"
    );
}

/// The marker is a carve-out, and a carve-out that spreads is a mode.
///
/// Nothing about the scan limits how often a block may declare itself wrong on
/// purpose, so the limit is stated here: a handful across the whole shipped
/// tree. The number is a handful rather than one because a document exists
/// twice in the working tree — German and English — and each twin carries its
/// own marker; the public tree sees half of them.
///
/// A marker also has to earn its place twice over, and both halves are checked
/// by the sweep itself: it must guard a fenced block, and the addresses it
/// names must be the ones the real boundary refuses.
#[test]
fn the_counter_example_marker_stays_a_carve_out() {
    let docs = shipped_docs();
    let marked: Vec<String> = docs
        .iter()
        .flat_map(|(file, text)| {
            counter_examples_in(text)
                .into_iter()
                .map(move |m| format!("{file}:{}", m.marker_line))
        })
        .collect();
    assert!(
        marked.len() <= 4,
        "counter-example markers stopped being exceptional ({}): {marked:#?}",
        marked.len()
    );
    // Zero would mean the marker is dead code and the section that teaches by
    // counter-example lost its lesson, which is a finding of its own.
    assert!(
        !marked.is_empty(),
        "no shipped document teaches by counter-example any more — if the pair \
         in § the hive boundary was removed, remove the marker support with it"
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
    // Since GH #228 the whole library is sealed, so an address that names a hive
    // and then keeps going crosses more than one boundary, and each seal reports
    // the crossing it sees — `<talky>/collector/assemble` is two findings on its
    // own. The per-address assertions below are what this test is really made
    // of; the count only keeps it from going quiet.
    //
    // The count is a LITERAL again, and that is the point of GH #238. It briefly
    // had to be derived, because `./agent/...` in the first line resolved to the
    // template named `slack-agent` on its last segment — a finding that existed
    // in the private tree and not in the public one, so a literal passed here and
    // failed in the public clone. Since the tail is read only inside a template's
    // own documents, `templates/README.md` no longer names `slack-agent` at all
    // and both trees count the same.
    assert_eq!(
        found.len(),
        7,
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

/// GH #238 — the shortened instance name is read inside its own template and
/// nowhere else.
///
/// Three synthetic documents around one address. They pin the decision, not the
/// outcome: a rule that simply dropped the tail alias would pass the second and
/// third of these and fail the first, and a rule that kept it globally would
/// pass the first and fail the other two.
#[test]
fn the_instance_name_convention_only_reads_inside_its_own_template() {
    let own = r#"wired as {to: <drain>/drain, condition:"#.to_string();
    let mut scanned = 0usize;

    // 1. The template's own recipe, where `<drain>` IS `memory-drain`.
    let found = findings(
        &[(
            "templates/memory-drain/template.json".to_string(),
            own.clone(),
        )],
        &mut scanned,
    );
    assert_eq!(
        found.len(),
        1,
        "the template's own recipe went unread: {found:#?}"
    );
    assert!(
        found[0].contains("templates/memory-drain/config.json")
            && found[0].contains("shortened instance name"),
        "the finding neither blamed memory-drain nor said it read a convention: {found:#?}"
    );

    // 2. The same line in a foreign document, where `drain` is whatever that
    //    document's reader named their own hive.
    let found = findings(&[("docs/config.md".to_string(), own)], &mut scanned);
    assert!(
        found.is_empty(),
        "a foreign document was blamed through another template's shortened name: {found:#?}"
    );

    // 3. The line this issue was filed from: `agent` is a placeholder, and the
    //    only template it ever resolved to (`slack-agent`) is private, so this
    //    assertion is the one that used to hold in one tree and not the other.
    let found = findings(
        &[(
            "templates/README.md".to_string(),
            r#"  "add_edges":[{"from":"./ingress","to":"./agent/session-keeper/stamp"}]"#
                .to_string(),
        )],
        &mut scanned,
    );
    assert_eq!(
        found.len(),
        1,
        "the placeholder `agent` was resolved to a template again: {found:#?}"
    );
    assert!(
        found[0].contains("session-keeper/stamp"),
        "the one finding is the real one — the crossing into session-keeper: {found:#?}"
    );
}
