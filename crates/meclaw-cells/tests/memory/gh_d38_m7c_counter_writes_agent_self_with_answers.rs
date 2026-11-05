//! GH #1423 (M7-C) -- the assistant's reply to a claim is kept as the
//! assistant's own statement, beside the claim it answers.
//!
//! The member passes the doctor's word on ("the mole is harmless"), the
//! companion qualifies it. The closer's (mocked) verdict names that reply under
//! `counter`; the pass files it as a fact of `source` `agent:self`, `stance`
//! `asserted`, `answers` = the claim's id, on the claim's own axis and on the
//! companion's turn -- never as the member's. Through the M7-A ingress the row
//! keeps all of that. Bare script runs, no colony, no model.

// harness_fold: #[path = "support/memory_hive_subject.rs"]
#[allow(unused_imports)]
use crate::support; // harness_fold: mod support;

use meclaw_core::serde_json::json;
use support::*;

#[test]
fn a_counter_is_filed_as_the_assistants_reply() {
    if !shipped() {
        return;
    }
    let facts = [m7c_fact("f-said", M7C_SAID, "harmless", "")];
    let out = m7c_close(
        &m7c_turns(),
        &facts,
        json!({"counter": [{"fact_id": "f-said", "episode_id": M7C_REPLY,
                            "claim": "worth showing to a doctor if it changes",
                            "why": "the assistant qualified the doctor's word"}]}),
    );
    let blocks = m7c_blocks(&out);
    assert_eq!(blocks.len(), 1, "{out:?}");
    let block = &blocks[0];
    assert_eq!(block["episode_id"], M7C_REPLY, "{block}");
    let reply = &block["facts"][0];
    assert_eq!(reply["source"], "agent:self", "{block}");
    assert_eq!(reply["stance"], "asserted", "{block}");
    assert_eq!(reply["answers"], "f-said", "{block}");
    assert_eq!(reply["episode_id"], M7C_REPLY, "{block}");
    // The claim's axis, as the window spells it.
    assert_eq!(reply["subject"], "member", "{block}");
    assert_eq!(reply["predicate"], "has_mole", "{block}");
    assert_eq!(
        reply["claim"], "worth showing to a doctor if it changes",
        "{block}"
    );
    let report = m7c_report(&out);
    assert_eq!(report["countered"], 1, "{report}");
    assert_eq!(report["added"], 0, "{report}");

    // At the ingress (close pass, the companion's own turn): the row is the
    // agent's, it answers the claim, it says it outright.
    let staged = m7a_staged(&m7a_arrive(block["facts"].clone(), true));
    assert_eq!(staged.len(), 1, "{staged:?}");
    let rows = m7a_fact_rows(&m7a_apply(&staged, "", None));
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["source"], "agent:self", "{:?}", rows[0]);
    assert_eq!(rows[0]["answers"], "f-said", "{:?}", rows[0]);
    assert_eq!(rows[0]["stance"], "asserted", "{:?}", rows[0]);
}

#[test]
fn a_counter_never_speaks_for_the_member() {
    if !shipped() {
        return;
    }
    // Whatever the verdict claims about the source, the code decides it.
    let out = m7c_close(
        &m7c_turns(),
        &[m7c_fact("f-said", M7C_SAID, "harmless", "")],
        json!({"counter": [{"fact_id": "f-said", "episode_id": M7C_REPLY,
                            "claim": "worth a second look", "why": "",
                            "source": "person", "stance": "believed"}]}),
    );
    let blocks = m7c_blocks(&out);
    assert_eq!(blocks.len(), 1, "{out:?}");
    assert_eq!(blocks[0]["facts"][0]["source"], "agent:self");
    assert_eq!(blocks[0]["facts"][0]["stance"], "asserted");
}
