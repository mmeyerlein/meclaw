//! GH #1421 (M7-A) -- `answers` points an agent's REPLY at the statement it
//! answers, and only the close pass writes one.
//!
//! The per-turn block is the front model speaking for its own turn; a reply
//! it files there would be its promise, not a reply it made. So a per-turn
//! `answers` falls away (the fact itself is filed) with an
//! `answers_not_inline` receipt. In a close-pass block, `answers` is kept on
//! a fact of `source = agent:self` and reaches the row; on any other source
//! it falls away with an `answers_not_self` receipt. Bare script runs, no
//! colony, no model.

// harness_fold: #[path = "support/memory_hive_subject.rs"]
#[allow(unused_imports)]
use crate::support; // harness_fold: mod support;

use meclaw_core::serde_json::json;
use support::*;

fn reply() -> meclaw_core::serde_json::Value {
    m7a_fact(
        "a mole that changes is worth showing to a doctor",
        json!({"source": "agent:self", "stance": "asserted", "answers": "f-said"}),
    )
}

#[test]
fn a_per_turn_answers_falls_away_with_a_receipt() {
    if !shipped() {
        return;
    }
    let out = m7a_arrive(json!([reply()]), false);
    let staged = m7a_staged(&out);
    assert_eq!(staged.len(), 1, "the fact itself is filed: {out:?}");
    assert!(staged[0].get("answers").is_none(), "{staged:?}");
    let rejects = m7a_rejects(&out);
    assert!(
        rejects
            .iter()
            .any(|(r, t)| r == "answers_not_inline" && t.contains("1 fact(s)")),
        "{rejects:?}"
    );
    let rows = m7a_fact_rows(&m7a_apply(&staged, "", None));
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].get("answers").is_none(), "{:?}", rows[0]);
}

#[test]
fn a_close_pass_reply_of_the_agent_keeps_its_answers() {
    if !shipped() {
        return;
    }
    let out = m7a_arrive(json!([reply()]), true);
    assert!(m7a_rejects(&out).is_empty(), "{out:?}");
    let staged = m7a_staged(&out);
    assert_eq!(staged[0]["answers"], "f-said", "{staged:?}");
    let rows = m7a_fact_rows(&m7a_apply(&staged, "", None));
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["source"], "agent:self");
    assert_eq!(rows[0]["answers"], "f-said");
    assert_eq!(rows[0]["stance"], "asserted");
}

#[test]
fn a_close_pass_answers_on_another_source_falls_away() {
    if !shipped() {
        return;
    }
    let mut person = reply();
    person["source"] = json!("person");
    let out = m7a_arrive(json!([person]), true);
    let staged = m7a_staged(&out);
    assert_eq!(staged.len(), 1, "{out:?}");
    assert!(staged[0].get("answers").is_none(), "{staged:?}");
    let rejects = m7a_rejects(&out);
    assert!(
        rejects.iter().any(|(r, _)| r == "answers_not_self"),
        "{rejects:?}"
    );
}
