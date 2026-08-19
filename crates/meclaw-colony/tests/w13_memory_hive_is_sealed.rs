//! W13 (rulings F1 + F3, 2026-08-15), rewritten for GH #197 — the shipped
//! `memory-hive` is really sealed.
//!
//! GH #133 and #132 built two opt-in switches. A switch nobody flips is theatre,
//! so the reference hive flips both:
//!
//! - `templates/memory-hive/config.json` declares `params.ports`
//! - `templates/memory-hive/store/config.json` declares `write_surface: "internal"`
//!
//! # What this file used to assert, and why that was wrong
//!
//! It pinned `params.ports == ["writer", "recall", "extract-glue"]` as a literal
//! and asserted each entry was a real direct child. That spelled out the RESULT
//! of a decision instead of the property behind it — and when the owner's ruling
//! of 2026-08-18 turned those three cell names into lanes at the hive path, the
//! test went red for the migration it was supposed to protect. A test that
//! forbids a change because it dictates the change's outcome is the defect, not
//! the change.
//!
//! What the file is actually for is one sentence: **nothing outside this hive
//! can address anything inside it.** That is true of the port form and of the
//! sealed form, it survives every rearrangement of the interior, and it is the
//! only thing a caller is entitled to rely on. So the assertion is now derived
//! from the tree — every child the template ships is run through the REAL
//! boundary — rather than compared against a list somebody has to keep.
//!
//! Guarded like every other template-reading test (GH #49): only a tree that
//! actually carries the template runs the body.

use meclaw_colony::mutation::port_boundary::{
    SealedHive, collect_sealed_hives, validate_hive_port_boundary,
};
use meclaw_core::serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Where the hive lives while it is being checked. Any path does; what matters
/// is that endpoints resolve the way the colony resolves them.
const HIVE: &str = "/memory";

