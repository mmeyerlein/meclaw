//! GH #1451 (D38 V2, T-7) -- a mentioned wellbeing closes; never past the day.
//!
//! WHY this file exists: the open wellbeing is asked after ONCE ("ask lightly
//! once, then follow"). The seed that carried the line books a mark
//! `wellbeing {state: "closed", fact_seq}`; from then on `wellbeing_line`
//! gives nothing for that fact, in the seed and in the feed alike. A fact of
//! another day never opens a line. Knob `wellbeing_line` (see the sibling
//! file `gh_d38_v2_the_next_call_knows_the_open_wellbeing_once.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn fact(seq: i64, at: &str) -> Value {
    json!({"seq": seq, "kind": "fact", "session_id": "text-1", "at": at,
           "value": canonical(&json!({"subject": "Ada", "predicate": "health",
                                      "claim": "Ada feels unwell.", "source": "person",
                                      "stance": "asserted", "confidence": 0.9}))})
}

fn line(cell: &str, marks: &Value) -> Value {
    let (got, err) = cell_scope(
        cell,
        json!({"wellbeing_line": "1"}),
        "wellbeing_line(ARGS['marks'], '2026-11-07')",
        json!({"marks": marks}),
    );
    assert!(!err.contains("Traceback"), "{cell}: {err}");
    got
}

/// Pin: after the closing mark no line, in both cells.
#[test]
fn a_mentioned_wellbeing_closes() {
    if !shipped() {
        return;
    }
    let closed = json!([
        {"seq": 9, "kind": "wellbeing", "session_id": "phone-2", "at": "2026-11-07T10:00:00Z",
         "value": canonical(&json!({"state": "closed", "fact_seq": 2}))},
        fact(2, "2026-11-07T08:55:00Z")
    ]);
    for cell in ["history", "policy"] {
        assert_eq!(
            line(cell, &closed),
            Value::Null,
            "{cell}: closed after the mention"
        );
    }
}

/// Pin: a fact of yesterday opens nothing today.
#[test]
fn a_wellbeing_never_outlives_its_day() {
    if !shipped() {
        return;
    }
    let old = json!([fact(2, "2026-11-06T21:00:00Z")]);
    for cell in ["history", "policy"] {
        assert_eq!(line(cell, &old), Value::Null, "{cell}: not past the day");
    }
}
