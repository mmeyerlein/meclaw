//! GH #144 -- instantiating the shipped `memory-hive` leaves its egress open.
//!
//! `gh85_template_default_deny.rs` proves the cut in the abstract: a declared
//! profile is never overwritten, an absent one is filled with `network:
//! "deny"`. This file proves it on the tree that actually ships, because the
//! defect was never in the mechanism -- it was in the ONE template cell that
//! forgot to declare, and an abstract test cannot notice that.
//!
//! Both directions are asserted from one staging run: the embed cell keeps the
//! egress it declares, and its neighbours still get the default-deny block. A
//! fix that opened the network for the whole hive would pass the first
//! assertion and fail the second.
//!
//! Guarded like every other template-reading test (GH #49): only a tree that
//! carries the template runs the body.

use meclaw_colony::mutation::stage::build_staging_tree_from_templates;
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry};
use meclaw_core::serde_json::{Value as JsonValue, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn memory_hive() -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("templates/memory-hive");
    p.join("config.json").is_file().then_some(p)
}

/// Every `${VAR}` the tree references WITHOUT a default, bound to a dummy.
///
/// Collected from the template rather than listed here: a missing binding is
/// `EnvVarMissing` and would turn a sandbox regression into an unrelated red
/// the day the hive references one more key. The values are never used -- the
/// staging never calls anything.
fn dummy_env(source: &Path) -> HashMap<String, String> {
    let mut env = HashMap::new();
    let mut stack = vec![source.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(&p) else {
                continue;
            };
            let mut rest = raw.as_str();
            while let Some(start) = rest.find("${") {
                rest = &rest[start + 2..];
                let Some(end) = rest.find('}') else { break };
                let name = &rest[..end];
                if !name.contains(":-")
                    && !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                {
                    env.insert(name.to_string(), format!("dummy-{name}"));
                }
                rest = &rest[end + 1..];
            }
        }
    }
    env
}

/// Stage one instantiation of the real hive and return its staging root.
fn stage_the_hive(td: &tempfile::TempDir, source: PathBuf) -> PathBuf {
    let env = dummy_env(&source);
    let registry = TemplatesRegistry::from_entries(vec![TemplateEntry {
        template_id: "t-memory-hive".into(),
        name: "memory-hive".into(),
        version: None,
        filesystem_path: source,
    }]);
    let diff = json!({"add_nodes": [{"name": "memory", "template": "memory-hive"}]});
    let (staged, subtrees) = build_staging_tree_from_templates(
        td.path(),
        "m-gh144",
        "/",
        &diff,
        &registry,
        &env,
        &HashMap::new(),
    )
    .expect("staging the shipped memory-hive");
    // The hive is a subtree, so it arrives as a rename-root rather than as a
    // flat staged dir: one directory, ready for a single atomic rename.
    staged
        .first()
        .map(|s| s.staging_path.clone())
        .or_else(|| {
            subtrees
                .first()?
                .rename_roots
                .first()
                .map(|r| r.root_staging_path.clone())
        })
        .expect("the hive staged somewhere")
}

/// The `params.sandbox` of the staged cell at `rel`, read back off disk --
/// which is what the operator would read and what the cell will spawn with.
fn staged_sandbox(root: &Path, rel: &str) -> JsonValue {
    let p = root.join(rel).join("config.json");
    let raw =
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read staged {}: {e}", p.display()));
    let cfg: JsonValue = meclaw_core::serde_json::from_str(&raw).expect("staged config.json");
    cfg["params"]["sandbox"].clone()
}

#[test]
fn a_freshly_instantiated_hive_can_still_reach_its_embedder() {
    let Some(source) = memory_hive() else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    let root = stage_the_hive(&td, source);

    assert_eq!(
        staged_sandbox(&root, "embed"),
        json!({"trust": "restricted", "network": "allow", "filesystem": {"runtime": true}}),
        "the semantic leg's only call path must survive instantiation with its egress; \
         inheriting the default-deny block is GH #144 (953/953 calls dead at exit 0)"
    );

    // And the cut still bites everywhere it should: a cell that shuffles JSON
    // has no business on the network, and says so by saying nothing.
    for quiet in ["writer", "recall", "extract-glue", "dream-glue"] {
        assert_eq!(
            staged_sandbox(&root, quiet),
            json!({"trust": "restricted", "network": "deny", "filesystem": {"runtime": true}}),
            "{quiet} declares no profile, so instantiation must still hand it default-deny"
        );
    }
}
