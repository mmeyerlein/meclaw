//! GH #311 — a `PORTS` slot names an address the boundary admits.
//!
//! `gh203_documented_port_addresses` reads the literal `from:` / `to:` keys of
//! a shipped document and says so in its own header: *"a ports TABLE row or a
//! sentence naming an address is not distinguishable from prose ABOUT an
//! address"*. #311 is exactly the form it left out. `collector`'s
//! `template.json` opened its port recipe with **"Entry (into ./assemble …)"**
//! and `session-keeper`'s with **"Entry (into ./stamp …)"** — no `from:`, no
//! `to:`, nothing for that scan to read, and both hives sealed with
//! `params.ports: []`, so every address the two slots offered is refused with
//! `hive_port_boundary`. The refusal bites precisely where the slot sends the
//! reader, because the same slot tells them to wire the ports in the mutation
//! that instantiates the hive — and the seal guards a mutation's `add_edges`.
//!
//! # What is scanned, and why this one CAN be read
//!
//! Not prose in general — one slot. `templates/README.md` § Versioning presents
//! the `PORTS` entry of `template.json`'s `description.examples[]` as the
//! caller's interface, and that entry has a shape: it opens with the literal
//! token `PORTS` and it lists endpoints. Inside it, a `./<name>` whose `<name>`
//! is a direct child of the template's own directory is an ADDRESS — there is
//! nothing else it could be, because the slot's subject is where a parent wires.
//! Everything outside that slot stays out of scope: a README sentence, a table
//! row, a paragraph about the hive's internal chain all write `./<child>` for
//! good reasons, and a check that guessed there would be a check people learn
//! to work around (the measurement is in the task receipt: the same pattern over
//! whole READMEs is ~100 hits, nearly all of them legitimate internal prose).
//!
//! # The question is asked of the substrate
//!
//! Same discipline as `gh196_shipped_hive_ports`, `gh202_shipped_drain_requirements`
//! and `gh203_documented_port_addresses`: the template's own `config.json` is
//! planted in a throwaway colony root, read by the REAL `collect_sealed_hives`,
//! and each candidate address is put through the REAL
//! `validate_hive_port_boundary` as an edge coming from outside. A template that
//! declares real ports therefore reports nothing about them, and a check that
//! re-derived "what a port looks like" could agree with itself while disagreeing
//! with the colony.
//!
//! # The one carve-out: a sentence that names the seal
//!
//! A correctly written slot names the refused addresses on purpose —
//! `firewall`: *"an edge naming ./screen is refused with `hive_port_boundary`"*;
//! `llm-registry`: the same sentence over three names, plus *"a parent's
//! `params.graph` edge straight into ./store maintains them, which the hive port
//! seal does not touch because the bootstrap is deliberately outside its scope"*
//! (true: the seal reads a mutation's `add_edges` and never the birth topology).
//! So an occurrence is exempt when the sentence it stands in names the seal
//! itself — `hive_port_boundary`, `params.graph` or `boot graph`. That is the
//! distinction #203 drew and could not act on, made decidable by narrowing the
//! surface: inside a PORTS slot, a sentence that names the seal IS prose about
//! an address, and every other one is a wiring instruction.
//!
//! # History: the declared debt, and why there is no list here any more
//!
//! Three further templates carried the same defect out of #311's scope and were
//! carried on a named list that asserted the finding rather than muting it
//! ([#337](https://github.com/mmeyerlein/meclaw/issues/337)); all three are
//! fixed, so the list and its assertion are gone with their subject and the
//! sweep below reads every shipped slot.
//!
//! # The second surface: a README's fenced wiring example
//!
//! Closing that debt turned up the same defect one surface over
//! (`templates/dispatcher/README.md` offered `./collect/assemble` three times, in
//! copy-paste-ready JSON). A fenced block is readable for the same reason the
//! slot is, and for a sharper one: inside it, `"from"` and `"to"` are the
//! substrate's own keys, so a `./<parent>/<child>` there IS an `add_edges`
//! endpoint and cannot be prose about one. Outside a fence it is a sentence, and
//! that is the ~100-hit ambiguity named above. The rule, the sealed-hive index
//! (`candidates()`) and the carve-out (`names_the_seal`) are shared; only the
//! reader differs, and the prose unit the carve-out reads is the text since the
//! previous fence or heading instead of the sentence.
//!
//! Measured over the whole shipped library on 2026-08-21, the README rule yielded
//! six hits: the three dispatcher lines (fixed with this file) and three in
//! `templates/builder-hive/README.md`, which quoted a boot-time scaffold verbatim
//! under prose that names `hive_port_boundary` and were therefore exempt. Zero
//! others, zero false positives. That template was retired on 2026-08-27
//! (GH #434), so the README sweep now finds no exempt hit at all; the carve-out
//! stays because it is shared with the PORTS sweep, where `firewall` exercises
//! it under a floor.

