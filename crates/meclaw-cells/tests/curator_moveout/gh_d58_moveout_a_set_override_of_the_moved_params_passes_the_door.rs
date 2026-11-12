//! D58 moveout (GH #1476, plan `u-moveout.md` contract points 5 and 7) -- a
//! set override of the moved params passes the door.
//!
//! WHY this file exists: a knob a script reads is not yet a knob a set can
//! turn. The door (`check_override_params`) lets an override through only
//! for a param the cell declares. The waiting-time knobs (`horizon_advise`,
//! `horizon_defaults_ms`, `horizon_quantile`) move from `curator/policy` to
//! `lead/lead`, and so do the fixed instruction texts of the side calls,
//! shipped with today's texts as their defaults, and the wording of a side
//! call beside an errand (`bridge_wording`, re-check M-1). A set that still names a
//! moved knob on the curator is refused there -- the umstieg the CHANGELOG
//! names; the soul2me sets render it on the lead.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_moveout.rs"]
#[allow(unused_imports)]
use crate::curator_moveout; // harness_fold: mod curator_moveout;

use curator_hive::*;
use curator_moveout::{MOVED_KNOBS, TOOL_WORDS};
use meclaw_colony::mutation::subtree::{check_override_params, parse_subtree};
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use meclaw_core::serde_json::{Map, Value, json};

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

fn door(template: &str, cell: &str, over: &Value) -> Result<(), String> {
    let parsed = parse_subtree(
        &core_root().join("templates").join(template),
        &shipped_registry(),
    )
    .unwrap_or_else(|e| panic!("the shipped {template} template parses: {e:?}"));
    let node = parsed
        .cells
        .into_iter()
        .find(|c| c.rel_path == cell)
        .unwrap_or_else(|| panic!("{template} has a {cell} cell"));
    check_override_params(&node, Some(cell), template, over).map_err(|e| format!("{e:?}"))
}

/// A value a set renders for a moved knob (soul2me's phone surface).
fn set_value(knob: &str) -> Value {
    match knob {
        "horizon_advise" => json!("1"),
        "horizon_defaults_ms" => json!(json!({"weather": 2000, "web": 5000}).to_string()),
        "bridge_wording" => json!("consult:soon,research:later"),
        _ => json!("75"),
    }
}

#[test]
fn gh_d58_moveout_a_set_override_of_the_moved_params_passes_the_door() {
    if !shipped() {
        return;
    }
    let lead = lead_config("lead");
    let mut all = Map::new();
    for knob in MOVED_KNOBS {
        let setting = &lead["contract"]["settings"][knob];
        assert!(
            setting["description"]
                .as_str()
                .is_some_and(|d| !d.is_empty()),
            "lead/lead declares `{knob}` and says what it does: {setting}"
        );
        let over = json!({ knob: set_value(knob) });
        if let Err(why) = door("lead", "lead", &over) {
            panic!("a set's {over} on lead/lead must pass the door: {why}");
        }
        all.insert(knob.to_string(), set_value(knob));
        // The curator declares it no more: a set that still names it there
        // is refused, never silently ignored.
        assert!(
            door("curator", "policy", &over).is_err(),
            "curator/policy still takes `{knob}`"
        );
        let policy = cell_config("policy");
        assert!(
            policy["params"].get(knob).is_none()
                && policy["contract"]["settings"].get(knob).is_none(),
            "curator/policy still ships `{knob}`"
        );
    }
    if let Err(why) = door("lead", "lead", &Value::Object(all.clone())) {
        panic!("all moved knobs at once on lead/lead: {why}");
    }
    assert_eq!(
        lead["params"]["horizon_advise"],
        json!("0"),
        "shipped off, as before"
    );

    // The side texts: a knob of the lead, today's text its default.
    let texts: Vec<(String, Value)> = lead["params"]
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|(k, v)| k != "script_inline" && v.to_string().contains(TOOL_WORDS))
        .collect();
    assert_eq!(
        texts.len(),
        1,
        "one knob of lead/lead ships the tool round's side text: {:?}",
        texts.iter().map(|(k, _)| k).collect::<Vec<_>>()
    );
    let (knob, shipped_text) = &texts[0];
    assert!(
        lead["contract"]["settings"].get(knob.as_str()).is_some(),
        "a set can turn `{knob}`"
    );
    let over = json!({ knob.as_str(): shipped_text.clone() });
    if let Err(why) = door("lead", "lead", &over) {
        panic!("a set's `{knob}` on lead/lead must pass the door: {why}");
    }
    let policy_cfg = cell_config("policy").to_string();
    assert!(
        !policy_cfg.contains(TOOL_WORDS),
        "the text left curator/policy (script and params)"
    );
}
