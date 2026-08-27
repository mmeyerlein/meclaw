//! GH #85 -- a cell instantiated from a template is sandboxed unless it says
//! otherwise.
//!
//! # The migration cut, and why it is prospective
//!
//! Flipping "absent `sandbox` block" from "no sandbox" to "restricted" is a
//! breaking change for every topology in the tree, so it is applied where the
//! repo already applies such changes: at INSTANTIATION, to the node being
//! born, and nowhere else. A tree on disk keeps running exactly as it did; the
//! next node instantiated from a template starts restricted. That is the same
//! shape as the GH #20 secret cut, which changed what instantiation writes and
//! left every existing instance untouched.
//!
//! `cell.provenance` (shipped in 0.4.0) is what makes "template-sourced"
//! answerable: it is stamped by the very call that mints a fresh `cell.id`,
//! and the `adopt` path -- which re-instantiates an existing on-disk node and
//! has no template to name -- deliberately carries none.
//!
//! # What the default is, and what it deliberately is not
//!
//! `{"trust": "restricted", "network": "deny", "filesystem": {"runtime":
//! true}}`. Path-free on purpose: a default that named the cell's own
//! directory would bake an absolute host path into the instantiated
//! `config.json` and break the moment the tree is exported and imported
//! somewhere else -- the very failure GH #20 was opened about. A template that
//! needs paths declares them; the escape hatch stays an explicit
//! `"trust": "trusted"`.

use meclaw_colony::mutation::stage::build_staging_tree_from_templates;
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry};
use meclaw_core::serde_json::{Value as JsonValue, json};
use std::collections::HashMap;
use tempfile::TempDir;

const CONTRACT_TAIL: &str = r#""version":"0.1.0","settings":{},"consumes":{}"#;

