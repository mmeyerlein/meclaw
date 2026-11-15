//! GH #1528 (d58-3 ballast) -- a set turns the judge on and chooses its
//! model.
//!
//! WHY this file exists: a knob a script reads is not yet a knob a set can
//! turn. The door (`check_override_params`) lets an override through only
//! for a param the addressed cell declares; an undeclared one is refused when
//! the set is applied, and the judge would stay off in every colony while its
//! tests ran green on a hand-set hive. The judge's model is chosen by the set
//! (a small, inexpensive one), so the door must take it on the judge's cell.
//!
//! Pinned: the door takes `ballast_judge` "1" and "0" on `curator/policy`,
//! and `model`, `reasoning_effort`, `x_title` and `http_referer` on
//! `curator/relevance`; and a hive whose set turns the switch on sends the
//! judge its requests.

// harness_fold: #[path = "support/curator_bank.rs"]
#[allow(unused_imports)]
use crate::curator_bank; // harness_fold: mod curator_bank;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_bank::*;
use meclaw_colony::mutation::subtree::{check_override_params, parse_subtree};
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use meclaw_core::serde_json::json;

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
fn a_set_override_is_taken() {
    let parsed = parse_subtree(&core_root().join("templates/curator"), &shipped_registry())
        .unwrap_or_else(|e| panic!("the shipped curator template parses: {e:?}"));
    let cell = |rel: &str| {
        parsed
            .cells
            .iter()
            .find(|c| c.rel_path == rel)
            .unwrap_or_else(|| panic!("the curator has a cell {rel}"))
    };
    let policy = cell("policy");
    for over in [json!({"ballast_judge": "1"}), json!({"ballast_judge": "0"})] {
        if let Err(why) = check_override_params(policy, Some("policy"), "curator", &over) {
            panic!("a set's {over} on curator/policy must pass the door: {why:?}");
        }
    }
    let judge = cell(JUDGE_CELL);
    for over in [
        json!({"model": "example/small-judge"}),
        json!({"reasoning_effort": "low"}),
        json!({"x_title": "meclaw-lab ballast", "http_referer": "https://example.com"}),
    ] {
        if let Err(why) = check_override_params(judge, Some(JUDGE_CELL), "curator", &over) {
            panic!("a set's {over} on curator/relevance must pass the door: {why:?}");
        }
    }

    let (wall, _) = phone_asked::wall();
    let (h, o) = rebuild_with_hive(&wall, &wall.runs[0]);
    assert!(
        !h.judged.is_empty(),
        "{}: the switch set \"1\", the judge is asked",
        o.who()
    );
}