use meclaw_colony::config::HiveParams;
use meclaw_colony::mutation::port_boundary::validate_hive_port_boundary;
use meclaw_colony::mutation::port_boundary::{SealedHive, collect_sealed_hives};
use meclaw_core::serde_json::{Value, json};

/// Where the synthetic hive lives while it is being checked, and a caller
/// outside it. Any paths do; what matters is that "outside" really is.
const HIVE: &str = "/h";
const CALLER: &str = "/caller";

/// A sentence naming one of these is prose ABOUT the boundary, not a wiring
/// instruction. Compared case-insensitively.
const NAMES_THE_SEAL: &[&str] = &["hive_port_boundary", "params.graph", "boot graph"];

fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

// ───────────────────────────────────────────────── the shipped candidates

/// One shipped template whose root `config.json` is a hive that declared
/// `params.ports`.
struct Candidate {
    /// Directory basename under `templates/`, for the failure message.
    name: String,
    /// Its `config.json`, read by the substrate on demand.
    config: std::path::PathBuf,
    /// The short names of its direct children — the segments a slot could name.
    children: Vec<String>,
    /// Its `PORTS` slot, when it has one.
    slot: Option<String>,
}

/// Every template directory directly under `templates/` whose own
/// `config.json` is a sealed hive. Sub-unit copies inside a composite carry no
/// `template.json` and therefore no slot; they are skipped by construction.
fn candidates() -> Vec<Candidate> {
    let templates = core_root().join("templates");
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&templates) else {
        return out;
    };
    let mut dirs: Vec<std::path::PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs {
        let config = dir.join("config.json");
        let Ok(raw) = std::fs::read_to_string(&config) else {
            continue;
        };
        let Ok(val) = meclaw_core::serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        if val
            .get("cell")
            .and_then(|c| c.get("type"))
            .and_then(Value::as_str)
            != Some("hive")
        {
            continue;
        }
        let params = val.get("params").cloned().unwrap_or(Value::Null);
        if params.is_null() {
            continue;
        }
        let Ok(hp) = meclaw_core::serde_json::from_value::<HiveParams>(params) else {
            continue;
        };
        if hp.ports.is_none() {
            continue; // key absent is the OPEN state — nothing is sealed here
        }
        let mut children: Vec<String> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        children.sort();
        out.push(Candidate {
            name: dir.file_name().unwrap().to_string_lossy().into_owned(),
            config,
            children,
            slot: ports_slot(&dir.join("template.json")),
        });
    }
    out
}

/// The `PORTS` entry of a `template.json`'s `description.examples[]`.
///
/// Recognised by the literal token the shipped library writes it with — the
/// entry begins with `PORTS`. A template that has no such entry contributes
/// nothing rather than being judged on its prose.
fn ports_slot(template_json: &std::path::Path) -> Option<String> {
    let raw = std::fs::read_to_string(template_json).ok()?;
    let val: Value = meclaw_core::serde_json::from_str(&raw).ok()?;
    val.get("description")?
        .get("examples")?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .find(|e| e.trim_start().starts_with("PORTS"))
        .map(str::to_string)
}

/// Plant one template's `config.json` as the hive `/h` of a throwaway colony
/// root and let the substrate's own reader say what it seals.
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

// ─────────────────────────────────────────────────────── reading the slot

/// Split a slot into sentences: a `.` or `;` followed by whitespace ends one.
///
/// `hop.route`, `params.ports` and `1.1.0` survive because the separator needs
/// the space; the shipped slots are one long paragraph each, so this is the
/// only boundary available and it is enough to keep a disclaiming clause from
/// covering the whole slot.
fn sentences(text: &str) -> Vec<&str> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut start = 0usize;
    for i in 0..b.len() {
        if !matches!(b[i], b'.' | b';') {
            continue;
        }
        match b.get(i + 1) {
            Some(c) if c.is_ascii_whitespace() => {}
            _ => continue,
        }
        out.push(&text[start..=i]);
        start = i + 1;
    }
    if start < b.len() {
        out.push(&text[start..]);
    }
    out
}

