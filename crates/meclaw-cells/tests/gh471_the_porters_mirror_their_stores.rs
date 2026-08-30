//! GH #471 — the three new porters mirror the stores they walk, and refuse
//! what must never travel.
//!
//! A member's export used to carry the memory hive and nothing else, so a
//! member reborn from it remembered everything it had been told and knew
//! nothing about who may be told what. `affinity`, `firewall` and
//! `session-keeper` each grew a porter of their own; every one of them carries
//! a SCHEMA mirror of its store's declaration, because a script cannot import a
//! JSON file and a walk that reads a column list has to have one.
//!
//! A mirror is exactly the thing that rots silently: a column added to a store
//! and not to the mirror does not fail anything — it simply stops travelling,
//! and the loss surfaces one colony later as an empty field nobody can trace.
//! `memory-hive` has had this gate since GH #243
//! (`gh243_a_memory_can_leave_a_hive_and_arrive_in_another`); these three get
//! the same one.
//!
//! Three properties:
//!
//! 1. **Every content column of the store is in the mirror**, and every column
//!    of the mirror is in the store. Set equality, per table.
//! 2. **The excluded tables are excluded on purpose.** Each porter names what
//!    it will not carry and why; this file pins the SET, so removing an
//!    exclusion is a decision somebody has to write down here too.
//! 3. **The document format is per hive.** Three formats, all different, and
//!    none of them the memory hive's — a reader that met the wrong one would
//!    write rows nobody declared, which is exactly what the version string
//!    exists to prevent.

use meclaw_core::serde_json::{Value, from_str};
use std::collections::BTreeSet;
use std::path::PathBuf;

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .join(rel)
}

/// One hive: where its porter lives, where its store lives, the document format
/// the porter stamps, and the tables the store has that the walk deliberately
/// leaves behind.
struct Hive {
    name: &'static str,
    porter: &'static str,
    store: &'static str,
    format: &'static str,
    /// Tables the STORE declares and the mirror deliberately does not carry.
    left_behind: &'static [&'static str],
    /// Tables the mirror carries that the store does not declare, because the
    /// store creates them itself out of `params.canonical`.
    store_owned: &'static [&'static str],
}

const HIVES: &[Hive] = &[
    Hive {
        name: "affinity",
        porter: "templates/affinity/porter/config.json",
        store: "templates/affinity/store/config.json",
        format: "meclaw-affinity-export/1",
        left_behind: &["port_scratch"],
        store_owned: &["entity_aliases", "entity_rejected_pairs"],
    },
    Hive {
        name: "firewall",
        porter: "templates/firewall/porter/config.json",
        store: "templates/firewall/rules/config.json",
        format: "meclaw-firewall-export/1",
        left_behind: &["arrivals", "held", "port_scratch"],
        store_owned: &[],
    },
    Hive {
        name: "session-keeper",
        porter: "templates/session-keeper/porter/config.json",
        store: "templates/session-keeper/sessions/config.json",
        format: "meclaw-session-export/1",
        left_behind: &["port_scratch"],
        store_owned: &[],
    },
];

fn shipped() -> bool {
    HIVES
        .iter()
        .all(|h| repo(h.porter).is_file() && repo(h.store).is_file())
}

fn read_json(rel: &str) -> Value {
    from_str(&std::fs::read_to_string(repo(rel)).unwrap_or_else(|e| panic!("{rel}: {e}")))
        .unwrap_or_else(|e| panic!("{rel} is not json: {e}"))
}

