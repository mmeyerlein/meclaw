//! GH #1421 (M7-A) -- `stance` and `via` belong to the identity of a
//! statement (`claim_hash`), the way `source` does.
//!
//! "Hopes it is harmless" and "the doctor says it is harmless" are two
//! statements, so they hash apart, and the (episode, claim_hash) dedup keeps
//! both. `asserted` is what an empty stance reads as, so an explicit
//! `asserted` hashes exactly like a fact without a stance -- and that is the
//! hash of the old road. Bare script runs, no colony, no model.

// harness_fold: #[path = "support/memory_hive_subject.rs"]
#[allow(unused_imports)]
use crate::support; // harness_fold: mod support;

use meclaw_core::serde_json::{Value, json};
use support::*;

fn hash_of(extra: Value) -> String {
    let staged = m7a_staged(&m7a_arrive(json!([m7a_fact("harmless", extra)]), false));
    assert_eq!(staged.len(), 1, "{staged:?}");
    staged[0]["claim_hash"]
        .as_str()
        .expect("a hash")
        .to_string()
}

#[test]
fn the_stance_and_the_via_move_the_hash() {
    if !shipped() {
        return;
    }
    let plain = hash_of(json!({}));
    let asserted = hash_of(json!({"stance": "asserted"}));
    let believed = hash_of(json!({"stance": "believed"}));
    let reported = hash_of(json!({"stance": "reported"}));
    let doctor = hash_of(json!({"stance": "reported", "via": "the doctor"}));
    let paper = hash_of(json!({"stance": "reported", "via": "the paper"}));

    assert_eq!(asserted, plain, "asserted is what no stance reads as");
    assert_eq!(
        plain,
        probe(
            EXTRACT,
            &format!("print(claim_hash({M7A_EPISODE:?}, 'member', 'has_mole', 'harmless'))")
        ),
        "a fact without a stance keeps the hash of the old road"
    );
    let all = [&plain, &believed, &reported, &doctor, &paper];
    for (i, a) in all.iter().enumerate() {
        for b in &all[i + 1..] {
            assert_ne!(a, b, "two statements, one hash: {all:?}");
        }
    }
}

#[test]
fn the_dedup_keeps_a_belief_beside_the_same_words_asserted() {
    if !shipped() {
        return;
    }
    let out = m7a_arrive(
        json!([
            m7a_fact("harmless", json!({})),
            m7a_fact("harmless", json!({"stance": "believed"})),
        ]),
        false,
    );
    let rows = m7a_fact_rows(&m7a_apply(&m7a_staged(&out), "", None));
    assert_eq!(rows.len(), 2, "{rows:?}");
}
