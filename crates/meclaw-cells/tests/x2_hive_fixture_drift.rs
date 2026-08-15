//! meclaw-os -- the drift lock behind the memory_drain_colony fixture (GH #125).
//!
//! WHY THIS FILE IS NOT EXPORTED
//! ============================
//! It used to be here because the memory hive was private (0.7.0 ruling: only
//! memory-drain goes public), so the comparison below was one only the private
//! tree could make. That reason is gone: the hive template is public since
//! 2026-08-15. What keeps this file off the export is now the file itself --
//! it, like the rest of the hive test suite, still carries internal shorthand
//! (ruling codes, wave and package names) that has to be translated out before
//! it can ship. That is a separate beat, after launch.
//!
//! WHAT IT GUARDS
//! ==============
//! `memory_drain_colony.rs` used to read the hive template directly, which is
//! why it sat on the blocklist itself. It now boots from
//! `tests/fixtures/memory_drain_colony/`. A snapshot without a lock rots
//! silently -- the hive moves, the public test keeps measuring against last
//! year's writer and stays green while it means nothing. This file is that
//! lock, in three grades, one per fixture:
//!
//! 1. `hive_writer_config.json` -- `cell`, `params` and `contract` BYTE
//!    identical to the shipped writer. The writer script IS the subject of the
//!    colony test, so nothing less than byte identity will do for the part that
//!    runs. The `description` is the one thing that does NOT travel: it is
//!    inert metadata -- discovery prose, never executed -- and the drain's
//!    public contract is a narrower thing than the hive's own writer doc. The
//!    fixture writes its own and the lock below pins that it stays its own.
//! 2. `hive_writer_store_edge.json` -- BYTE identical to the canonical pretty
//!    print of the shipped hive's `writer <-> store` edges. Canonical because
//!    `serde_json::Value` orders object keys (no `preserve_order` feature), so
//!    the pretty print of a parsed value is a stable normal form.
//! 3. `hive_store_config.json` -- a PROJECTION of the shipped store, restricted
//!    to the `episodes` surface. It cannot be byte compared, and that is
//!    deliberate: the full store config would publish the predicate/claim
//!    canon, the judged cardinality and the nightly GC's vocabulary, none of
//!    which the public drain contract documents (GH #125 leak guard). So the
//!    lock has two halves instead: what the projection DOES carry has to equal
//!    the shipped original, and what it does NOT carry is pinned as a ceiling,
//!    so a later refresh cannot quietly widen the public surface.

use meclaw_core::serde_json::{Value, from_str, to_string_pretty};

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn fixture(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/memory_drain_colony")
        .join(name)
}