/// The porter's script, run under a python that imports nothing and executes
/// only the constant header — so the mirror is read as the values the cell
/// really holds, never as a regex over source text.
fn constants(porter_rel: &str, names: &[&str]) -> Value {
    let cfg = read_json(porter_rel);
    let script = cfg["params"]["script_inline"]
        .as_str()
        .unwrap_or_else(|| panic!("{porter_rel}: no params.script_inline"));
    // The script reads stdin at its first line and then dispatches on a phase
    // it will not find, so it parks. Everything above the dispatch — the whole
    // constant header — has run by then, and `park()` leaves via SystemExit,
    // which the harness below catches so the constants can be printed.
    let program = format!(
        "import sys, io, json\n\
         sys.stdin = io.StringIO({})\n\
         try:\n\
         \x20   exec(compile({}, 'porter', 'exec'), globals())\n\
         except SystemExit:\n\
         \x20   pass\n\
         sys.stderr.write(json.dumps({{name: globals()[name] for name in {}}}))\n",
        meclaw_core::serde_json::to_string(
            &meclaw_core::serde_json::json!({"body": {}, "envelope": {}, "params": {}}).to_string()
        )
        .unwrap(),
        meclaw_core::serde_json::to_string(script).unwrap(),
        meclaw_core::serde_json::to_string(names).unwrap(),
    );
    let out = {
        use std::io::Write;
        let mut child = std::process::Command::new("python3")
            .arg("-")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("python3");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(program.as_bytes())
            .expect("write");
        child.wait_with_output().expect("wait")
    };
    from_str(&String::from_utf8_lossy(&out.stderr)).unwrap_or_else(|e| {
        panic!(
            "{porter_rel}: could not read the constants ({e}); stdout was {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

#[test]
fn every_porter_carries_its_store_column_for_column() {
    if !shipped() {
        return;
    }
    for h in HIVES {
        let store = read_json(h.store);
        let c = constants(h.porter, &["SCHEMA", "WALK", "FORMAT", "HIVE"]);

        let declared = keys(&store["params"]["schema"]);
        let mirrored = keys(&c["SCHEMA"]);
        let expected: BTreeSet<String> = declared
            .iter()
            .filter(|t| !h.left_behind.contains(&t.as_str()))
            .cloned()
            .chain(h.store_owned.iter().map(|s| (*s).to_string()))
            .collect();
        assert_eq!(
            mirrored, expected,
            "{}: the porter's SCHEMA mirror and the store's declaration name \
             different tables. A table that is in the store and not in the \
             mirror stops travelling without failing anything, and the loss \
             surfaces one colony later as a hive that is simply missing \
             something",
            h.name
        );

        for table in &mirrored {
            if h.store_owned.contains(&table.as_str()) {
                continue; // the store writes these itself, out of params.canonical
            }
            assert_eq!(
                keys(&c["SCHEMA"][table]),
                keys(&store["params"]["schema"][table]),
                "{}: the columns of `{table}` differ between the porter's \
                 mirror and the store. A column added to the store and not \
                 here is a column that silently stops being transferred",
                h.name
            );
        }

        let walk: BTreeSet<String> = c["WALK"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(|v| v.as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            walk, mirrored,
            "{}: the walk and the mirror disagree. A table in the mirror that \
             the walk never reads is a promise the document does not keep",
            h.name
        );
        assert_eq!(c["HIVE"].as_str(), Some(h.name));
    }
}

#[test]
fn what_stays_behind_stays_behind_by_name() {
    if !shipped() {
        return;
    }
    for h in HIVES {
        let store = read_json(h.store);
        let declared = keys(&store["params"]["schema"]);
        for table in h.left_behind {
            assert!(
                declared.contains(*table),
                "{}: this file claims `{table}` is deliberately left behind, but \
                 the store no longer declares it — a stale exclusion reads \
                 exactly like a live one",
                h.name
            );
        }
        let c = constants(h.porter, &["SCHEMA"]);
        for table in h.left_behind {
            assert!(
                !keys(&c["SCHEMA"]).contains(*table),
                "{}: `{table}` travels. It is machinery of THIS installation — \
                 a notepad, a spent rate window — and carrying it over would \
                 hand a second colony work it never did",
                h.name
            );
        }
    }
}

#[test]
fn each_hive_has_its_own_document_format() {
    if !shipped() {
        return;
    }
    let mut seen: BTreeSet<String> = BTreeSet::new();
    seen.insert("meclaw-memory-export/1".to_string());
    for h in HIVES {
        let c = constants(h.porter, &["FORMAT"]);
        let format = c["FORMAT"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            format, h.format,
            "{}: the document format moved. A format string is a version, and a \
             reader that does not know one refuses instead of guessing — so \
             moving it is a deliberate act with a migration behind it",
            h.name
        );
        assert!(
            seen.insert(format.clone()),
            "{}: {format} is already another hive's format. Two hives sharing \
             one format is how an `affinity` part gets written into a memory \
             store: both declare a table called `entities`",
            h.name
        );
    }
}
