//! d58-3 treue T2 -- the GH #925 mark `correction` stays what it was.
//!
//! The tap's mark `correction` (GH #925, OR-BD-17) counts the model
//! rewriting a standing instruction, on the turn of the answer, value "".
//! The withdrawal of the own earlier turn reads the same section beside it
//! and books a mark of its own kind on the withdrawn turn; it never changes,
//! moves or replaces the GH #925 mark. A correction with no earlier own
//! answer of the session withdraws nothing, and a block beside a call (no
//! final answer) leaves neither mark. No model is called.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

const CORRECTING: &str = "Let me put that right.\n\n```sidecar\n{\"correction\": \"That was wrong. Do not repeat it.\"}\n```";

#[test]
fn gh_d583_treue_the_rewrite_mark_of_gh925_stays_as_it_was() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.name_main_target();
    // A correction as the first answer of the session: nothing to withdraw.
    turn(&mut h, "s1", "t1", "Hello.", CORRECTING, json!({}));
    // An own answer, then a block beside a call: no final answer, no mark.
    turn(
        &mut h,
        "s1",
        "t2",
        "What is new?",
        "The bakery opened again.",
        json!({}),
    );
    let call = h.curate("s1", "t3", 0, json!([user("Really?")]), mode("Be brief."));
    h.tap(
        &call,
        "tool_calls",
        json!({}),
        json!([tool_call("k1", "consult_cogny"), said(CORRECTING)]),
    );
    assert!(
        h.rows("SELECT turn_id FROM marks WHERE kind = 'withdrawn'")
            .is_empty(),
        "no earlier own answer, or no final answer: nothing is withdrawn"
    );
    // A final answer with the section after an own answer: both marks, each
    // of its own kind and on its own turn.
    let call = h.curate(
        "s1",
        "t4",
        0,
        json!([user("Are you sure?")]),
        mode("Be brief."),
    );
    h.tap(&call, "stop", json!({}), json!([said(CORRECTING)]));
    // The kinds left out are the station's own per-call marks (`notes` is
    // GH #1303: the `consult_cogny` call of t3 leaves t4 the
    // `consult.pending` note) and the withdrawal's own kind.
    let gh925 = h.rows(
        "SELECT kind, value, session_id, turn_id, audience_set FROM marks \
         WHERE kind NOT IN ('curate', 'no_package', 'lead', 'notes', 'withdrawn') ORDER BY seq",
    );
    assert_eq!(
        gh925,
        vec![
            vec![
                json!("correction"),
                json!(""),
                json!("s1"),
                json!("t1"),
                json!(ROUND_E)
            ],
            vec![
                json!("correction"),
                json!(""),
                json!("s1"),
                json!("t4"),
                json!(ROUND_E)
            ],
        ],
        "the GH #925 mark as before: {:?}",
        h.stderr
    );
    assert_eq!(
        h.rows(
            "SELECT session_id, turn_id, value, audience_set FROM marks WHERE kind = 'withdrawn'"
        ),
        vec![vec![json!("s1"), json!("t2"), json!("t4"), json!(ROUND_E)]]
    );
}
