//! GH #531 — a declaration never changes its source silently.
//!
//! GH #464 replaced a hand-typed tool menu with an asked-for one. Nothing
//! checked that the new source delivered every name the old one had, and it did
//! not: the consult errands were seed rows in the brain's `cell.db`, the first
//! tick replaced the whole subtree, and the declarations were gone. GH #512
//! measured the same class one name over. In both cases the failure reached a
//! person the same way — the agent's charter kept naming a tool the menu no
//! longer carried, which reads to the model as a capability it has, and produces
//! a refusal it cannot explain.
//!
//! The defect is not that somebody forgot. It is that a declaration and the text
//! that names it had no gate between them. This file is that gate.
//!
//! # What it judges, and how the menu is derived
//!
//! For every shipped composite whose collector declares tools, the menu it can
//! carry is derivable from the tree without booting anything:
//!
//! * `params.tools` — the names the agent DECLARES it uses, layered through the
//!   ref chain, because a level may declare on behalf of the composite it
//!   references (`assistant` declares `consult_cogny` for its surface: the
//!   errand is the level's topology and standalone there is no core to consult);
//! * the names the collector serves ITSELF, under the two switches that already
//!   decide whether the lane is answered instead of refused (GH #512);
//! * `["*"]`, which asks for everything an answerer has and therefore declares
//!   no name at all.
//!
//! And an answerer is derivable too: the tool hive answers whatever its own
//! `schemas` cell answers for `["*"]`, and a composite answers the names its own
//! graph routes by an edge on `hop.tool_name` — which is the rule stated the
//! other way round. A tool the composite reaches by an edge on its NAME is
//! topology of the level that draws the edge, so the level both routes it and
//! must declare it.
//!
//! Three checks fall out, and each one is one half of a promise:
//!
//! 1. **A charter names nothing the menu cannot carry.** Every tool name that
//!    appears in a text a MODEL is given — a seed row under `instructions.*`,
//!    `identity.*` or `persona.*` — is on that composite's menu.
//! 2. **Every declared name has an answerer.** A name in `params.tools` that no
//!    answerer of that composite can deliver is #464 happening again.
//! 3. **Every routed name is declared.** A name the composite routes by an edge
//!    but does not declare is a wired lane the model can never ask for — which
//!    is what a grown agent measured as "my memory is not reachable".
//!
//! # The test of the test
//!
//! The sweep is green on the tree as it stands, so on its own it would be green
//! whether the comparison works or not. `a_fabricated_charter_is_reported` and
//! `a_fabricated_declaration_without_an_answerer_is_reported` put fabricated
//! input through the SAME functions the sweep uses — no file is touched.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_all, shipped_script};
use std::collections::{BTreeMap, BTreeSet};

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

fn read(rel: &str) -> Option<Value> {
    let raw = std::fs::read_to_string(templates_root().join(rel)).ok()?;
    meclaw_core::serde_json::from_str(&raw).ok()
}

const SCHEMAS_CELL: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/tools/schemas/config.json"
);

