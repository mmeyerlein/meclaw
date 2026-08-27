//! GH #325, re-pinned (GH #434): the factory registry and the PUBLISHED
//! catalogue name the same cell types.
//!
//! This assertion used to hang on `templates/builder-hive/g1`'s `KNOWN_TYPES`
//! — a Python set inside a generated JSON string, in a template that is now
//! retired. The claim outlives its old carrier, and its new one is better:
//! `docs/cell-types.md` § Overview is the SHIPPED table a reader measures the
//! tree against, in both languages. A type the registry can spawn but the
//! catalogue does not list is undocumented capability; a row the registry
//! cannot spawn is a promise the tree does not keep.
//!
//! `hive` is in the table and NOT in the registry, on purpose: it is a scope
//! marker, not an actor, and has no factory. `ref` has no row at all
//! (template-time type, no runtime properties) — see the two paragraphs under
//! the table.

use std::collections::BTreeSet;
use std::path::PathBuf;

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The `Type` column of the overview table, minus `hive`.
fn catalogue_types(doc: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut in_table = false;
    for line in doc.lines() {
        if line.starts_with("| Type |") {
            in_table = true;
            continue;
        }
        if in_table {
            if !line.starts_with('|') {
                break;
            }
            let cell = line
                .trim_start_matches('|')
                .split('|')
                .next()
                .unwrap_or("")
                .trim();
            if let Some(name) = cell.strip_prefix('`').and_then(|c| c.strip_suffix('`'))
                && name != "Type"
                && !name.starts_with("---")
                && name != "hive"
            {
                out.insert(name.to_string());
            }
        }
    }
    out
}

#[test]
fn the_catalogue_lists_exactly_the_types_the_registry_can_spawn() {
    let de = std::fs::read_to_string(repo("docs/cell-types.md")).expect("docs/cell-types.md");
    let listed = catalogue_types(&de);
    assert!(
        listed.len() >= 12,
        "the catalogue parsed almost nothing ({listed:?}) — the table's shape moved, \
         not the tree's capability",
    );

    let registry: BTreeSet<String> = meclaw_cli::factories::built_in_factories()
        .keys()
        .map(|k| k.to_string())
        .collect();

    assert_eq!(
        listed,
        registry,
        "docs/cell-types.md § Overview must name exactly what built_in_factories() \
         spawns (`hive` excluded — a scope marker has no factory).\n  \
         listed but not spawnable: {:?}\n  spawnable but not listed: {:?}",
        listed.difference(&registry).collect::<Vec<_>>(),
        registry.difference(&listed).collect::<Vec<_>>(),
    );
}

#[test]
fn both_language_editions_of_the_catalogue_agree() {
    // A half-translated capability list is how a public reader learns something
    // the tree does not do.
    let de = catalogue_types(&std::fs::read_to_string(repo("docs/cell-types.md")).expect("de"));
    let en = catalogue_types(&std::fs::read_to_string(repo("docs/cell-types.en.md")).expect("en"));
    assert_eq!(
        de, en,
        "the two editions of § Overview list different types"
    );
}