fn repo_path(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The template root, or `None` in a tree that does not carry it.
fn memory_hive() -> Option<PathBuf> {
    let p = repo_path("templates/memory-hive");
    p.join("config.json").is_file().then_some(p)
}

fn read_json(p: &Path) -> Value {
    let raw = std::fs::read_to_string(p).expect("read config.json");
    meclaw_core::serde_json::from_str(&raw).expect("config.json parses")
}

/// Short names of every cell directory the template ships — the set an
/// instantiation copies along, read from the tree rather than listed here so a
/// new interior cell is covered the day it appears.
fn interior_cells(root: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(root)
        .expect("read the template root")
        .filter_map(|e| {
            let e = e.expect("dir entry");
            let p = e.path();
            (p.is_dir() && p.join("config.json").is_file())
                .then(|| e.file_name().to_string_lossy().into_owned())
        })
        .collect();
    out.sort();
    out
}

/// The seal exactly as the substrate reads it out of the shipped file. Planting
/// the real `config.json` in a throwaway root and asking the real reader is the
/// `gh196` pattern: a test that re-derives the rule can agree with itself while
/// disagreeing with the colony.
fn seal_the_substrate_reads(root: &Path) -> Vec<SealedHive> {
    let td = tempfile::TempDir::new().expect("tempdir");
    let croot = td.path();
    std::fs::create_dir_all(croot.join("main/memory")).expect("mkdir");
    std::fs::write(
        croot.join("main/config.json"),
        r#"{"cell":{"type":"hive"}}"#,
    )
    .expect("write main config");
    std::fs::copy(
        root.join("config.json"),
        croot.join("main/memory/config.json"),
    )
    .expect("copy the hive config");
    let paths = [meclaw_core::Path::new(HIVE)];
    let sealed = collect_sealed_hives(croot, paths.iter());
    assert_eq!(sealed.len(), 1, "the reader saw no seal at all");
    sealed
}

#[test]
fn the_shipped_memory_hive_is_sealed_to_its_own_path() {
    let Some(root) = memory_hive() else {
        return;
    };
    let sealed = seal_the_substrate_reads(&root);
    assert!(
        sealed[0].ports.is_empty(),
        "GH #197: the hive path is the address and the lane is the port — a declared port names \
         an interior cell, which is the thing the migration removed: {:?}",
        sealed[0].ports
    );

    // And that is not a statement about a list, it is a statement about every
    // cell the template ships: from outside, none of them is an endpoint.
    let cells = interior_cells(&root);
    assert!(
        cells.len() >= 8,
        "sanity: this hive carries a real interior, found {cells:?}"
    );
    for cell in &cells {
        for edge in [
            json!({"from": "./anchor", "to": format!("{HIVE}/{cell}")}),
            json!({"from": format!("{HIVE}/{cell}"), "to": "./anchor"}),
        ] {
            let shown = edge.to_string();
            let err = validate_hive_port_boundary(&json!({"add_edges": [edge]}), "/", &sealed)
                .expect_err(&format!("{shown}: an interior address must be refused"));
            assert_eq!(
                err.error_code(),
                "hive_port_boundary",
                "wrong code for {shown}"
            );
        }
    }
}

#[test]
fn the_hive_states_its_lanes_and_none_of_them_is_a_cell_name() {
    let Some(root) = memory_hive() else {
        return;
    };
    let cfg = read_json(&root.join("config.json"));
    let contract = cfg["params"]["contract"]
        .as_object()
        .expect("a sealed hive owes a contract — the lane IS the port now");
    let cells = interior_cells(&root);
    let mut lanes = 0usize;
    for side in ["accepts", "emits"] {
        for lane in contract[side].as_array().expect("accepts/emits is a list") {
            let route = lane["route"].as_str().expect("a lane names a route");
            assert!(
                !route.is_empty() && !route.contains('/') && !route.starts_with('.'),
                "'{route}' is a path, not a lane"
            );
            assert!(
                !cells.iter().any(|c| c == route),
                "'{route}' is the name of a cell in this hive — a lane says what a caller wants, \
                 never where it lands (ruling 2026-08-18)"
            );
            assert!(
                !lane["because"]
                    .as_str()
                    .unwrap_or_default()
                    .trim()
                    .is_empty(),
                "lane '{route}' states no reason"
            );
            lanes += 1;
        }
    }
    assert!(
        lanes >= 4,
        "the contract says almost nothing: {lanes} lanes"
    );
}

#[test]
fn the_shipped_memory_store_declares_an_internal_write_surface() {
    let Some(root) = memory_hive() else {
        return;
    };
    let cfg = read_json(&root.join("store/config.json"));
    assert_eq!(
        cfg["params"]["write_surface"], "internal",
        "ruling F3: the memory store is writable only from inside its own hive"
    );
    // GH #260: the cell-level half above bounds only what `handle()` runs. The
    // `transfer` slot is answered by the substrate BEFORE `handle()`, so a store
    // that seals one half and not the other is still writable from outside —
    // through a door the declaration above cannot see.
    assert_eq!(
        cfg["contract"]["write_surface"], "internal",
        "GH #260: the substrate half has to be declared too, or an import walks past ruling F3"
    );
}

/// The canonical mutation from `templates/memory-hive/README.md` (the block a
/// builder copies), expressed against a hive instantiated at `/memory`. Every
/// endpoint is the hive; the lane rides on `hop.route`.
fn readme_mutation() -> Value {
    json!({"add_edges": [
        {"from": "./anchor", "to": "./memory",
         "modifier": {"set_hop": {"route": "'in_episode'"}}},
        {"from": "./anchor", "to": "./memory",
         "modifier": {"set_hop": {"route": "'in_query'"}}},
        {"from": "./anchor", "to": "./memory",
         "modifier": {"set_hop": {"route": "'in_remember'"}}},
        {"from": "./memory", "to": "./capture",
         "condition": "has(hop.route) && hop.route == 'bundle'"},
        {"from": "./memory", "to": "./capture",
         "condition": "has(hop.route) && hop.route == 'reject'"},
    ]})
}

#[test]
fn the_documented_wiring_stays_legal_under_the_seal() {
    let Some(root) = memory_hive() else {
        return;
    };
    // If the seal rejected the mutation the README hands a builder, the
    // template would be unusable. Both directions are in here.
    validate_hive_port_boundary(&readme_mutation(), "/", &seal_the_substrate_reads(&root))
        .expect("the README's own wiring must stay legal");
}

#[test]
fn the_hive_path_itself_and_the_internal_graph_stay_legal() {
    let Some(root) = memory_hive() else {
        return;
    };
    let sealed = seal_the_substrate_reads(&root);

    // The transit address...
    validate_hive_port_boundary(
        &json!({"add_edges": [{"from": "./anchor", "to": "./memory"}]}),
        "/",
        &sealed,
    )
    .expect("the hive path is an address");

    // ...and the hive's own graph, which wires straight at `./store`.
    let internal = json!({"add_edges": [
        {"from": "./writer", "to": "./store"},
        {"from": "./store",  "to": "./recall"},
        {"from": "./embed",  "to": "./store"},
    ]});
    validate_hive_port_boundary(&internal, HIVE, &sealed)
        .expect("inside the hive every edge stays legal");
}