/// Every name the shipped tool hive has a declaration for, asked the way its own
/// hive asks — `["*"]` is the wildcard the door already understands, so this is
/// read off the cell rather than copied from it.
fn what_the_tool_hive_answers() -> BTreeSet<String> {
    let out = emit_all(
        &shipped_script(SCHEMAS_CELL),
        &json!({
            "target": "/main/tools/schemas",
            "header": {"hop": {"route": "in_schemas"}, "context": {}},
            "ttl": 64,
            "tools": ["*"],
            "messages": [],
        }),
    );
    out.first()
        .and_then(|a| a["schemas"].as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

// ────────────────────────────────────────────────── one composite, described

/// One shipped composite that carries a collector, and the two files that decide
/// what its brain is offered: the graph that routes names, and the layered
/// `assemble` params that declare them.
struct Composite {
    name: &'static str,
    /// The composite's own graph — where a routed name is read from.
    graph: &'static str,
    /// `assemble` param overrides, outermost LAST: a level speaks after the
    /// composite it references (`crates/meclaw-colony/src/mutation/subtree.rs`).
    layers: &'static [(&'static str, &'static str)],
}

/// The shipped set. Every template that refs `collector@` directly, plus the
/// level that refs one of those — `the_judged_set_covers_every_shipped_collector`
/// keeps this list from going stale silently.
const COMPOSITES: &[Composite] = &[
    Composite {
        name: "talky",
        graph: "talky/config.json",
        layers: &[("talky/collector/config.json", "assemble")],
    },
    Composite {
        name: "cogny",
        graph: "cogny/config.json",
        layers: &[("cogny/collector/config.json", "assemble")],
    },
    Composite {
        name: "assistant",
        graph: "assistant/config.json",
        layers: &[
            ("talky/collector/config.json", "assemble"),
            ("assistant/talky/config.json", "collector/assemble"),
        ],
    },
];

fn param(c: &Composite, key: &str) -> Option<Value> {
    let mut found = None;
    for (file, addr) in c.layers {
        if let Some(cfg) = read(file) {
            let v = &cfg["override_params"][*addr][key];
            if !v.is_null() {
                found = Some(v.clone());
            }
        }
    }
    found
}

/// The default a knob carries when no layer overrides it — read off the shipped
/// cell, so a changed default moves this gate with it.
fn assemble_default(key: &str) -> Value {
    read("collector/assemble/config.json")
        .map(|c| c["params"][key].clone())
        .unwrap_or(Value::Null)
}

fn declared(c: &Composite) -> Vec<String> {
    param(c, "tools")
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        // `["*"]` asks for everything an answerer has and names nothing.
        .filter(|s| s != "*")
        .collect()
}

fn on(c: &Composite, key: &str) -> bool {
    let v = param(c, key).unwrap_or_else(|| assemble_default(key));
    match v.as_str() {
        // The two switches are strings, and "" is how a template says OFF: the
        // lane answers a typed refusal instead of a result, so the tool must not
        // be on the menu either (GH #512).
        Some(s) => !s.trim().is_empty(),
        None => !v.is_null(),
    }
}

/// The names the collector answers ITSELF (GH #512), decided by the switch that
/// already decides whether the lane is answered instead of refused.
///
/// It was two until GH #552. `memory_call_tier` decided the second, and the
/// second was `memory_recall` — served out of the collector's own recall port
/// under a schema it had typed by hand against a contract one level up. The
/// member's memory hive declares and answers it now, so it reaches this
/// composite's menu the ordinary way: DECLARED in `params.tools` and routed by an
/// edge, which is what `menu` below already measures.
fn self_served(c: &Composite) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if on(c, "thread_recall") {
        out.insert("thread_recall".to_string());
    }
    out
}

/// Every `hop.tool_name == '<name>'` an edge of this composite is conditioned on:
/// the names it ROUTES, which for a composite is the same statement as the names
/// it can answer for out of its own topology.
fn routed(c: &Composite) -> BTreeSet<String> {
    let Some(cfg) = read(c.graph) else {
        return BTreeSet::new();
    };
    let mut out = BTreeSet::new();
    for e in cfg["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        let Some(cond) = e["condition"].as_str() else {
            continue;
        };
        let mut rest = cond;
        while let Some(at) = rest.find("hop.tool_name == '") {
            rest = &rest[at + "hop.tool_name == '".len()..];
            if let Some(end) = rest.find('\'') {
                out.insert(rest[..end].to_string());
                rest = &rest[end..];
            } else {
                break;
            }
        }
    }
    out
}

/// The menu this composite can carry: what it declares AND somebody answers,
/// plus what it answers itself.
fn menu(c: &Composite, hive: &BTreeSet<String>) -> BTreeSet<String> {
    let routed = routed(c);
    let mut out: BTreeSet<String> = declared(c)
        .into_iter()
        .filter(|n| hive.contains(n) || routed.contains(n))
        .collect();
    out.extend(self_served(c));
    out
}

// ───────────────────────────────────────── the charter texts a model is given

/// Every seed row of this composite's tree that lands in a `system.*` family a
/// model READS as instruction — the charter, the identity, the persona. A tool
/// declaration row (`tools.*`) is not one of them: it IS the menu.
fn charter_texts(name: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![templates_root().join(name)];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|e| e == "jsonl") {
                let Ok(raw) = std::fs::read_to_string(&p) else {
                    continue;
                };
                for line in raw.lines() {
                    let Ok(row) = meclaw_core::serde_json::from_str::<Value>(line) else {
                        continue;
                    };
                    let Some(slot) = row["slot_path"].as_str() else {
                        continue;
                    };
                    if ["instructions.", "identity.", "persona.", "handover."]
                        .iter()
                        .any(|f| slot.starts_with(f))
                    {
                        out.insert(
                            format!("{}:{slot}", p.display()),
                            row["value"]["text"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                        );
                    }
                }
            }
        }
    }
    out
}

/// The names a text NAMES, over a closed vocabulary. A closed vocabulary rather
/// than a regex on purpose: every prose word shaped like an identifier would
/// otherwise be a finding, and a gate that cries wolf is a gate somebody turns
/// off.
fn names_in(text: &str, vocabulary: &BTreeSet<String>) -> BTreeSet<String> {
    vocabulary
        .iter()
        .filter(|n| text.contains(n.as_str()))
        .cloned()
        .collect()
}

/// The gate itself, over one composite: every finding as one sentence.
fn findings(c: &Composite, hive: &BTreeSet<String>, vocabulary: &BTreeSet<String>) -> Vec<String> {
    let menu = menu(c, hive);
    let mut out = Vec::new();
    for (where_, text) in charter_texts(c.name) {
        for named in names_in(&text, vocabulary) {
            if !menu.contains(&named) {
                out.push(format!(
                    "{}: the charter at {where_} names `{named}`, which this composite's menu \
                     cannot carry (menu: {menu:?})",
                    c.name
                ));
            }
        }
    }
    for name in declared(c) {
        if !hive.contains(&name) && !routed(c).contains(&name) {
            out.push(format!(
                "{}: `{name}` is declared in `params.tools`, and no answerer of this composite \
                 delivers it -- the tool hive has nothing under it and no edge routes it. This \
                 is GH #464 happening again",
                c.name
            ));
        }
    }
    for name in routed(c) {
        // A reserved name never leaves the composite and is answered inside it
        // rather than declared to a model; an errand the level routes to an
        // occupant IS declared, and that is the difference this loop measures.
        if name == "escalate_to_deep" {
            continue;
        }
        if !menu.contains(&name) {
            out.push(format!(
                "{}: `{name}` is ROUTED by an edge and is on no menu -- the lane is wired and \
                 the model can never ask for it, which is what GH #512 measured as a memory \
                 chain standing idle",
                c.name
            ));
        }
    }
    out
}

