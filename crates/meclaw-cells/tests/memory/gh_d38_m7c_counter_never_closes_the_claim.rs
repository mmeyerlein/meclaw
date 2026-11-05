//! GH #1423 (M7-C) -- a counter stands next to the claim and ends nothing.
//!
//! "It never replaces the claim, and the claim never absorbs it" (M7 section
//! 4.2, point 5): the reply's fact carries no `replaces`, the block carries no
//! window a closure could use, the pass writes no `expired_at`,
//! `superseded_by` or `closure_source` on the claim, and the ingress closes
//! nothing -- even when the verdict smuggles a `replaces` into the item.
//! Bare script runs, no colony, no model.

// harness_fold: #[path = "support/memory_hive_subject.rs"]
#[allow(unused_imports)]
use crate::support; // harness_fold: mod support;

use meclaw_core::serde_json::json;
use support::*;

#[test]
fn the_claim_stays_open_beside_its_counter() {
    if !shipped() {
        return;
    }
    let out = m7c_close(
        &m7c_turns(),
        &[m7c_fact("f-said", M7C_SAID, "harmless", "")],
        json!({"counter": [{"fact_id": "f-said", "episode_id": M7C_REPLY,
                            "claim": "worth showing to a doctor", "why": "",
                            "replaces": "f-said", "expired_at": "now"}]}),
    );
    let blocks = m7c_blocks(&out);
    assert_eq!(blocks.len(), 1, "{out:?}");
    for f in blocks[0]["facts"].as_array().expect("facts") {
        assert!(
            f.get("replaces").is_none_or(|r| r == "" || r.is_null()),
            "a counter replaces nothing: {f}"
        );
    }
    for op in m7c_fact_ops(&out) {
        for key in ["expired_at", "superseded_by", "closure_source"] {
            assert!(
                op["set"].get(key).is_none(),
                "a counter closes nothing: {op}"
            );
        }
    }
    let report = m7c_report(&out);
    assert_eq!(report["countered"], 1, "{report}");
    for key in ["corrected", "sharpened", "retracted", "closed"] {
        assert_eq!(report[key], 0, "{key}: {report}");
    }

    // The ingress mints the reply and closes no statement -- with or without
    // the window the claim was shown in.
    let staged = m7a_staged(&m7a_arrive(blocks[0]["facts"].clone(), true));
    let window = json!([{"subject": "member", "predicate": "has_mole",
                         "statements": [{"id": "f-said", "claim": "harmless",
                                         "since": "2026-11-05T10:00:00Z",
                                         "last_asserted": "2026-11-05T10:00:00Z"}]}]);
    for w in [None, Some(window)] {
        let applied = m7a_apply(&staged, "", w);
        assert!(
            !applied
                .iter()
                .map(op_args)
                .any(|a| a["operation"] == "update" && a["table"] == "facts"),
            "the claim is never closed by its counter: {applied:?}"
        );
        assert_eq!(m7a_fact_rows(&applied).len(), 1, "{applied:?}");
    }
}

#[test]
fn a_counter_restating_a_standing_record_is_not_filed_twice() {
    if !shipped() {
        return;
    }
    let out = m7c_close(
        &m7c_turns(),
        &[m7c_fact("f-said", M7C_SAID, "harmless", "")],
        json!({"counter": [{"fact_id": "f-said", "episode_id": M7C_REPLY,
                            "claim": "harmless", "why": ""}]}),
    );
    assert!(m7c_blocks(&out).is_empty(), "{out:?}");
    let report = m7c_report(&out);
    assert_eq!(report["countered"], 0, "{report}");
    assert_eq!(report["restated"], 1, "{report}");
}