fn read(p: &std::path::Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

fn parse(p: &std::path::Path) -> Value {
    from_str(&read(p)).unwrap_or_else(|e| panic!("parse {}: {e}", p.display()))
}

/// The canonical form of the hive's `writer <-> store` edges, derived from the
/// shipped hive config exactly the way `memory_drain_colony.rs` used to derive
/// it before the fixture existed.
fn shipped_writer_store_edges() -> Value {
    let hive = parse(&repo("templates/memory-hive/config.json"));
    let edges: Vec<Value> = hive["params"]["graph"]["edges"]
        .as_array()
        .expect("hive edges")
        .iter()
        .filter(|e| {
            let ends = ["./writer", "./store"];
            ends.contains(&e["from"].as_str().unwrap_or_default())
                && ends.contains(&e["to"].as_str().unwrap_or_default())
        })
        .cloned()
        .collect();
    Value::Array(edges)
}

/// Grade 1: the writer travels whole, or the public test measures a ghost.
/// Whole means everything that RUNS -- the script, its runner, its timeout, its
/// contract. Byte equality via the canonical pretty print, so a reformatting of
/// either side is not mistaken for a change.
#[test]
fn the_writer_fixture_is_byte_identical_to_the_shipped_writer() {
    let snap = parse(&fixture("hive_writer_config.json"));
    let live = parse(&repo("templates/memory-hive/writer/config.json"));

    for key in ["cell", "params", "contract"] {
        assert_eq!(
            to_string_pretty(&snap[key]).expect("pretty"),
            to_string_pretty(&live[key]).expect("pretty"),
            "the writer's {key} drifted -- refresh \
             tests/fixtures/memory_drain_colony/hive_writer_config.json from the \
             shipped writer (everything but its description) and re-read the \
             colony test's gates against the new writer"
        );
    }

    // The description does NOT travel, and this is the tripwire that keeps it
    // that way: the drain's public surface documents one table, and the hive
    // writer's own prose speaks about the whole ingress lane.
    assert_ne!(
        snap["description"], live["description"],
        "the snapshot must not carry the shipped writer description"
    );
}

/// Grade 2: the wiring the colony test boots is the hive's own wiring.
#[test]
fn the_edge_fixture_is_byte_identical_to_the_shipped_hive_edge() {
    let shipped = to_string_pretty(&shipped_writer_store_edges()).expect("pretty");
    assert_eq!(
        read(&fixture("hive_writer_store_edge.json")).trim_end(),
        shipped.trim_end(),
        "the writer <-> store edge of the memory hive changed -- refresh \
         tests/fixtures/memory_drain_colony/hive_writer_store_edge.json"
    );
    let n = shipped_writer_store_edges()
        .as_array()
        .expect("array")
        .len();
    assert_eq!(
        n, 1,
        "the write path inside the hive is ONE edge: writer -> store"
    );
}

/// Grade 3a: everything the projection DOES carry is the shipped original.
#[test]
fn the_store_projection_matches_the_shipped_store_where_it_speaks() {
    let snap = parse(&fixture("hive_store_config.json"));
    let live = parse(&repo("templates/memory-hive/store/config.json"));

    assert_eq!(snap["cell"], live["cell"], "cell block drifted");
    assert_eq!(snap["contract"], live["contract"], "store contract drifted");
    assert_eq!(
        snap["params"]["query_timeout_ms"], live["params"]["query_timeout_ms"],
        "query_timeout_ms drifted"
    );
    assert_eq!(
        snap["params"]["schema"]["episodes"], live["params"]["schema"]["episodes"],
        "the episodes schema drifted -- this is the table the drain writes into, \
         so the colony test's count and idempotence gates measure against it; \
         refresh tests/fixtures/memory_drain_colony/hive_store_config.json"
    );
}

/// Grade 3b: the leak ceiling. What the projection must NEVER grow into.
///
/// A future refresh is a copy-paste away from pulling the whole store config
/// across, and that config is the hive's design document: twelve more tables,
/// the canonical bindings of predicate/subject/claim with their alias and
/// refusal tables, the judged cardinality, and a description that names the
/// nightly GC's candidate feed and the dream lane. The drain contract
/// (`templates/memory-drain/README.md`) documents exactly one hive
/// table -- `episodes`. This test holds the public snapshot to that line.
#[test]
fn the_store_projection_publishes_nothing_beyond_the_episodes_surface() {
    let snap = parse(&fixture("hive_store_config.json"));

    let tables: Vec<&str> = snap["params"]["schema"]
        .as_object()
        .expect("schema object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        tables,
        vec!["episodes"],
        "the public snapshot may carry the episodes table and nothing else -- \
         every other table of the hive store is private hive design (GH #125)"
    );

    for forbidden in ["canonical", "fts"] {
        assert!(
            snap["params"].get(forbidden).is_none(),
            "params.{forbidden} is hive identity machinery and must not appear \
             in the public snapshot (GH #125 leak guard)"
        );
    }

    // The shipped description is the leakiest single value in the store config.
    // The snapshot carries its OWN, and this is the tripwire that says so.
    let live = parse(&repo("templates/memory-hive/store/config.json"));
    assert_ne!(
        snap["description"], live["description"],
        "the snapshot must not carry the shipped store description -- it spells \
         out the memory ladder, the identity dimensions and the GC lanes"
    );
}