/// True when this sentence is prose ABOUT the boundary rather than an
/// instruction to wire past it.
fn names_the_seal(sentence: &str) -> bool {
    let lower = sentence.to_ascii_lowercase();
    NAMES_THE_SEAL.iter().any(|k| lower.contains(k))
}

/// `./<child>` as a standalone token: not part of a longer path segment and not
/// the tail of one (`<x>/./y` cannot occur, but `./assembler` must not match
/// `./assemble`).
fn addresses_in<'a>(sentence: &'a str, child: &str) -> Vec<&'a str> {
    let needle = format!("./{child}");
    let b = sentence.as_bytes();
    let n = needle.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + n.len() <= b.len() {
        if &b[i..i + n.len()] != n {
            i += 1;
            continue;
        }
        let left_ok = i == 0 || !is_path_byte(b[i - 1]);
        // A trailing `.` ends the sentence unless a segment continues after it,
        // so `./store.` is the address and `./store.db` is not; a `/` after the
        // name makes it a DEEP endpoint, which is `gh203`'s subject and not
        // this one's.
        let right_ok = match b.get(i + n.len()) {
            None => true,
            Some(b'/') => false,
            Some(b'.') => b
                .get(i + n.len() + 1)
                .is_none_or(|c| !c.is_ascii_alphanumeric()),
            Some(c) => !c.is_ascii_alphanumeric() && !matches!(c, b'-' | b'_'),
        };
        if left_ok && right_ok {
            out.push(&sentence[i..i + n.len()]);
        }
        i += 1;
    }
    out
}

fn is_path_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.')
}

// ───────────────────────────────────────────────────────────── the check

/// What the scan looked at, so "nothing wrong" can be told from "nothing read".
#[derive(Default)]
struct Counts {
    slots: usize,
    refused_children: usize,
    exempted: usize,
}

/// One template's findings, as one sentence each.
fn findings_for(c: &Candidate, counts: &mut Counts) -> Vec<String> {
    let Some(slot) = c.slot.as_deref() else {
        return Vec::new();
    };
    counts.slots += 1;
    let seal = seal_the_substrate_reads(&c.config);
    let refused: Vec<&String> = c
        .children
        .iter()
        .filter(|ch| !boundary_admits(&seal, ch))
        .collect();
    counts.refused_children += refused.len();
    let ports = if seal.ports.is_empty() {
        "none — the hive path itself is the only address".to_string()
    } else {
        seal.ports.join(", ")
    };
    let mut out = Vec::new();
    for sentence in sentences(slot) {
        let exempt = names_the_seal(sentence);
        for child in &refused {
            for hit in addresses_in(sentence, child) {
                if exempt {
                    counts.exempted += 1;
                    continue;
                }
                out.push(format!(
                    "templates/{}/template.json: the PORTS slot offers '{hit}' as an endpoint, and \
                     the boundary of this sealed hive refuses it. Declared ports: {ports}. Write \
                     the hive path and put the meaning of the dropped segment on the edge's lane. \
                     In: …{}…",
                    c.name,
                    excerpt(sentence, hit)
                ));
            }
        }
    }
    out
}

/// A short window around the hit, for a message that can be acted on.
fn excerpt<'a>(sentence: &'a str, hit: &str) -> &'a str {
    let at = sentence.find(hit).unwrap_or(0);
    let from = sentence[..at]
        .char_indices()
        .rev()
        .take(45)
        .last()
        .map(|(i, _)| i)
        .unwrap_or(0);
    let to = sentence[at..]
        .char_indices()
        .take(60)
        .last()
        .map(|(i, ch)| at + i + ch.len_utf8())
        .unwrap_or(sentence.len());
    &sentence[from..to]
}

// ───────────────────────────────────────────────────────────── the tests

#[test]
fn every_ports_slot_addresses_the_hive_it_seals() {
    let mut counts = Counts::default();
    let mut found = Vec::new();
    for c in candidates() {
        found.extend(findings_for(&c, &mut counts));
    }
    assert!(
        found.is_empty(),
        "a shipped PORTS slot wires past its own hive boundary:\n  {}",
        found.join("\n  ")
    );
    // "Nothing wrong" and "nothing read" look identical from the outside, and
    // the second is the failure mode of a scan this narrow. The floors are set
    // for the SMALLER of the two trees: the public clone carries an allow-list
    // subset of the library (8 slots and 24 interior addresses reach this sweep
    // there, 11 and 38 here — measured 2026-08-21, after the last declared-debt
    // template was fixed and the whole library became reachable), and a subset
    // is not a defect.
    assert!(
        counts.slots >= 7,
        "the scan read almost no slots: {}",
        counts.slots
    );
    assert!(
        counts.refused_children >= 20,
        "the scan resolved almost no interior addresses: {}",
        counts.refused_children
    );
    // The carve-out is exercised by the shipped tree — `firewall` names its own
    // refused address on purpose, and it travels into the public tree too. Zero
    // here would mean the exemption is dead code and the next correctly written
    // slot gets accused.
    assert!(
        counts.exempted >= 1,
        "no shipped slot names the seal any more — if the sentence that teaches \
         the refusal was removed, remove the exemption with it"
    );
}