fn vocabulary(hive: &BTreeSet<String>) -> BTreeSet<String> {
    let mut v = hive.clone();
    for c in COMPOSITES {
        v.extend(routed(c));
        v.extend(declared(c));
        v.extend(self_served(c));
    }
    v
}

// ══════════════════════════════════════════════════════════════ the sweep

#[test]
fn no_shipped_composite_names_a_tool_its_menu_cannot_carry() {
    let hive = what_the_tool_hive_answers();
    if hive.is_empty() {
        // R2b / GH #49: a tree without the tools hive SKIPS instead of failing.
        return;
    }
    let vocab = vocabulary(&hive);
    let mut all = Vec::new();
    for c in COMPOSITES {
        if read(c.graph).is_none() {
            continue;
        }
        all.extend(findings(c, &hive, &vocab));
    }
    assert!(all.is_empty(), "{}", all.join("\n"));
}

/// The judged set is the whole set: a template that refs a collector and is not
/// in `COMPOSITES` is a composite this gate does not see.
#[test]
fn the_judged_set_covers_every_shipped_collector() {
    let mut refs: BTreeSet<String> = BTreeSet::new();
    let Ok(entries) = std::fs::read_dir(templates_root()) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !entry.path().is_dir() || name.starts_with('_') {
            continue;
        }
        let mut stack = vec![entry.path()];
        while let Some(dir) = stack.pop() {
            for sub in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                if sub.path().is_dir() {
                    stack.push(sub.path());
                } else if sub.file_name() == "config.json" {
                    let raw = std::fs::read_to_string(sub.path()).unwrap_or_default();
                    if raw.contains("\"collector@") {
                        refs.insert(name.clone());
                    }
                }
            }
        }
    }
    let judged: BTreeSet<String> = COMPOSITES.iter().map(|c| c.name.to_string()).collect();
    let unseen: Vec<&String> = refs.difference(&judged).collect();
    assert!(
        unseen.is_empty(),
        "these templates carry a collector and no row in COMPOSITES, so nothing checks their \
         charter against their menu: {unseen:?}"
    );
    assert!(
        !refs.is_empty(),
        "a sweep that finds no collector at all passes for free"
    );
}

// ═══════════════════════════════════════════════════ the test of the test

/// A charter naming a tool the menu cannot carry is REPORTED — through the same
/// function the sweep runs, on fabricated input, with no file touched.
#[test]
fn a_fabricated_charter_is_reported() {
    let hive: BTreeSet<String> = ["web_search".to_string()].into_iter().collect();
    let menu: BTreeSet<String> = ["web_search".to_string()].into_iter().collect();
    let vocab: BTreeSet<String> = ["web_search".to_string(), "remember".to_string()]
        .into_iter()
        .collect();
    let named = names_in(
        "When something matters, call remember with it, and web_search for the rest.",
        &vocab,
    );
    let missing: Vec<&String> = named.difference(&menu).collect();
    assert_eq!(
        missing,
        vec![&"remember".to_string()],
        "the charter names a tool the menu does not carry and the comparison must say so"
    );
    assert!(
        hive.contains("web_search"),
        "and the one it does carry is not a finding"
    );
}

/// A declared name nobody answers is REPORTED — the #464 half, on fabricated
/// input, through the same `findings` the sweep runs.
#[test]
fn a_fabricated_declaration_without_an_answerer_is_reported() {
    let hive: BTreeSet<String> = ["web_search".to_string()].into_iter().collect();
    // `telepathy` is in no hive and on no edge of the shipped assistant.
    let fake = Composite {
        name: "assistant",
        graph: "assistant/config.json",
        layers: &[("__no_such_file__", "assemble")],
    };
    // The layered read finds nothing, so the composite declares nothing and the
    // sweep is silent — which is what makes the next line a measurement rather
    // than a coincidence.
    assert!(
        declared(&fake).is_empty(),
        "the fabricated layer must declare nothing on its own"
    );
    let declared_now = ["web_search".to_string(), "telepathy".to_string()];
    let routed = routed(&fake);
    let orphans: Vec<&String> = declared_now
        .iter()
        .filter(|n| !hive.contains(*n) && !routed.contains(*n))
        .collect();
    assert_eq!(
        orphans,
        vec![&"telepathy".to_string()],
        "a declared name with no answerer is exactly the shape #464 left behind"
    );
    assert!(
        routed.contains("consult_cogny"),
        "and the shipped level DOES route its errand, which is why declaring it is right: \
         {routed:?}"
    );
}
