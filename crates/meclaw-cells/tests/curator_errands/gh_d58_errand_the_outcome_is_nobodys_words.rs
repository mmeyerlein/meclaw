//! GH #1469 (D58 errand) -- the outcome row is nobody's words.
//!
//! WHY this file exists: the row an errand without an answer leaves is of
//! the kind `outcome`, and its element has the form of the `(delegation)`
//! row it names (origin `user`). Every rule that reads a wall row by its
//! origin alone would take it for the person's words: the transcript a
//! summary is made from said `user: ...` for it (probe on the tree before
//! the change, both cells), so a summary could state the failure of an
//! errand as something the person said (the class of GH #1305). The rules
//! hang on the kind, the same for every role:
//!
//! * the transcripts of `./policy` and `./handover` say `note:`;
//! * its block is of the kind `outcome`, which `window_lost` watches and no
//!   plan shrinks, at the prio of a tool result unless the table names it;
//! * a curate of the errand's own round files nothing: that round is still
//!   at work, and its answer or its next failure decides.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn policy(probe: &str) -> Value {
    let (got, err) = policy_scope(json!({}), probe, Value::Null);
    assert!(!err.contains("Traceback"), "{err}");
    got
}

#[test]
fn a_transcript_reads_the_outcome_as_a_note() {
    if !shipped() {
        return;
    }
    let probe = r#"transcript_line("outcome", {"origin": "user", "type": "text", "text": "x"})"#;
    assert_eq!(policy(probe), json!("note: x"), "./policy");
    let (got, err) = cell_scope("handover", json!({}), probe, Value::Null);
    assert!(!err.contains("Traceback"), "{err}");
    assert_eq!(got, json!("note: x"), "./handover");
}

#[test]
fn the_block_of_an_outcome_is_watched_and_never_shrunk() {
    if !shipped() {
        return;
    }
    let el = r#"{"origin": "user", "type": "text", "text": "x"}"#;
    assert_eq!(
        policy(&format!(r#"block_kind({el}, "outcome")"#)),
        json!("outcome"),
        "a kind of its own"
    );
    assert_eq!(
        policy(r#""outcome" in LOST_KINDS"#),
        json!(true),
        "`window_lost` watches it"
    );
    assert_eq!(
        policy(&format!(r#"shrinkable({{"kind": "outcome"}}, {el})"#)),
        json!(false),
        "no plan shows it in a one-line form"
    );
    assert_eq!(
        policy(r#""outcome" in SCAFFOLD_KINDS"#),
        json!(false),
        "it is no scaffold"
    );
    assert_eq!(
        policy(r#"[prio_of("outcome"), prio_of("tool_result")]"#),
        policy(r#"[prio_of("tool_result"), prio_of("tool_result")]"#),
        "the prio of a tool result while the table does not name the kind"
    );
}

#[test]
fn a_curate_of_the_errands_own_round_files_nothing() {
    if !shipped() {
        return;
    }
    let row =
        r#"{"delegation_id": "dg-1", "round": "p01~5~aaaa1111", "until": 5, "ended": "failed"}"#;
    assert_eq!(
        policy(&format!(r#"errand_outcome({row}, "p01~5~aaaa1111", 9)"#)),
        json!(""),
        "the round of the errand itself"
    );
    assert_eq!(
        policy(&format!(r#"errand_outcome({row}, "t2", 9)"#)),
        json!("failed"),
        "the next turn of the session"
    );
    let silent = r#"{"delegation_id": "dg-1", "round": "p01~5~aaaa1111", "until": 5}"#;
    assert_eq!(
        policy(&format!(
            r#"[errand_outcome({silent}, "t2", 4), errand_outcome({silent}, "t2", 5)]"#
        )),
        json!(["", "no answer"]),
        "nothing before the deadline, `no answer` from it on"
    );
    let answered = r#"{"delegation_id": "dg-1", "round": "p01~5~aaaa1111", "until": 5, "ended": "failed", "answered": 1}"#;
    assert_eq!(
        policy(&format!(r#"errand_outcome({answered}, "t2", 9)"#)),
        json!(""),
        "an answer that reached the wall ends the matter"
    );
}
