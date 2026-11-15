//! GH #1464 (D58 coldgc, contract point 1) -- a set override of `gc_on_cold`
//! passes the door.
//!
//! WHY this file exists: a knob a script reads is not yet a knob a set can
//! turn. The door (`check_override_params`) lets an override through only
//! for a param the cell declares in its `params` block; an undeclared one is
//! refused when the set is applied, and the knob would stay off in every
//! colony while its tests ran green on a hand-set hive. Declared, with its
//! preset "0" in the params and in the contract's settings, a set turns it.
//! No preset of a role sets it: which role collects on a cold cache is the
//! set's word alone. The same holds for `cold_to` (addendum 10.11.), the
//! share of the role's `window_target` a rebuild after an expired cache
//! aims at: declared empty -- the inherited aim of before --, no preset.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
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

#[test]
fn the_knob_is_declared_with_its_shipped_default() {
    let policy = cell_config("policy");
    assert_eq!(
        policy["params"]["gc_on_cold"],
        json!("0"),
        "templates/curator/policy/config.json declares gc_on_cold, shipped \"0\""
    );
    let setting = &policy["contract"]["settings"]["gc_on_cold"];
    assert_eq!(
        setting["default"],
        json!("0"),
        "the setting's default is the param"
    );
    assert_eq!(setting["type"], json!("string"), "{setting}");
    assert!(
        setting["description"]
            .as_str()
            .is_some_and(|d| !d.is_empty()),
        "the setting says what it does: {setting}"
    );
    assert_eq!(
        policy["params"]["cold_to"],
        json!(""),
        "templates/curator/policy/config.json declares cold_to, shipped empty"
    );
    let setting = &policy["contract"]["settings"]["cold_to"];
    assert_eq!(
        setting["default"],
        json!(""),
        "the setting's default is the param"
    );
    assert_eq!(setting["type"], json!("string"), "{setting}");
    assert!(
        setting["description"]
            .as_str()
            .is_some_and(|d| !d.is_empty()),
        "the setting says what it does: {setting}"
    );
}

/// Every key named `name` anywhere in `v`, as a path of keys.
fn keys_named(v: &Value, name: &str, at: &str, found: &mut Vec<String>) {
    match v {
        Value::Object(o) => {
            for (k, x) in o {
                let here = format!("{at}.{k}");
                if k == name {
                    found.push(here.clone());
                }
                keys_named(x, name, &here, found);
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                keys_named(x, name, &format!("{at}[{i}]"), found);
            }
        }
        _ => {}
    }
}

/// Every `config.json` under `dir`, depth first.
fn configs_under(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            configs_under(&p, out);
        } else if p.file_name().is_some_and(|f| f == "config.json") {
            out.push(p);
        }
    }
}

/// The knob is no value of any profile (R-58-15: the former presets of the
/// roles are profile values now): a profile cannot carry it, no example set
/// names it, and no shipped template sets it -- no composite's
/// `override_params`, no model cell's profile, nothing but the declaration
/// of the curator's own policy cell (target review R3 m4: the second part
/// read the table of the test helper alone). So every target reads the
/// shipped "0" until a set says otherwise.
#[test]
fn no_role_presets_the_knob() {
    if !shipped() {
        return;
    }
    let (got, err) = policy_scope(
        json!({}),
        "sorted(k for k in ('gc_on_cold', 'cold_to') if k in ROLE_KEYS)",
        Value::Null,
    );
    assert!(!err.contains("Traceback"), "{err}");
    assert_eq!(got, json!([]), "no profile carries either knob: {got}");
    for name in curator_profiles::VALUE_SETS {
        let set = curator_profiles::value_set(name);
        assert!(
            set.get("gc_on_cold").is_none() && set.get("cold_to").is_none(),
            "{name}: {set}"
        );
    }
    let templates = core_root().join("templates");
    let mut configs = Vec::new();
    configs_under(&templates, &mut configs);
    assert!(
        configs.len() > 10,
        "check of the setup: the shipped templates are read: {}",
        configs.len()
    );
    let mut set_by = Vec::new();
    for path in &configs {
        let rel = path.strip_prefix(&templates).unwrap_or(path);
        let top = rel.iter().next().and_then(|t| t.to_str()).unwrap_or("");
        // The declaration itself: the curator's policy cell, of this
        // version or a frozen one.
        if (top == "curator" || top.starts_with("curator@")) && rel.ends_with("policy/config.json")
        {
            continue;
        }
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let Ok(cfg) = meclaw_core::serde_json::from_str::<Value>(&text) else {
            continue;
        };
        for knob in ["gc_on_cold", "cold_to"] {
            keys_named(&cfg, knob, &rel.display().to_string(), &mut set_by);
        }
    }
    assert!(
        set_by.is_empty(),
        "no shipped template sets either knob: {set_by:?}"
    );
}

#[test]
fn gh_d58_cold_a_set_override_of_gc_on_cold_passes_the_door() {
    let parsed = parse_subtree(&core_root().join("templates/curator"), &shipped_registry())
        .unwrap_or_else(|e| panic!("the shipped curator template parses: {e:?}"));
    let cell = parsed
        .cells
        .into_iter()
        .find(|c| c.rel_path == "policy")
        .expect("the curator has a policy cell");
    for over in [
        json!({"gc_on_cold": "1"}),
        json!({"gc_on_cold": "0"}),
        json!({"cold_to": "0.45"}),
        json!({"cold_to": ""}),
    ] {
        if let Err(why) = check_override_params(&cell, Some("policy"), "curator", &over) {
            panic!("a set's {over} on curator/policy must pass the door: {why:?}");
        }
    }
}
