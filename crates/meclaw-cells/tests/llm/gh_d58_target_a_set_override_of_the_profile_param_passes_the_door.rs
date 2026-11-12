//! D58 target (GH #1483, contract points 2 and 12) -- a set override of the
//! profile param passes the door.
//!
//! WHY this file exists: a param a cell reads is not yet a param a set can
//! turn. The door (`check_override_params`) lets an override through only
//! for a param the cell declares in its `params` block and in
//! `contract.settings`; an undeclared one is refused when the set is
//! applied, and the profile would stay the template's in every colony. Every
//! model cell a curator cuts for -- four in `talky`, one in `cogny`, two in
//! `simple-agent` -- declares `window_profile`, has `package_line` on (or it
//! never reports), and a set's override of the profile passes the door.

use meclaw_colony::mutation::subtree::{check_override_params, parse_subtree};
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use meclaw_core::serde_json::{Value, json};

fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn shipped_registry() -> TemplatesRegistry {
    let scanned = scan_templates_dir(&core_root().join("templates")).unwrap_or_default();
    TemplatesRegistry::from_entries(
        scanned
            .into_iter()
            .map(|s| TemplateEntry {
                template_id: format!("scan-{}", s.name),
                name: s.name,
                version: s.version,
                filesystem_path: s.filesystem_path,
            })
            .collect(),
    )
}

fn read(rel: &str) -> Value {
    let p = core_root().join(rel);
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The model cells the curators of the shipped composites cut for.
const TARGETS: [(&str, &str); 7] = [
    ("talky", "brain"),
    ("talky", "quick"),
    ("talky", "quick_spec"),
    ("talky", "quick_bridge"),
    ("cogny", "brain"),
    ("simple-agent", "brain"),
    ("simple-agent", "strong"),
];

#[test]
fn gh_d58_target_a_set_override_of_the_profile_param_passes_the_door() {
    if !core_root().join("templates/talky/config.json").is_file() {
        return;
    }
    let registry = shipped_registry();
    let mut failures = Vec::new();
    for (composite, cell) in TARGETS {
        let cfg = read(&format!("templates/{composite}/{cell}/config.json"));
        if cfg["params"]["package_line"] != json!(true) {
            failures.push(format!("{composite}/{cell}: `package_line` is not on"));
        }
        if cfg["params"].get("window_profile").is_none() {
            failures.push(format!(
                "{composite}/{cell}: `params` declares no `window_profile`"
            ));
        }
        if cfg["contract"]["settings"].get("window_profile").is_none() {
            failures.push(format!(
                "{composite}/{cell}: `contract.settings` declares no `window_profile`"
            ));
        }
        let parsed = match parse_subtree(&core_root().join("templates").join(composite), &registry)
        {
            Ok(p) => p,
            Err(e) => {
                failures.push(format!("{composite}: the template does not parse: {e:?}"));
                continue;
            }
        };
        let Some(found) = parsed.cells.into_iter().find(|c| c.rel_path == cell) else {
            failures.push(format!("{composite}: no cell {cell}"));
            continue;
        };
        for over in [
            json!({"window_profile": {"keep_recent": 6, "speaks": true}}),
            json!({"window_profile": {}}),
        ] {
            if let Err(why) = check_override_params(&found, Some(cell), composite, &over) {
                failures.push(format!("{composite}/{cell}: {over} is refused: {why:?}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
