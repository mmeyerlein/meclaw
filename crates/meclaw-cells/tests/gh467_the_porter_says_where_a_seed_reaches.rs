//! GH #467, development-rules § 2d: the drift lock for the porter's claim about
//! where a seed reaches.
//!
//! The porter's public prose used to say that a seeded alias table is a table
//! WITHOUT its key and that a hive receiving aliases that way cannot use them —
//! which reads as *alias tables are not seedable*, and is false. GH #467
//! measured the opposite: the mutation staging seeder reads every
//! `seed/<table>.jsonl` it finds, including tables `params.schema` never names,
//! and the store rebuilds those tables WITH their primary key at first wake
//! (`ensure_keyed_table`, GH #255). What a seed genuinely cannot reach is a
//! `cell.db` that already exists — and that is what the transfer lane is for.
//!
//! A retraction in prose is only a retraction while something reads it, so this
//! test does both halves the ruling asks for:
//!
//!   1. the sentence is grepped on the public surface (`description`), and the
//!      withdrawn claim must be gone rather than merely softened;
//!   2. the mechanism it describes is asserted off the tree — every alias and
//!      rejected-pair table the store names in `params.canonical` is absent
//!      from `params.schema`, which is exactly what makes it a table the store
//!      creates itself and a seeder can fill before the first spawn.
//!
//! Half two is the part that ages: if the alias tables ever move into
//! `params.schema`, the sentence stops being true and this test says so.
//!
//! Guarded like every other template-reading test (GH #49): the public export
//! carries `templates/memory-hive/`, so the guard is a courtesy for a tree that
//! does not.

use meclaw_core::serde_json::Value;
use std::path::PathBuf;

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const PORTER: &str = "templates/memory-hive/porter/config.json";
const STORE: &str = "templates/memory-hive/store/config.json";

fn shipped() -> Option<(Value, Value)> {
    let porter = repo(PORTER);
    let store = repo(STORE);
    if !porter.is_file() || !store.is_file() {
        return None;
    }
    let porter: Value = meclaw_core::serde_json::from_str(&std::fs::read_to_string(porter).ok()?)
        .expect("the porter's config.json is JSON");
    let store: Value = meclaw_core::serde_json::from_str(&std::fs::read_to_string(store).ok()?)
        .expect("the store's config.json is JSON");
    Some((porter, store))
}

/// The whole `description` block as one searchable string. The claim lives in
/// one of its fields today; which field it lives in is not the promise.
fn description_text(porter: &Value) -> String {
    porter
        .get("description")
        .map(|d| d.to_string())
        .unwrap_or_default()
}

/// The alias and rejected-pair tables, read off the store's own `canonical`
/// declaration rather than listed here.
fn store_created_tables(store: &Value) -> Vec<String> {
    let canonical = store
        .pointer("/params/canonical")
        .and_then(Value::as_object)
        .expect("the memory store declares params.canonical");
    let mut out = Vec::new();
    for (_table, dimensions) in canonical {
        for dim in dimensions.as_array().into_iter().flatten() {
            for key in ["aliases", "rejected"] {
                if let Some(name) = dim.get(key).and_then(Value::as_str) {
                    out.push(name.to_string());
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

#[test]
fn the_porter_says_a_seed_reaches_the_alias_tables_and_the_lane_is_for_a_running_hive() {
    let Some((porter, _store)) = shipped() else {
        return;
    };
    let text = description_text(&porter);

    for phrase in [
        "the staging seeder writes them keyless",
        "the store rebuilds them with their key at first wake",
        "this lane is the way into a hive that is already running",
    ] {
        assert!(
            text.contains(phrase),
            "the porter's public description no longer carries {phrase:?}. GH #467 measured \
             that a seed DOES reach the alias tables; the retraction of the older claim is a \
             sentence, and a sentence nobody reads is not a retraction. Move this test with it \
             (development-rules § 2d)."
        );
    }

    assert!(
        !text.contains("WITHOUT its key"),
        "the withdrawn claim is back on the public surface. A seeded alias table arrives \
         keyless and the store rebuilds it keyed at first wake -- writing that it is 'a table \
         WITHOUT its key' reads as 'not seedable', which is the thing GH #467 disproved."
    );
}

#[test]
fn every_alias_table_the_sentence_speaks_of_is_one_the_store_creates_itself() {
    let Some((_porter, store)) = shipped() else {
        return;
    };
    let declared = store
        .pointer("/params/schema")
        .and_then(Value::as_object)
        .expect("the memory store declares params.schema");
    let created = store_created_tables(&store);

    assert!(
        !created.is_empty(),
        "the store names no alias tables in params.canonical, so the porter's sentence about \
         them describes nothing. Either the canonicalisation left the template or this test is \
         reading the wrong key."
    );

    for table in &created {
        assert!(
            !declared.contains_key(table),
            "`{table}` is now declared in params.schema. The porter's description explains that \
             a seed reaches these tables because the STORE creates them (ensure_keyed_table, GH \
             #255) rather than the spawn path -- a table that moved into params.schema is \
             created on the spawn path instead, and the sentence has to move with it."
        );
    }
}
