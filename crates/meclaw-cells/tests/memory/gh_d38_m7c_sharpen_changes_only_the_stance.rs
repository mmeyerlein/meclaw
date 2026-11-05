//! GH #1423 (M7-C) -- a `sharpen` that names a `stance` changes how firmly
//! the record's source said it, and nothing else.
//!
//! One update on the record (`stance`, `via`), by its id, only while it is
//! open; no replacement block, no new claim, nothing closed; counted as
//! `stance_sharpened`, not as `sharpened`. `via` stays only beside
//! `reported`; a stance that is none of the three drops the item. A `sharpen`
//! without a stance is the old road byte for byte (a replacement through the
//! ingress). Bare script runs, no colony, no model.

// harness_fold: #[path = "support/memory_hive_subject.rs"]
#[allow(unused_imports)]
use crate::support; // harness_fold: mod support;

use meclaw_core::serde_json::json;
use support::*;

#[test]
fn a_stance_sharpen_is_one_update_of_the_status() {
    if !shipped() {
        return;
    }
    let out = m7c_close(
        &m7c_turns(),
        &[m7c_fact("f-said", M7C_SAID, "harmless", "")],
        json!({"sharpen": [{"fact_id": "f-said", "stance": "Reported", "via": " the doctor ",
                            "subject": "member", "predicate": "has_mole",
                            "claim": "certainly harmless", "why": "passed on"}]}),
    );
    assert!(m7c_blocks(&out).is_empty(), "no replacement: {out:?}");
    let ops = m7c_fact_ops(&out);
    assert_eq!(ops.len(), 1, "{ops:?}");
    assert_eq!(ops[0]["operation"], "update", "{ops:?}");
    assert_eq!(
        ops[0]["set"],
        json!({"stance": "reported", "via": "the doctor"}),
        "only the status: {ops:?}"
    );
    assert_eq!(ops[0]["where"]["id"], "f-said", "{ops:?}");
    assert!(
        ops[0]["where"].get("expired_at").is_some(),
        "only an open record: {ops:?}"
    );
    let report = m7c_report(&out);
    assert_eq!(report["stance_sharpened"], 1, "{report}");
    assert_eq!(report["sharpened"], 0, "{report}");
}

#[test]
fn via_stays_only_beside_reported() {
    if !shipped() {
        return;
    }
    let out = m7c_close(
        &m7c_turns(),
        &[m7c_fact("f-said", M7C_SAID, "harmless", "")],
        json!({"sharpen": [{"fact_id": "f-said", "stance": "believed", "via": "the doctor"}]}),
    );
    let ops = m7c_fact_ops(&out);
    assert_eq!(ops.len(), 1, "{ops:?}");
    assert_eq!(ops[0]["set"], json!({"stance": "believed", "via": ""}));
}

#[test]
fn an_unknown_stance_or_an_unshown_record_changes_nothing() {
    if !shipped() {
        return;
    }
    for item in [
        json!({"fact_id": "f-said", "stance": "certain"}),
        json!({"fact_id": "f-invented", "stance": "believed"}),
    ] {
        let out = m7c_close(
            &m7c_turns(),
            &[m7c_fact("f-said", M7C_SAID, "harmless", "")],
            json!({"sharpen": [item.clone()]}),
        );
        assert!(m7c_fact_ops(&out).is_empty(), "{item}: {out:?}");
        assert!(m7c_blocks(&out).is_empty(), "{item}: {out:?}");
        assert_eq!(m7c_report(&out)["stance_sharpened"], 0, "{item}");
    }
}

#[test]
fn a_sharpen_without_a_stance_keeps_the_old_road() {
    if !shipped() {
        return;
    }
    let out = m7c_close(
        &m7c_turns(),
        &[m7c_fact("f-said", M7C_SAID, "harmless", "")],
        json!({"sharpen": [{"fact_id": "f-said", "subject": "member",
                            "predicate": "has_mole", "claim": "harmless, says the doctor",
                            "why": "rough"}]}),
    );
    let blocks = m7c_blocks(&out);
    assert_eq!(blocks.len(), 1, "{out:?}");
    assert_eq!(blocks[0]["facts"][0]["replaces"], "f-said");
    assert!(blocks[0]["facts"][0].get("stance").is_none());
    let report = m7c_report(&out);
    assert_eq!(report["sharpened"], 1, "{report}");
    assert_eq!(report["stance_sharpened"], 0, "{report}");
}

#[test]
fn an_add_carries_its_stance_to_the_ingress() {
    if !shipped() {
        return;
    }
    let out = m7c_close(
        &m7c_turns(),
        &[],
        json!({"add": [{"episode_id": M7C_SAID, "subject": "member", "predicate": "has_mole",
                        "claim": "harmless", "fact_kind": "world", "confidence": "80",
                        "stance": "reported", "via": "the doctor"}]}),
    );
    let blocks = m7c_blocks(&out);
    assert_eq!(blocks.len(), 1, "{out:?}");
    assert_eq!(blocks[0]["facts"][0]["stance"], "reported");
    assert_eq!(blocks[0]["facts"][0]["via"], "the doctor");
    // Without a stance the add is the item it always was.
    let out = m7c_close(
        &m7c_turns(),
        &[],
        json!({"add": [{"episode_id": M7C_SAID, "subject": "member", "predicate": "has_mole",
                        "claim": "harmless", "fact_kind": "world", "confidence": "80"}]}),
    );
    let fact = &m7c_blocks(&out)[0]["facts"][0];
    assert!(
        fact.get("stance").is_none() && fact.get("via").is_none(),
        "{fact}"
    );
}
