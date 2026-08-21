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
//! # Declared debt, asserted rather than muted
//!
//! Three further templates carry the same defect and are out of #311's scope
//! ([#337](https://github.com/mmeyerlein/meclaw/issues/337)). They are listed
//! below by name, and `the_declared_debt_is_still_debt` requires each one that
//! ships in this tree to STILL produce a finding — the same rule the #229
//! counter-example marker follows. A template that gets fixed and stays on the
//! list fails here, so the list cannot rot into a silence.

use meclaw_colony::config::HiveParams;
use meclaw_colony::mutation::port_boundary::validate_hive_port_boundary;
use meclaw_colony::mutation::port_boundary::{SealedHive, collect_sealed_hives};
use meclaw_core::serde_json::{Value, json};

/// Where the synthetic hive lives while it is being checked, and a caller
/// outside it. Any paths do; what matters is that "outside" really is.
const HIVE: &str = "/h";
const CALLER: &str = "/caller";

/// Templates known to carry this defect and tracked elsewhere. Each entry is an
/// assertion, not a mute: see `the_declared_debt_is_still_debt`.
const DECLARED_DEBT: &[&str] = &["affinity", "llm-unit", "talky"];

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
        if DECLARED_DEBT.contains(&c.name.as_str()) {
            continue;
        }
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
    // subset of the library (7 slots and 18 interior addresses reach this sweep
    // there, 8 and 21 here), and a subset is not a defect.
    assert!(
        counts.slots >= 6,
        "the scan read almost no slots: {}",
        counts.slots
    );
    assert!(
        counts.refused_children >= 15,
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

/// The declared debt is an assertion about the tree, not a mute button.
///
/// A template that is fixed and stays on the list fails here, which is what
/// keeps the list from rotting into a silence. A template that does not ship in
/// this tree is skipped — the public clone carries a subset of the library
/// (`affinity` and `llm-unit` are private), and a subset is not a defect.
#[test]
fn the_declared_debt_is_still_debt() {
    let present: Vec<Candidate> = candidates()
        .into_iter()
        .filter(|c| DECLARED_DEBT.contains(&c.name.as_str()))
        .collect();
    assert!(
        !present.is_empty(),
        "not one declared-debt template ships here — the list has lost its subject"
    );
    for c in &present {
        let mut counts = Counts::default();
        let found = findings_for(c, &mut counts);
        assert!(
            !found.is_empty(),
            "'{}' is on the declared-debt list of GH #337 and no longer produces a finding. \
             Take it off the list in the same commit that fixed it.",
            c.name
        );
    }
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
