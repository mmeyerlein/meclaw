//! GH #399 — a `seed/` beside a cell type that can never load it is a refusal,
//! not silence.
//!
//! WHAT THE ISSUE GOT BACKWARDS, AND WHY IT MATTERS HERE
//! ====================================================
//! The issue was filed believing that since GH #398 the types with a fixed
//! schema and no loader were on the `owns_schema() == true` side, so staging
//! already kept out of their databases and the only remaining hole was a quiet
//! nothing. Measured, `web` was the **only** type declaring it, and the seven
//! named types were all still being seeded eagerly by
//! `mutation::stage::seed_cell_db_if_present`.
//!
//! That makes the real defect the mirror image of the filed one, and worse: for
//! a type whose tables are fixed in Rust, staging builds them from a seed header
//! that cannot describe a schema — no primary key, no `NOT NULL`, no `CHECK`, no
//! column order — and the cell's own `CREATE TABLE IF NOT EXISTS` then finds the
//! wrong table standing and leaves it. That is GH #398 exactly, once per type.
//!
//! So this file pins BOTH halves:
//!
//! 1. the six types that own a fixed schema and have no loader **say so**, which
//!    is what makes staging stand down;
//! 2. a `seed/*.jsonl` beside such a type is refused at validation time, naming
//!    the file and the type, instead of being silently ignored.
//!
//! `llm` is deliberately NOT in the list. It is the one type the issue named
//! that has both halves already — its own loader and a `validate_cell_dir` — and
//! its `system` table comes from the shared `persist::setup_cell_db` DDL that
//! staging applies first, so the seeder's `CREATE TABLE IF NOT EXISTS` is a
//! no-op and the keyed schema survives. Flipping it would also shrink the GH
//! #277 golden manifests below their file-count floors.

use meclaw_cells::harness::HarnessCellFactory;
use meclaw_cells::mcp::McpCellFactory;
use meclaw_cells::proxy::ProxyCellFactory;
use meclaw_cells::subcolony::SubcolonyCellFactory;
use meclaw_cells::vault::VaultCellFactory;
use meclaw_cells::{LlmCellFactory, TimerCellFactory};
use meclaw_colony::CellFactory;
use meclaw_core::serde_json::json;
use std::sync::Arc;

/// The six that own a fixed schema and cannot load a seed of their own.
fn schema_owners_without_a_loader() -> Vec<(&'static str, Arc<dyn CellFactory>)> {
    vec![
        (
            "harness",
            Arc::new(HarnessCellFactory) as Arc<dyn CellFactory>,
        ),
        ("mcp", Arc::new(McpCellFactory)),
        ("proxy", Arc::new(ProxyCellFactory)),
        ("subcolony", Arc::new(SubcolonyCellFactory)),
        ("timer", Arc::new(TimerCellFactory)),
        ("vault", Arc::new(VaultCellFactory)),
    ]
}

#[test]
fn the_six_declare_that_they_own_their_schema() {
    for (name, f) in schema_owners_without_a_loader() {
        assert!(
            f.owns_schema(),
            "`{name}` has its tables fixed in Rust, so a seed header cannot \
             describe them — it must declare `owns_schema()` or mutation staging \
             will build those tables constraint-free and the cell's own \
             `CREATE TABLE IF NOT EXISTS` will find them standing (GH #398, \
             reached again through GH #399)"
        );
    }
}

/// The counter-pin. Without it the test above would pass for a tree that had
/// simply flipped every factory to `true`, which would break the types whose
/// tables genuinely ARE per-instance or which load their own seed.
#[test]
fn a_type_that_is_seeded_for_it_does_not_claim_to_own_its_schema() {
    assert!(
        !LlmCellFactory.owns_schema(),
        "`llm` must stay on the seeded side: its `system` table comes from the \
         shared `persist::setup_cell_db` DDL that staging applies BEFORE the \
         seed, so the rows land in a correctly keyed table. Flipping it would \
         also stop `templates/talky/brain/seed/system.jsonl` from being \
         materialised at instantiation, which the GH #277 golden manifests pin."
    );
}

fn cell_dir_with_seed(file: &str) -> tempfile::TempDir {
    let td = tempfile::TempDir::new().expect("tempdir");
    let seed = td.path().join("seed");
    std::fs::create_dir_all(&seed).expect("create seed dir");
    std::fs::write(
        seed.join(file),
        "{\"schema\":{\"a\":\"text\"}}\n{\"a\":\"x\"}\n",
    )
    .expect("write seed");
    td
}

#[test]
fn a_seed_beside_a_type_that_cannot_load_it_is_refused_by_name() {
    for (name, f) in schema_owners_without_a_loader() {
        let td = cell_dir_with_seed("anything.jsonl");
        let err = f
            .validate_cell_dir(&json!({}), td.path())
            .expect_err(&format!(
                "`{name}` owns its schema and has no loader, so `seed/anything.jsonl` \
                 beside it can never load — staying silent leaves an operator \
                 waiting for rows that will never appear (GH #399)"
            ));
        assert!(
            err.contains("anything.jsonl"),
            "the refusal must name the FILE — an operator who wrote one needs to \
             be told which: {err}"
        );
        assert!(
            err.contains(name),
            "the refusal must name the TYPE, because that is the reason the file \
             can never load: {err}"
        );
    }
}

#[test]
fn a_cell_directory_without_a_seed_is_still_fine() {
    let td = tempfile::TempDir::new().expect("tempdir");
    for (name, f) in schema_owners_without_a_loader() {
        f.validate_cell_dir(&json!({}), td.path())
            .unwrap_or_else(|e| panic!("`{name}` refused a directory with no seed at all: {e}"));
    }
    // An empty `seed/` is not a mistake either: nothing was authored, so there
    // is nobody waiting for rows.
    let empty = tempfile::TempDir::new().expect("tempdir");
    std::fs::create_dir_all(empty.path().join("seed")).expect("create seed dir");
    for (name, f) in schema_owners_without_a_loader() {
        f.validate_cell_dir(&json!({}), empty.path())
            .unwrap_or_else(|e| panic!("`{name}` refused an EMPTY seed directory: {e}"));
    }
}

/// A non-`.jsonl` file beside one of them is not this rule's business. The
/// refusal is about a seed an operator authored expecting it to load, and the
/// seed vocabulary is `*.jsonl` — refusing a stray `README` or an editor's
/// backup would be a boot failure over litter.
#[test]
fn a_non_seed_file_in_the_seed_directory_is_not_a_refusal() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let seed = td.path().join("seed");
    std::fs::create_dir_all(&seed).expect("create seed dir");
    std::fs::write(seed.join("NOTES.md"), "not a seed\n").expect("write");
    for (name, f) in schema_owners_without_a_loader() {
        f.validate_cell_dir(&json!({}), td.path())
            .unwrap_or_else(|e| panic!("`{name}` refused a directory over a `NOTES.md`: {e}"));
    }
}
