//! GH #286 — the declared blast radius is the real one.
//!
//! `tools@1.0.0` exists so that the whole tool surface of an assistant is one
//! `swap_nodes` away from being something else. That property has a price the
//! issue names: a swap changes what the hive is ALLOWED to do, and today that
//! answer is not written anywhere — it is the silent sum of four `config.json`
//! files, and reading it means opening all four and doing the arithmetic in
//! one's head. So `template.json` carries two declarations, and this file holds
//! them to the tree:
//!
//! 1. **`sandbox_union`** — the widest value of every sandbox axis over ALL
//!    occupants. Not an average and not the intent: a union.
//! 2. **`reentrancy`** — one entry per occupant, saying whether a second call
//!    may run while a first is in flight.
//!
//! **The union is recomputed here, never listed here.** A test that carried the
//! expected union as a literal would be a second copy of the declaration, and
//! the first drift would move both. So the four occupant directories are read
//! off disk, their `params.sandbox` blocks are parsed by the SUBSTRATE's own
//! reader ([`SandboxProfile::parse`] — the same one `validate_params` runs, so
//! this also fails on a profile that would refuse the boot), and the axes are
//! folded one at a time, widest wins:
//!
//! * `trust` is `full` as soon as one occupant declares no `sandbox` block at
//!   all or declares `trust: "trusted"` — both are the unenforced state, and a
//!   union over them is unenforced.
//! * `network` is `allow` if any occupant is unrestricted in that sense or
//!   declares `network: "allow"`. An absent `network` key under
//!   `trust: "restricted"` is `deny`, because that is what the substrate
//!   enforces (`sandbox/profile.rs`, `NetworkPolicy::Deny` for `None`).
//! * `filesystem` is `unrestricted` if any occupant is unrestricted; otherwise
//!   the sorted union of the declared `read` and `write` roots. `runtime` is
//!   deliberately not a root: it is the interpreter set the substrate grants so
//!   that a runner can start at all, identical wherever it is asked for, and it
//!   widens nothing an occupant chose.
//!
//! **Why the reentrancy check runs in both directions.** #286's hazard is a
//! swap that quietly turns a parallel tool round sequential. A declaration with
//! a hole cannot catch that — an occupant nobody declared is exactly the one
//! whose serialisation surprises the caller. And an entry naming an occupant
//! that no longer exists is the same defect running backwards: it outlives the
//! cell it described and keeps answering for it. So every occupant directory
//! must have an entry, and every entry must name a directory that exists.

use meclaw_cells::sandbox::{NetworkPolicy, SandboxProfile};
use meclaw_core::serde_json::Value;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// The keys `sandbox_union` may carry. Closed on purpose: a fourth axis added
/// to the declaration without being computed here would read as covered and be
/// unchecked.
const UNION_KEYS: [&str; 4] = ["trust", "network", "filesystem", "because"];

/// The keys one `reentrancy` entry may carry. Closed for the same reason.
const REENTRANCY_KEYS: [&str; 2] = ["reentrant", "because"];

fn tools_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/tools")
}

fn read_json(path: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{}: {e} — the template must be on disk", path.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn template_json() -> Value {
    read_json(&tools_root().join("template.json"))
}

/// Every occupant of the hive, as `(directory name, params)`, sorted.
///
/// Discovered rather than listed: adding a tool is one occupant directory, and
/// this file must speak about the hive as it is on the day it runs, not as it
/// was on the day it was written.
fn occupants() -> Vec<(String, Value)> {
    let root = tools_root();
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&root).expect("templates/tools is readable") {
        let entry = entry.expect("templates/tools entry");
        let path = entry.path();
        let config = path.join("config.json");
        if !path.is_dir() || !config.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let params = read_json(&config)
            .get("params")
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default()));
        out.push((name, params));
    }
    assert!(
        !out.is_empty(),
        "templates/tools has no occupant directories — the union below would be a statement \
         about nothing"
    );
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The union as the tree computes it, plus who drove it there.
struct Union {
    trust: &'static str,
    network: &'static str,
    filesystem: Value,
    /// Occupants that declare no enforcement at all. Each of them alone sets
    /// every axis to its widest value, which is the whole uncomfortable point.
    unrestricted: Vec<String>,
}

impl Union {
    fn axis(&self, name: &str) -> Value {
        match name {
            "trust" => Value::from(self.trust),
            "network" => Value::from(self.network),
            "filesystem" => self.filesystem.clone(),
            other => panic!("no such axis: {other}"),
        }
    }
}

fn computed_union() -> Union {
    let mut unrestricted: Vec<String> = Vec::new();
    let mut network_allow = false;
    let mut roots: BTreeSet<String> = BTreeSet::new();

    for (name, params) in occupants() {
        let parsed = SandboxProfile::parse(&params).unwrap_or_else(|e| {
            panic!(
                "templates/tools/{name}/config.json: params.sandbox does not parse: {e}. \
                 This is the substrate's own reader — a block it refuses is a boot failure, \
                 not a documentation defect."
            )
        });
        match parsed {
            // No block and the explicit escape hatch are the same state: nothing
            // is enforced, so every axis is at its widest.
            None | Some(SandboxProfile::Trusted) => unrestricted.push(name),
            Some(SandboxProfile::Restricted {
                network,
                filesystem,
                ..
            }) => {
                if network == NetworkPolicy::Allow {
                    network_allow = true;
                }
                for root in filesystem.read.iter().chain(filesystem.write.iter()) {
                    roots.insert(root.display().to_string());
                }
            }
        }
    }

    let open = !unrestricted.is_empty();
    Union {
        trust: if open { "full" } else { "restricted" },
        network: if open || network_allow {
            "allow"
        } else {
            "deny"
        },
        filesystem: if open {
            Value::from("unrestricted")
        } else {
            Value::from(roots.into_iter().collect::<Vec<_>>())
        },
        unrestricted,
    }
}