/// The test of the test: the exact texts #311 was filed about, verbatim from the
/// state they shipped in.
///
/// A fixed tree contains, by construction, no PORTS slot that offers an interior
/// address, so the sweep above resolves nothing and would be green whether it
/// worked or not.
#[test]
fn the_scan_reports_the_slots_this_issue_was_filed_about() {
    let shipped_before_the_fix = [
        (
            "collector",
            "PORTS - Entry (into ./assemble, the parent edge names the lane via set_hop route \
             and promotes session_id): in_turn (an inbound user turn). Exit (from ./assemble, \
             on hop.route): 'brain' -> the agent LLM.",
            vec!["./assemble", "./assemble"],
        ),
        (
            "session-keeper",
            "PORTS - Entry (into ./stamp, the parent edge names the lane with set_hop route): \
             in_turn (an inbound turn from the surface). Optional second entry (into ./close): \
             in_sweep. Exit (on hop.route): 'turn' from ./stamp -> the context assembly, \
             'close' from ./close -> the consumer of a finished session.",
            vec!["./close", "./close", "./stamp", "./stamp"],
        ),
    ];
    for (name, slot, expected) in shipped_before_the_fix {
        let c = shipped_candidate(name, slot);
        let mut counts = Counts::default();
        let found = findings_for(&c, &mut counts);
        let mut hits: Vec<String> = found
            .iter()
            .map(|f| {
                let at = f.find("offers '").expect("the finding quotes the address") + 8;
                f[at..f[at..].find('\'').unwrap() + at].to_string()
            })
            .collect();
        hits.sort();
        assert_eq!(
            hits, expected,
            "{name}: the scan missed a known-bad address (or invented one): {found:#?}"
        );
        assert!(
            found
                .iter()
                .all(|f| f.contains(&format!("templates/{name}/template.json"))),
            "{name}: a finding blamed the wrong file: {found:#?}"
        );
    }
}

/// The carve-out exempts by naming the seal, and it exempts a SENTENCE.
///
/// Two synthetic slots around one address. A rule that simply dropped any slot
/// mentioning the boundary would pass both; a rule with no carve-out at all
/// would fail both.
#[test]
fn a_sentence_that_names_the_seal_is_prose_about_an_address() {
    let name = "collector";

    // 1. The correctly written form: the refusal is stated, the address is not
    //    offered. `firewall` and `llm-registry` write exactly this.
    let taught = "PORTS - every endpoint is the HIVE path (`params.ports` is empty since GH \
                  #228; an edge naming ./assemble is refused with hive_port_boundary), and what \
                  a caller wants rides on hop.route.";
    let mut counts = Counts::default();
    let found = findings_for(&shipped_candidate(name, taught), &mut counts);
    assert!(found.is_empty(), "{found:#?}");
    assert_eq!(counts.exempted, 1, "the exemption was not the reason");

    // 2. The same disclaimer followed by a wiring instruction in its own
    //    sentence. The exemption must not spread past the sentence that earned
    //    it — that is the difference between a carve-out and a mode.
    let mixed = format!("{taught} Entry (into ./assemble): in_turn.");
    let mut counts = Counts::default();
    let found = findings_for(&shipped_candidate(name, &mixed), &mut counts);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].contains("./assemble"), "{found:#?}");
    assert_eq!(counts.exempted, 1, "the first sentence still earned it");
}

