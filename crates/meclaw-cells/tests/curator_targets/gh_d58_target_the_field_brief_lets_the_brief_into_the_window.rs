//! d54g peerfix (GH #1568) -- the profile field `brief` lets the brief lines
//! into the window of a target whose `recall` keeps the memory out.
//!
//! WHY this file exists: the quick window dropped the wall rows of kind
//! `recall` and `brief` together (`FAST_NO_KINDS`), both on the one profile
//! field `recall`. The brief of a round is what affinity says about the
//! counterpart -- on a contact turn the door's persona with its duty to pass
//! things on (K-189, d54g diagnosis 0.67 section 4: the quick voice of the
//! contact turn never saw it). A target that keeps the memory out had no way
//! to see the counterpart. The field `brief` says it on its own; unset it
//! follows `recall`, so a profile that names no `brief` cuts the window of
//! before. The curator holds no name of a role or a product for it (curator
//! omni): the field is the target's word.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;

use curator_hive::*;
use curator_target::*;
use meclaw_colony::mutation::subtree::{check_override_params, parse_subtree};
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use meclaw_core::serde_json::{Value, json};

/// The ambient pair of affinity's brief (collector `call_brief_<16 hex>`).
const BRIEF_ID: &str = "call_brief_00000000000000b1";
/// The ambient pair of the memory (collector `call_recall_<16 hex>`).
const RECALL_ID: &str = "call_recall_00000000000000r1";

/// Whether the last call of a conversation on a target with `profile` shows
/// the brief pair and the recall pair of its round: `(brief, recall)`.
fn seen(profile: Value) -> (bool, bool) {
    let mut h = Hive::with(&[]);
    announce_profile(&mut h, "seat", profile);
    for (i, (words, reply)) in TALK.iter().take(2).enumerate() {
        turn_for(&mut h, "seat", &format!("t{i}"), words, reply);
    }
    let calls = curate_for(
        &mut h,
        Some("seat"),
        "t9",
        0,
        json!([
            tool_call(BRIEF_ID, "affinity_brief"),
            tool_result(
                BRIEF_ID,
                "peer 1a2b3c4d: the workshop's companion; pass results on"
            ),
            tool_call(RECALL_ID, "memory_recall"),
            tool_result(
                RECALL_ID,
                "WHAT WAS SAID\n  user: \"I asked the workshop for the word.\""
            ),
            user("The result you waited for: the word is Lavendel.")
        ]),
        json!({"path": "", "needs": "", "families": ""}),
    );
    let c = the_call(&h, &calls);
    let m = c
        .body
        .get("messages")
        .map(|m| m.to_string())
        .unwrap_or_default();
    (m.contains(BRIEF_ID), m.contains(RECALL_ID))
}

/// `None` when a set's `over` on `talky/quick` passes the door, else why not.
fn refused(over: Value) -> Option<String> {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let scanned = scan_templates_dir(&root.join("templates")).unwrap_or_default();
    let registry = TemplatesRegistry::from_entries(
        scanned
            .into_iter()
            .map(|s| TemplateEntry {
                template_id: format!("scan-{}", s.name),
                name: s.name,
                version: s.version,
                filesystem_path: s.filesystem_path,
            })
            .collect(),
    );
    let parsed = parse_subtree(&root.join("templates/talky"), &registry)
        .unwrap_or_else(|e| panic!("the shipped talky template parses: {e:?}"));
    let node = parsed
        .cells
        .into_iter()
        .find(|c| c.rel_path == "quick")
        .expect("talky has a quick cell");
    check_override_params(&node, Some("quick"), "talky", &over)
        .err()
        .map(|why| format!("a set's {over} on talky/quick must pass the door: {why:?}"))
}

#[test]
fn gh_d58_target_the_field_brief_lets_the_brief_into_the_window() {
    if !shipped() {
        return;
    }
    // Unset, `brief` follows `recall`: the windows of before.
    assert_eq!(
        seen(json!({"recall": true})),
        (true, true),
        "recall on, brief unset"
    );
    assert_eq!(
        seen(json!({"recall": false})),
        (false, false),
        "recall off, brief unset"
    );
    // Set, it decides the brief alone; the memory stays with `recall`.
    assert_eq!(
        seen(json!({"recall": false, "brief": true})),
        (true, false),
        "brief on"
    );
    assert_eq!(
        seen(json!({"recall": true, "brief": false})),
        (false, true),
        "brief off"
    );

    // A set turns it on for a cell through its profile param: the override passes.
    let mut quick = config_of_quick();
    quick["brief"] = json!(true);
    assert_eq!(refused(json!({"window_profile": quick})), None);
}

/// The shipped profile of `talky/quick` (its `window_profile` param).
fn config_of_quick() -> Value {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let cfg: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(root.join("templates/talky/quick/config.json"))
            .expect("quick config"),
    )
    .expect("quick config is JSON");
    assert!(
        cfg["params"]["window_profile"].get("brief").is_none(),
        "shipped: talky/quick names no brief, its window stays the window of before"
    );
    cfg["params"]["window_profile"].clone()
}
