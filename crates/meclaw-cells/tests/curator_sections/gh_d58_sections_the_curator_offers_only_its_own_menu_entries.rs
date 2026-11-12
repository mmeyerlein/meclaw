//! GH #1473 (D58 sections) -- the curator's menu answer offers its own
//! sections and nothing else.
//!
//! WHY this file exists: until GH #1473 `./schemas` offered `things` too,
//! switched by `things_section` and worded by `things_instruction`. A section
//! another domain keeps is that domain's to offer: every hive answers the
//! menu question with its own half (`sidecar[]`) and the collector merges the
//! halves. So the curator's half is `window` and `gap`, `things` and its two
//! knobs leave `schemas`, and its script names no `things`.
//!
//! The hive runs in one process (`support/curator_hive.rs`). Guarded like
//! every template-reading test (GH #49).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

#[test]
fn the_curator_offers_only_its_own_menu_entries() {
    if !shipped() {
        return;
    }
    let schemas = cell_config("schemas");
    for knob in ["things_section", "things_instruction"] {
        assert!(
            schemas["params"].get(knob).is_none(),
            "curator/schemas has no param `{knob}` any more (GH #1473)"
        );
        assert!(
            schemas["contract"]["settings"].get(knob).is_none(),
            "curator/schemas declares no setting `{knob}` any more (GH #1473)"
        );
    }
    assert!(
        !script_of("schemas").contains("\"things\""),
        "the curator's menu script names no section `things` (GH #1473)"
    );

    let mut h = Hive::new();
    h.out.clear();
    h.lane(
        "in_schemas",
        json!({}),
        json!({}),
        json!({"messages": [], "tools": ["*"]}),
    );
    let answers = h.routed("tool_schemas");
    assert_eq!(answers.len(), 1, "one menu answer: {:?}", h.out);
    let sections: Vec<String> = answers[0].body["sidecar"]
        .as_array()
        .expect("sidecar[]")
        .iter()
        .map(|o| o["section"].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(sections, ["window", "gap"], "the curator's own two");
}
