//! D58 judge (GH #1480, contract point 3) -- a domain fills its leaf of the
//! target's section.
//!
//! WHY this file exists: what the judge reads beside the conversation belongs
//! to the domains that own it (the presenter's screen topics, the person's
//! how-questions the affinity brief brings), not to the curator. A domain
//! puts `{target, source, value, subject?, until?}` on `in_part`; the
//! contribution stands as ONE leaf under the name of its source in the
//! family of its target, and a newer contribution of the same source
//! replaces it. Contributions of two sources stand side by side. Keys of the
//! form `judge:*` are gone from the ledger.
//!
//! Runs the shipped `./history` against the hive's ledger.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/history_judge.rs"]
#[allow(unused_imports)]
use crate::history_judge; // harness_fold: mod history_judge;
// harness_fold: #[path = "support/history_view.rs"]
#[allow(unused_imports)]
use crate::history_view; // harness_fold: mod history_view;

use curator_hive::*;
use history_judge::*;
use history_view::*;
use meclaw_core::serde_json::json;

#[test]
fn gh_d58_judge_a_domain_fills_its_leaf_of_the_targets_section() {
    let mut h = Hive::new();
    let params = view_params();
    said_turn(&mut h, 1, "s-now", "t1", "the choir", "the choir");

    // A contribution answers nothing.
    let put = part(&h, &params, "decide", "presenter", json!("an old line"), "");
    assert!(put.out.is_empty(), "in_part answers nothing: {:?}", put.out);
    let _ = part(&h, &params, "decide", "presenter", screen_value(), "");
    let _ = part(&h, &params, "decide", "affinity", hints_value(), "member:e");
    let _ = part(
        &h,
        &params,
        "decide",
        "weather",
        json!("rain from noon"),
        "",
    );

    let view = the_view(&view_ask(&h, &params, "decide", "s-now", "t2"));
    assert_eq!(
        leaf_value(&leaf(&view, "decide", "presenter")),
        screen_value(),
        "the newer contribution of a source replaces its leaf: {:?}",
        view.body["system"]
    );
    assert_eq!(
        leaf_value(&leaf(&view, "decide", "affinity")),
        hints_value(),
        "a contribution about the round's person stands for that round: {:?}",
        view.body["system"]
    );
    assert_eq!(
        leaf(&view, "decide", "weather"),
        json!({"text": "rain from noon"}),
        "a text contribution is a text leaf as it came"
    );
    let family = view.body["system"]["decide"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let mut sources: Vec<&String> = family.keys().collect();
    sources.sort();
    assert_eq!(
        sources,
        vec!["affinity", "presenter", "weather"],
        "one leaf per source, nothing else in the family"
    );

    let judge_keys = h.rows("SELECT key FROM state WHERE key LIKE 'judge:%'");
    assert!(
        judge_keys.is_empty(),
        "no state key `judge:*` any more: {judge_keys:?}"
    );
}