// ─────────────────────────────────────────────────────────────── the union

#[test]
fn the_declared_union_is_the_one_the_occupants_compute() {
    let computed = computed_union();
    let declared = template_json()
        .get("sandbox_union")
        .cloned()
        .expect("templates/tools/template.json declares `sandbox_union` — without it the blast radius of this hive is the silent sum of its occupants, which is the invisibility GH #286 exists to end");
    let obj = declared
        .as_object()
        .expect("templates/tools/template.json: `sandbox_union` must be a JSON object")
        .clone();

    for key in obj.keys() {
        assert!(
            UNION_KEYS.contains(&key.as_str()),
            "sandbox_union carries the key {key:?}, which nothing computes. Allowed: {}. \
             An axis that is declared but not folded over the occupants reads as covered \
             and is not.",
            UNION_KEYS.join(", ")
        );
    }

    let mut divergences = Vec::new();
    for axis in ["trust", "network", "filesystem"] {
        let want = computed.axis(axis);
        let got = obj.get(axis).cloned().unwrap_or(Value::Null);
        if got != want {
            divergences.push(format!("{axis}: declared {got}, the tree computes {want}"));
        }
    }

    let unrestricted = if computed.unrestricted.is_empty() {
        "none".to_string()
    } else {
        computed.unrestricted.join(", ")
    };
    assert!(
        divergences.is_empty(),
        "the declared sandbox union no longer matches the occupants on disk — {}. \
         Occupants that declare no enforcement at all: {unrestricted}; each one of them \
         alone sets every axis to its widest value. This is not a test to relax: the \
         declaration is what a replacement is measured against, so a tree that moved \
         without the declaration moving is exactly the widening #286 asked to be visible \
         in the diff.",
        divergences.join("; ")
    );
}

#[test]
fn the_union_says_out_loud_which_occupants_it_is_a_union_over() {
    let template = template_json();
    let because = template
        .get("sandbox_union")
        .and_then(|u| u.get("because"))
        .and_then(Value::as_str)
        .expect("sandbox_union states a `because`");
    assert!(
        !because.trim().is_empty(),
        "sandbox_union's `because` is empty. The value alone reads as a policy someone chose; \
         the sentence is what says it is a sum nobody chose."
    );
    for (name, _) in occupants() {
        assert!(
            because.contains(&name),
            "sandbox_union's `because` never names the occupant {name:?}. Every occupant is an \
             input to this union — the ones that tighten as much as the ones that widen, because \
             a union is not an average and a reader has to be able to see which is which. A new \
             occupant that is not named here is one whose contribution nobody weighed."
        );
    }
}

// ────────────────────────────────────────────────────────── the reentrancy

#[test]
fn every_occupant_has_a_reentrancy_entry_and_every_entry_has_an_occupant() {
    let template = template_json();
    let declared = template
        .get("reentrancy")
        .and_then(Value::as_object)
        .expect("templates/tools/template.json declares `reentrancy` as an object");

    let on_disk: BTreeSet<String> = occupants().into_iter().map(|(name, _)| name).collect();
    let named: BTreeSet<String> = declared.keys().cloned().collect();

    let missing: Vec<&String> = on_disk.difference(&named).collect();
    assert!(
        missing.is_empty(),
        "these occupants exist and no `reentrancy` entry speaks for them: {missing:?}. \
         #286's hazard is a swap that quietly turns a parallel tool round sequential, and \
         an undeclared occupant is precisely the one whose serialisation surprises the caller."
    );

    let stale: Vec<&String> = named.difference(&on_disk).collect();
    assert!(
        stale.is_empty(),
        "these `reentrancy` entries name occupants that do not exist: {stale:?}. An entry \
         that outlives its cell keeps answering for it — the same defect running backwards."
    );
}

#[test]
fn every_reentrancy_entry_is_a_verdict_with_a_reason() {
    let template = template_json();
    let declared = template
        .get("reentrancy")
        .and_then(Value::as_object)
        .expect("templates/tools/template.json declares `reentrancy` as an object");

    for (name, entry) in declared {
        let obj = entry
            .as_object()
            .unwrap_or_else(|| panic!("reentrancy.{name} must be a JSON object"));
        for key in obj.keys() {
            assert!(
                REENTRANCY_KEYS.contains(&key.as_str()),
                "reentrancy.{name} carries the key {key:?} (allowed: {})",
                REENTRANCY_KEYS.join(", ")
            );
        }
        assert!(
            obj.get("reentrant").and_then(Value::as_bool).is_some(),
            "reentrancy.{name} states no boolean `reentrant`. \"probably fine\" is not a \
             declaration a caller can plan a parallel round against."
        );
        let because = obj
            .get("because")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("reentrancy.{name} states no `because`"));
        assert!(
            !because.trim().is_empty(),
            "reentrancy.{name}'s `because` is empty. The verdict is cheap to copy from the \
             neighbour above it; the reason is what makes a swap notice it no longer holds."
        );
    }
}