/// Register a single-cell template directory under `<root>/templates/<name>`.
fn register_template(root: &std::path::Path, name: &str, config: &str) -> TemplatesRegistry {
    let dir = root.join("templates").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("template.json"), format!(r#"{{"name":"{name}"}}"#)).unwrap();
    std::fs::write(dir.join("config.json"), config).unwrap();
    TemplatesRegistry::from_entries(vec![TemplateEntry {
        template_id: format!("t-{name}"),
        name: name.to_string(),
        version: None,
        filesystem_path: dir,
    }])
}

/// A one-cell template of `cell_type`, with `params` as given.
fn template_config(cell_type: &str, params: JsonValue) -> String {
    format!(
        r#"{{"cell": {{"type": "{cell_type}"}}, "params": {params}, "contract": {{{CONTRACT_TAIL}}}}}"#
    )
}

/// Stage one `add_nodes` instantiation of `template` and return the staged
/// node's runtime params plus the `config.json` written to disk.
fn stage_one(td: &TempDir, template: &str, config: &str) -> (JsonValue, JsonValue) {
    let registry = register_template(td.path(), template, config);
    let diff = json!({"add_nodes": [{"name": "n1", "template": template}]});
    let (staged, _subtrees) = build_staging_tree_from_templates(
        td.path(),
        "m-1",
        "/",
        &diff,
        &registry,
        &HashMap::new(),
        &HashMap::new(),
        &Default::default(),
        &meclaw_colony::WorkPulse::silent(),
    )
    .expect("staging");
    let node = staged.into_iter().next().expect("one staged node");
    let raw = std::fs::read_to_string(node.staging_path.join("config.json")).unwrap();
    let on_disk: JsonValue = meclaw_core::serde_json::from_str(&raw).unwrap();
    (node.params, on_disk)
}

/// The block the cut installs.
fn expected_default() -> JsonValue {
    json!({"trust": "restricted", "network": "deny", "filesystem": {"runtime": true}})
}

// ---- the cut --------------------------------------------------------------

#[test]
fn a_template_sourced_code_cell_without_a_profile_gets_the_restricted_default() {
    let td = TempDir::new().unwrap();
    let (params, on_disk) = stage_one(
        &td,
        "codetpl",
        &template_config(
            "code",
            json!({"runner": "python3", "script_inline": "pass"}),
        ),
    );
    assert_eq!(
        params["sandbox"],
        expected_default(),
        "the cell that is about to spawn must see the default"
    );
    assert_eq!(
        on_disk["params"]["sandbox"],
        expected_default(),
        "and the operator must be able to read it in the instance config, \
         because a boundary nobody can see is one nobody can change"
    );
}

#[test]
fn the_default_covers_every_cell_type_that_enforces_it() {
    for (cell_type, params) in [
        (
            "code",
            json!({"runner": "python3", "script_inline": "pass"}),
        ),
        ("bash", json!({})),
        (
            "harness",
            json!({"adapter": "claude-code", "emit_to": "/out", "workspace_root": "/tmp"}),
        ),
    ] {
        let td = TempDir::new().unwrap();
        let (staged, _) = stage_one(&td, "t", &template_config(cell_type, params));
        assert_eq!(
            staged["sandbox"],
            expected_default(),
            "{cell_type} runs foreign code and must be restricted by default"
        );
    }
}

#[test]
fn a_declared_profile_is_never_overwritten() {
    let td = TempDir::new().unwrap();
    let declared = json!({"trust": "restricted", "filesystem": {"read": ["/srv/data"]}});
    let (params, _) = stage_one(
        &td,
        "declared",
        &template_config(
            "code",
            json!({"runner": "python3", "script_inline": "pass", "sandbox": declared}),
        ),
    );
    assert_eq!(
        params["sandbox"], declared,
        "the template's own profile wins; the default only fills a hole"
    );
}

#[test]
fn trusted_stays_the_escape_hatch() {
    let td = TempDir::new().unwrap();
    let (params, on_disk) = stage_one(
        &td,
        "trusted",
        &template_config("bash", json!({"sandbox": {"trust": "trusted"}})),
    );
    assert_eq!(params["sandbox"]["trust"], "trusted");
    assert_eq!(on_disk["params"]["sandbox"]["trust"], "trusted");
}

#[test]
fn a_cell_type_that_does_not_enforce_a_sandbox_gets_no_block() {
    // Injecting a key into every cell type would hand a `store` or a `hive` a
    // parameter it has no reader for, which is noise at best and a boot error
    // at worst.
    let td = TempDir::new().unwrap();
    let (params, on_disk) = stage_one(&td, "echotpl", &template_config("echo", json!({})));
    assert!(
        params.get("sandbox").is_none(),
        "an echo cell spawns no process and gets no profile, got {params}"
    );
    assert!(on_disk["params"].get("sandbox").is_none());
}

// ---- the prospective half -------------------------------------------------

#[test]
fn the_default_is_written_next_to_the_provenance_that_justifies_it() {
    // The two are one decision: "template-sourced" is exactly what provenance
    // records, so an instance carrying the default must also carry the stamp.
    let td = TempDir::new().unwrap();
    let (_params, on_disk) = stage_one(&td, "codetpl", &template_config("code", json!({})));
    assert_eq!(on_disk["cell"]["provenance"]["template"], "codetpl");
    assert_eq!(on_disk["params"]["sandbox"], expected_default());
}

#[test]
fn the_default_is_a_valid_profile_the_cells_crate_accepts() {
    // The cut would be worthless if the block it writes were rejected at boot
    // by the very parser that has to read it back. `meclaw-colony` cannot see
    // that parser, so the shape is pinned here against the documented schema
    // and proven end to end in the cells crate's own suite.
    let d = expected_default();
    assert_eq!(d["trust"], "restricted");
    assert_eq!(d["network"], "deny");
    assert_eq!(
        d["filesystem"]["runtime"], true,
        "without the runtime set not even the interpreter is reachable"
    );
    assert!(
        d["filesystem"].get("read").is_none() && d["filesystem"].get("write").is_none(),
        "the default names no host path, so an instantiated tree stays portable"
    );
}
