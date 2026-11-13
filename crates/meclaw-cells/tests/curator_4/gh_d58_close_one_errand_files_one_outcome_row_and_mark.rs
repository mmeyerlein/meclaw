//! GH #1469 (wave d58-2 close, inventory 39 and 44; errand review M1 and
//! m3) -- one errand files one outcome row and one mark.
//!
//! WHY this file exists: the outcome of an errand is filed by whichever comes
//! first -- a curate that reads the parked row, or the sweep that takes it --
//! and each write guards itself (`if_absent`). The guards were keyed on what
//! the write says: the wall row on the hash of its text, the mark on its
//! value with the outcome in it. So two writers that said it differently
//! filed two rows: a curate past the deadline (`no answer`) racing the tap
//! that just heard the call fail (`failed`, review M1), or a curate that
//! named the errand by its wall row beside the sweep that only had
//! `[delegation <id>]` (a park without its head text, review m3). Now both
//! guards are keyed on the errand alone: the round of the errand (its own
//! turn, `<label>~<deadline>~<hex8>`, one per `delegation_id`) and the kind.
//!
//! Runs the pure half of the shipped policy (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const INNER: &str = "p01~1000~aaaa1111";

/// The guards of the wall row (`eo-w0`) and the mark (`eo-m0`) the policy
/// files for `row`: by a curate of another turn that read `found`, or by the
/// sweep (`sweep`).
fn guards(row: Value, found: Value, sweep: bool) -> Value {
    let probe = "(lambda ops: {'wall': ops['eo-w0'].get('if_absent'), \
                 'mark': ops['eo-m0'].get('if_absent'), \
                 'text': json.loads(ops['eo-b0']['row']['body'])['text']})(\
                 dict(outcome_ops({'pending:delegation:s#p01': json.dumps(ARGS[0])}, \
                 ARGS[1], 's', 'p02', ['member:e'], '2026-03-02T10:00:00.000000Z', 2000, \
                 ARGS[2])[1]))";
    let (got, err) = policy_scope(json!({}), probe, json!([row, found, sweep]));
    assert!(!err.contains("Traceback"), "{err}");
    got
}

fn park(extra: Value) -> Value {
    let mut r = json!({"delegation_id": "dg-aaaa1111", "round": INNER, "until": 1000,
                       "parked_at": "2026-03-02T09:00:00.000000Z",
                       "audience_set": ["member:e"], "errand_session": "s",
                       "errand_round": ["member:e"]});
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        r[k] = v;
    }
    r
}

#[test]
fn one_errand_files_one_outcome_row_and_one_mark() {
    if !shipped() {
        return;
    }
    let found = json!([{"seq": 3, "turn_id": INNER, "kind": "delegation",
                        "body": json!({"origin": "user", "type": "text",
                                       "text": "[delegation item_aaaa1111: find out]"})
                                .to_string()}]);
    // Review M1: past the deadline a curate says `no answer`; the tap that
    // heard the call fail says `failed` to the next one.
    let silent = guards(park(json!({})), found.clone(), false);
    let failed = guards(park(json!({"ended": "failed"})), found.clone(), false);
    // Review m3: a park without its head text -- the curate names the errand
    // by its wall row, the sweep has only `[delegation <id>]`.
    let swept = guards(park(json!({})), json!([{"seq": 5}]), true);
    assert!(
        silent["text"] != failed["text"] && silent["text"] != swept["text"],
        "check of the setup: the three writers say it differently: {silent} {failed} {swept}"
    );
    for (name, g) in [("failed", &failed), ("sweep", &swept)] {
        assert_eq!(
            g["wall"], silent["wall"],
            "the wall row of the outcome is guarded by the errand, not by its words \
             ({name}): {silent} {g}"
        );
        assert_eq!(
            g["mark"], silent["mark"],
            "the mark of the outcome is guarded by the errand, not by its value \
             ({name}): {silent} {g}"
        );
    }
    assert_eq!(
        silent["wall"]["turn_id"],
        json!(INNER),
        "the guard names the errand's round: {silent}"
    );
    assert_eq!(
        silent["mark"]["turn_id"],
        json!(INNER),
        "the guard names the errand's round: {silent}"
    );
}
