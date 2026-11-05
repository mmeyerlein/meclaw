//! GH #1421 (M7-A) -- a fact keeps HOW FIRMLY its source committed to it.
//!
//! `facts` grows three additive columns (`stance`, `via`, `answers`); this
//! file locks the first: `stance` is one of `asserted`, `believed`,
//! `reported` (an empty or missing value reads as `asserted`). The ingress
//! (`extract-glue`) writes a known stance through to the row, drops a fact
//! whose stance is none of the three with a `stance_unknown` receipt next to
//! what the block could file, and leaves a fact without a stance on the old
//! road byte for byte. Bare script runs, no colony, no model.

// harness_fold: #[path = "support/memory_hive_subject.rs"]
#[allow(unused_imports)]
use crate::support; // harness_fold: mod support;

use meclaw_core::serde_json::{Value, json};
use support::*;

/// The keys a staged fact of the old road carries, and no other.
const OLD_KEYS: [&str; 12] = [
    "audience_set",
    "channel",
    "claim",
    "claim_hash",
    "confidence",
    "episode_id",
    "fact_kind",
    "predicate",
    "replaces",
    "subject",
    "valid_from",
    "valid_until",
];

fn keys(v: &Value) -> Vec<String> {
    v.as_object()
        .expect("an object")
        .keys()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn legacy_hash(claim: &str) -> String {
    probe(
        EXTRACT,
        &format!("print(claim_hash({M7A_EPISODE:?}, 'member', 'has_mole', {claim:?}))"),
    )
}

#[test]
fn the_store_declares_the_three_columns_with_an_empty_default() {
    let schema = &read_json(std::path::Path::new(STORE))["params"]["schema"]["facts"];
    for col in ["stance", "via", "answers"] {
        assert_eq!(
            schema[col],
            json!({"type": "text", "default": ""}),
            "facts.{col}: additive, and a row from before the column reads empty"
        );
    }
    // `confidence` stays, untouched: it is never a truth signal.
    assert_eq!(schema["confidence"], "int");
}

#[test]
fn a_known_stance_is_staged_and_written_to_the_row() {
    if !shipped() {
        return;
    }
    let out = m7a_arrive(
        json!([
            m7a_fact("harmless", json!({"stance": "believed"})),
            m7a_fact(
                "benign",
                json!({"stance": " Reported ", "via": "the doctor"})
            ),
            m7a_fact("on the arm", json!({"stance": "asserted"})),
        ]),
        false,
    );
    assert!(m7a_rejects(&out).is_empty(), "{out:?}");
    let staged = m7a_staged(&out);
    let stances: Vec<&str> = staged
        .iter()
        .map(|f| f["stance"].as_str().unwrap_or("?"))
        .collect();
    assert_eq!(stances, ["believed", "reported", "asserted"], "{staged:?}");

    let rows = m7a_fact_rows(&m7a_apply(&staged, "", None));
    assert_eq!(rows.len(), 3, "{rows:?}");
    let by = |claim: &str| rows.iter().find(|r| r["claim"] == claim).expect(claim);
    assert_eq!(by("harmless")["stance"], "believed");
    assert_eq!(by("benign")["stance"], "reported");
    assert_eq!(by("benign")["via"], "the doctor");
    assert_eq!(by("on the arm")["stance"], "asserted");
}

#[test]
fn an_unknown_stance_drops_its_fact_with_a_receipt() {
    if !shipped() {
        return;
    }
    let out = m7a_arrive(
        json!([
            m7a_fact("harmless", json!({"stance": "certain"})),
            m7a_fact("on the arm", json!({"stance": 3})),
            m7a_fact("small", json!({})),
        ]),
        false,
    );
    let staged = m7a_staged(&out);
    let claims: Vec<&str> = staged.iter().filter_map(|f| f["claim"].as_str()).collect();
    assert_eq!(claims, ["small"], "the rest of the block is filed: {out:?}");
    let rejects = m7a_rejects(&out);
    let unknown: Vec<_> = rejects
        .iter()
        .filter(|(r, _)| r == "stance_unknown")
        .collect();
    assert_eq!(unknown.len(), 1, "{rejects:?}");
    assert!(unknown[0].1.contains("dropped 2 fact(s)"), "{unknown:?}");
    assert!(
        !unknown[0].1.contains("certain"),
        "a receipt counts, it never echoes the model's words: {unknown:?}"
    );
}

#[test]
fn a_missing_stance_is_the_old_road_byte_for_byte() {
    if !shipped() {
        return;
    }
    for extra in [json!({}), json!({"stance": null})] {
        let out = m7a_arrive(json!([m7a_fact("small", extra.clone())]), false);
        assert!(m7a_rejects(&out).is_empty(), "{extra}: {out:?}");
        let staged = m7a_staged(&out);
        assert_eq!(staged.len(), 1, "{out:?}");
        assert_eq!(keys(&staged[0]), OLD_KEYS, "{extra}: {:?}", staged[0]);
        assert_eq!(staged[0]["claim_hash"], legacy_hash("small"), "{extra}");

        let rows = m7a_fact_rows(&m7a_apply(&staged, "", None));
        assert_eq!(rows.len(), 1, "{rows:?}");
        for col in ["stance", "via", "answers"] {
            assert!(
                rows[0].get(col).is_none(),
                "{extra}: the old road names no {col}: {:?}",
                rows[0]
            );
        }
    }
}
