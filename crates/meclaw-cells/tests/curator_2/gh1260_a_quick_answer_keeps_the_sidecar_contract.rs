//! GH #1260 -- a quick answer keeps the sidecar contract.
//!
//! WHY this file exists: since GH #1247 the quick window dropped the sidecar
//! contract, so every turn answered on the fast model left no block -- no
//! needs, no topics, no claims, no moves for the hive behind the splitter. The
//! owner's line (OR-WG-2): the quick answer is an answer like any other and
//! carries the block, the contract fitted to the window's share like the
//! brain's; only the bridge of a `complex`/`research` errand speaks without
//! one (it is not the answer). The leaf `instructions.quick` stays beside it.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

const SECTIONS: [&str; 4] = ["needs", "topics", "claims", "moves"];

fn contract() -> String {
    let mut text = String::from(
        "After your answer -- always after, never instead of it -- append ONE sidecar block.",
    );
    for (i, name) in SECTIONS.iter().enumerate() {
        let kind = if i < 2 { "required" } else { "optional" };
        text.push_str(&format!(
            "\n\n## {name} ({kind})\nWhat the turn says about {name}."
        ));
    }
    text
}

fn curate_on(h: &mut Hive, turn_id: &str, path: &str, ask: &str) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn_id, "iter": "0", "phase": "",
                     "path": path, "families": ""});
    let ctx = json!({"session_id": "s1", "turn_id": turn_id, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": path});
    let system = json!({"instructions": {"mode": {"text": "Be brief."},
                                         "sidecar": {"text": contract()}}});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(ask)], "system": system}),
    );
}

#[test]
fn a_quick_answer_keeps_the_sidecar_contract() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    turn(&mut h, "s1", "t0", "hello", "hi", json!({}));

    curate_on(&mut h, "t1", "simple", "what time is it in Tokyo?");
    let quick = h.routed("quick");
    assert_eq!(quick.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let ins = &quick[0].body["system"]["instructions"];
    let text = ins["sidecar"]["text"].as_str().unwrap_or_default();
    for name in SECTIONS {
        assert!(
            text.contains(&format!("## {name} (")),
            "the quick window carries the section {name} of the offer: {ins:?}"
        );
    }
    assert!(
        ins["quick"]["text"].as_str().is_some_and(|t| !t.is_empty()),
        "`instructions.quick` stays beside the contract: {ins:?}"
    );

    curate_on(&mut h, "t2", "complex", "plan my week around the move");
    let bridge = h.routed("quick");
    assert_eq!(bridge.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(bridge[0].hop["bridge"], json!("consult"));
    assert!(
        bridge[0].body["system"]["instructions"]
            .get("sidecar")
            .is_none(),
        "the bridge is not the answer and carries no contract: {:?}",
        bridge[0].body["system"]
    );
}