/// A template may name a REAL port, and the substrate is what decides that.
///
/// Without this, the check would be indistinguishable from "never write
/// `./<child>` in a PORTS slot", which is wrong for every hive that declares
/// ports.
#[test]
fn a_declared_port_is_an_address_and_is_not_reported() {
    let td = tempfile::TempDir::new().unwrap();
    let dir = td.path().join("half-open");
    std::fs::create_dir_all(dir.join("brief")).unwrap();
    std::fs::create_dir_all(dir.join("store")).unwrap();
    std::fs::write(
        dir.join("config.json"),
        r#"{"cell":{"type":"hive"},"params":{"ports":["brief"]}}"#,
    )
    .unwrap();
    let c = Candidate {
        name: "half-open".to_string(),
        config: dir.join("config.json"),
        children: vec!["brief".to_string(), "store".to_string()],
        slot: Some("PORTS - Entry: ./brief. Maintenance: ./store.".to_string()),
    };
    let mut counts = Counts::default();
    let found = findings_for(&c, &mut counts);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0].contains("./store") && found[0].contains("brief"),
        "the finding must name the offered address and the ports that exist: {found:#?}"
    );
}

// ──────────────────────────── the second surface: a README wiring example

// The `PORTS` slot is the caller's interface, but it is not the only place the
// shipped library hands a reader an edge to copy. A README's fenced JSON block
// is the other one, and it is worse: it is copy-paste ready. The rule is the
// same rule — an `add_edges` endpoint that names a cell INSIDE a sealed hive is
// refused by `validate_hive_port_boundary` — read off a different surface, and
// the surface is what makes it decidable. Inside a fence, `"from"` and `"to"`
// are the substrate's own keys; a `./<parent>/<child>` there is an address and
// cannot be prose about one. Outside a fence the same string is a sentence, and
// that is the ~100-hit ambiguity the module header describes.
//
// Both ends are read, because the boundary reads both
// (`crates/meclaw-colony/src/mutation/port_boundary.rs`): a reply lane wired
// straight out of an interior cell bypasses the port exactly as an inbound one
// does.

/// One `"from"` / `"to"` value of the deep form `./<parent>/<child>` that stands
/// inside a fenced block of a shipped README.
struct ReadmeEndpoint {
    /// Directory basename under `templates/`, for attribution.
    template: String,
    /// Path below `templates/`, for the failure message.
    file: String,
    /// 1-indexed line of the value.
    line: usize,
    /// The matched address, exactly as written.
    address: String,
    /// Its last segment — the one the boundary would judge.
    child: String,
    /// Whether the prose introducing this fence names the seal.
    exempt: bool,
}

/// What the README sweep looked at, so "nothing wrong" can be told from
/// "nothing read".
#[derive(Default)]
struct ReadmeCounts {
    files: usize,
    endpoints: usize,
}

/// Every `README.md` below `templates/`, at any depth.
fn shipped_readmes() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![core_root().join("templates")];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|n| n == "README.md") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// The `"from"` / `"to"` values on one line, in order.
///
/// Deliberately literal: these are the substrate's own keys, written the way a
/// mutation writes them. A line that does not carry the key contributes nothing.
fn wiring_values(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for (at, _) in line.match_indices('"') {
        let rest = &line[at..];
        let rest = match rest
            .strip_prefix("\"from\"")
            .or_else(|| rest.strip_prefix("\"to\""))
        {
            Some(r) => r,
            None => continue,
        };
        let Some(rest) = rest.trim_start().strip_prefix(':') else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix('"') else {
            continue;
        };
        let Some(end) = rest.find('"') else {
            continue;
        };
        out.push(&rest[..end]);
    }
    out
}

/// `./<parent>/<child>` at the head of a value, as the scan's pattern reads it:
/// two segments of `[A-Za-z0-9_-]`, and the second is what the boundary judges.
/// A deeper path contributes its first two segments, which is where the seal
/// already bites.
fn deep_endpoint(value: &str) -> Option<(String, String)> {
    let rest = value.strip_prefix("./")?;
    let ok = |s: &str| !s.is_empty() && s.bytes().all(is_path_segment_byte);
    let mut segs = rest.split('/');
    let parent = segs.next()?;
    let child = segs.next()?;
    if !ok(parent) || !ok(child) {
        return None;
    }
    Some((format!("./{parent}/{child}"), child.to_string()))
}

fn is_path_segment_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_')
}

