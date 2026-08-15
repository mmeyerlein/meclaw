//! W13 (rulings F1 + F3, 2026-08-15) — the shipped `memory-hive` is really sealed.
//!
//! GH #133 and #132 built two opt-in switches. A switch nobody flips is theatre,
//! so the reference hive flips both:
//!
//! - `templates/memory-hive/config.json` declares `params.ports`
//! - `templates/memory-hive/store/config.json` declares `write_surface: "internal"`
//!
//! This is the drift lock over that decision, in three directions at once:
//! the declaration matches the README's port table, every declared port is a
//! real direct child, and the canonical mutation the README hands a builder is
//! still legal under the seal while a deep endpoint into the store is not.
//!
//! Guarded like every other template-reading test (GH #49): only a tree that
//! actually carries the template runs the body.

use meclaw_colony::mutation::port_boundary::{SealedHive, validate_hive_port_boundary};
use meclaw_core::serde_json::{Value, json};
use std::path::{Path, PathBuf};

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

/// The three endpoints the README's port table names. Kept as a literal so a
/// silent widening of the declaration has to walk past a failing assertion.
const DECLARED_PORTS: [&str; 3] = ["writer", "recall", "extract-glue"];

#[test]
fn the_shipped_memory_hive_declares_exactly_the_documented_ports() {
    let Some(root) = memory_hive() else {
        return;
    };
    let cfg = read_json(&root.join("config.json"));
    let ports: Vec<String> = cfg["params"]["ports"]
        .as_array()
        .expect("params.ports is declared -- the hive is sealed (ruling F1)")
        .iter()
        .map(|v| v.as_str().expect("a port name is a string").to_string())
        .collect();
    assert_eq!(
        ports, DECLARED_PORTS,
        "the declaration must be the README's port table, in its order"
    );

    // Every port is a REAL direct child -- a port that names nothing would seal
    // the hive against everything, which is the worst possible typo.
    for p in &ports {
        assert!(
            root.join(p).join("config.json").is_file(),
            "declared port '{p}' is not a direct child of the hive"
        );
    }

    // And the cells that are deliberately NOT ports stay out.
    for interior in ["store", "extractor", "dreamer", "embed", "judge", "cron"] {
        assert!(
            root.join(interior).join("config.json").is_file(),
            "sanity: '{interior}' is a cell of this hive"
        );
        assert!(
            !ports.iter().any(|p| p == interior),
            "'{interior}' must NOT be a port -- that is the whole point of the seal"
        );
    }
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
}

/// The canonical mutation from `templates/memory-hive/README.md` (the block a
/// builder copies), expressed against a hive instantiated at `/memory`.
fn readme_mutation() -> Value {
    json!({"add_edges": [
        {"from": "./anchor",         "to": "./memory/writer"},
        {"from": "./anchor",         "to": "./memory/recall"},
        {"from": "./anchor",         "to": "./memory/extract-glue"},
        {"from": "./memory/recall",       "to": "./capture"},
        {"from": "./memory/extract-glue", "to": "./capture"},
    ]})
}

fn sealed_at(path: &str) -> Vec<SealedHive> {
    vec![SealedHive {
        path: path.to_string(),
        ports: DECLARED_PORTS.iter().map(|s| s.to_string()).collect(),
    }]
}

#[test]
fn the_documented_wiring_stays_legal_under_the_seal() {
    // If the seal rejected the mutation the README hands a builder, the
    // template would be unusable. Both directions are in here: three inbound
    // port edges and two outbound ones.
    validate_hive_port_boundary(&readme_mutation(), "/", &sealed_at("/memory"))
        .expect("the README's own port wiring must stay legal");
}

#[test]
fn a_deep_endpoint_into_the_sealed_memory_hive_is_rejected() {
    for edge in [
        json!({"from": "./anchor", "to": "./memory/store"}),
        json!({"from": "./memory/store", "to": "./void"}),
        json!({"from": "./anchor", "to": "./memory/extractor"}),
        json!({"from": "./memory/dreamer", "to": "./void"}),
    ] {
        let shown = edge.to_string();
        let err =
            validate_hive_port_boundary(&json!({"add_edges": [edge]}), "/", &sealed_at("/memory"))
                .expect_err("a non-port endpoint from outside must reject");
        assert_eq!(
            err.error_code(),
            "hive_port_boundary",
            "wrong code for {shown}"
        );
    }
}

#[test]
fn the_hive_path_itself_and_the_internal_graph_stay_legal() {
    // The transit address...
    validate_hive_port_boundary(
        &json!({"add_edges": [{"from": "./anchor", "to": "./memory"}]}),
        "/",
        &sealed_at("/memory"),
    )
    .expect("the hive path is an address");

    // ...and the hive's own graph, which wires straight at `./store`.
    let internal = json!({"add_edges": [
        {"from": "./writer", "to": "./store"},
        {"from": "./store",  "to": "./recall"},
        {"from": "./embed",  "to": "./store"},
    ]});
    validate_hive_port_boundary(&internal, "/memory", &sealed_at("/memory"))
        .expect("inside the hive every edge stays legal, port or not");
}