/// Read every shipped README, fence by fence.
///
/// The prose that introduces a fence is the text since the previous fence or
/// heading — the same unit a reader takes the block's justification from, and
/// the README analogue of the slot's sentence.
fn readme_endpoints(counts: &mut ReadmeCounts) -> Vec<ReadmeEndpoint> {
    let root = core_root().join("templates");
    let mut out = Vec::new();
    for path in shipped_readmes() {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        counts.files += 1;
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        let template = rel.split('/').next().unwrap_or(&rel).to_string();
        let mut in_fence = false;
        let mut prose = String::new();
        let mut introducing = String::new();
        for (idx, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("```") {
                if !in_fence {
                    introducing = std::mem::take(&mut prose);
                }
                prose.clear();
                in_fence = !in_fence;
                continue;
            }
            if !in_fence {
                if line.starts_with('#') {
                    prose.clear();
                } else {
                    prose.push_str(line);
                    prose.push('\n');
                }
                continue;
            }
            for value in wiring_values(line) {
                counts.endpoints += 1;
                let Some((address, child)) = deep_endpoint(value) else {
                    continue;
                };
                out.push(ReadmeEndpoint {
                    template: template.clone(),
                    file: format!("templates/{rel}"),
                    line: idx + 1,
                    address,
                    child,
                    exempt: names_the_seal(&introducing),
                });
            }
        }
    }
    out
}

/// Every interior address of the shipped library: a child of a sealed hive that
/// the REAL boundary refuses, mapped to the hives that refuse it.
///
/// The index comes from `candidates()`, so the two surfaces of this file agree
/// on what "sealed" means, and the refusal comes from
/// `validate_hive_port_boundary`, so a hive that declares real ports keeps them.
fn interior_addresses() -> std::collections::BTreeMap<String, Vec<String>> {
    let mut out: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for c in candidates() {
        let seal = seal_the_substrate_reads(&c.config);
        for child in &c.children {
            if !boundary_admits(&seal, child) {
                out.entry(child.clone()).or_default().push(c.name.clone());
            }
        }
    }
    out
}

/// A fenced wiring example in a shipped README names the hive, not a cell inside
/// it — the second form of the same defect the `PORTS` sweep above catches.
#[test]
fn a_readme_wiring_example_addresses_the_hive_it_seals() {
    let interior = interior_addresses();
    let mut counts = ReadmeCounts::default();
    let mut findings = Vec::new();
    let mut exempted: std::collections::BTreeMap<String, usize> = Default::default();
    for ep in readme_endpoints(&mut counts) {
        let Some(hives) = interior.get(&ep.child) else {
            continue; // not a cell inside anything sealed
        };
        if ep.exempt {
            *exempted.entry(ep.template).or_default() += 1;
            continue;
        }
        findings.push(format!(
            "{}:{}: a fenced wiring example offers '{}' as an edge endpoint, and '{}' is an \
             interior cell of the sealed hive template {} — a mutation naming it is refused with \
             hive_port_boundary. Write the hive path and put the meaning of the dropped segment on \
             the edge's lane.",
            ep.file,
            ep.line,
            ep.address,
            ep.child,
            hives.join(" / ")
        ));
    }
    assert!(
        findings.is_empty(),
        "a shipped README hands the reader an edge its own hive boundary refuses:\n  {}",
        findings.join("\n  ")
    );
    // "Nothing wrong" and "nothing read" look identical from the outside. A tree
    // with every wiring example correct contains, by construction, ZERO deep
    // endpoints, so the floors have to sit on the reading, not on the findings:
    // measured 2026-08-21, 33 READMEs and 213 `from`/`to` values here, 19 and
    // 159 in the public clone's allow-list subset of the library.
    assert!(
        counts.files >= 15,
        "the sweep read almost no READMEs: {}",
        counts.files
    );
    assert!(
        counts.endpoints >= 100,
        "the sweep read almost no wiring endpoints: {}",
        counts.endpoints
    );
    // The carve-out has no witness in the README sweep any more, and that is a
    // measured state rather than an oversight: of the six hits the rule ever
    // yielded, three were the dispatcher lines this file fixed and three were in
    // `templates/builder-hive/README.md`, retired with its template (GH #434).
    // The exemption itself is NOT dead — `names_the_seal` is shared with the
    // PORTS sweep above, where `firewall` exercises it and a floor holds it
    // honest. So: no floor here, and the map is reported rather than asserted,
    // because a fresh exempt README must not have to touch this test to be
    // allowed.
    assert!(
        exempted.values().sum::<usize>() <= counts.endpoints,
        "the exemption counter outran the reading it counts (exemptions: {exempted:?})"
    );
}

/// A synthetic candidate carrying a shipped template's real seal and children,
/// with a slot text supplied by the test.
fn shipped_candidate(name: &str, slot: &str) -> Candidate {
    let dir = core_root().join("templates").join(name);
    let mut children: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    children.sort();
    Candidate {
        name: name.to_string(),
        config: dir.join("config.json"),
        children,
        slot: Some(slot.to_string()),
    }
}
