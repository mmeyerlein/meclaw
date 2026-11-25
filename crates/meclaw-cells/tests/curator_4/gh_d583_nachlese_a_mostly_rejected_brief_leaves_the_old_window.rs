//! GH #1539 (d58-3 nachlese) -- an answer of the summarizer whose lines are
//! mostly not of the form (more dropped than kept) is no brief: it fails as
//! an answer without a line of the form does (`summary_rejected: most lines
//! not of the form`), the old window stays, and the `summary_rejected` mark
//! says how many lines were kept and dropped. As many dropped as kept is
//! still a brief (the mixed answer of `gh_d38_z3_prose_is_rejected_...`).
//!
//! WHY this file exists: in the summary bank of d58-3 one answer (role
//! consult) kept 1 of 44 lines; one valid line made the answer a success,
//! and a one-line brief stood in for the whole transcript -- 22 planted
//! points gone, only a mark said so. A count, no meaning.
//!
//! Stub summarizer (`support/curator_hive.rs` `Hive::answer`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn summary_slot_hash(h: &Hive) -> Value {
    h.rows(&format!(
        "SELECT hash FROM slots WHERE path = '{}'",
        summary_slot(ROUND_E)
    ))
    .first()
    .map(|r| r[0].clone())
    .unwrap_or(Value::Null)
}

fn rejected_marks(h: &Hive) -> Vec<Value> {
    h.rows("SELECT value FROM marks WHERE kind = 'summary_rejected' ORDER BY seq")
        .into_iter()
        .map(|r| meclaw_core::serde_json::from_str(r[0].as_str().unwrap()).expect("json"))
        .collect()
}

#[test]
fn a_mostly_rejected_brief_leaves_the_old_window() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "keep_recent", json!(1)),
        ("policy", "summary_form", json!("brief")),
    ]);
    turn(&mut h, "s1", "t1", "q1", "a1", json!({}));
    turn(&mut h, "s1", "t2", "q2", "a2", json!({}));
    turn(
        &mut h,
        "s1",
        "t3",
        "q3",
        "a3",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
    );
    h.fire(&last_add(&h));
    let q1 = short_id(&user("q1"));
    let day = h.transcript_day();
    h.answer(
        &format!("- person ({day}) hannah/lives_in: Hannah lives in Lisbon. [asserted] [#{q1}]"),
        "stop",
    );
    assert_eq!(
        h.rows("SELECT COUNT(*) FROM summaries")[0][0],
        json!(1),
        "check of the setup: the first brief {:?}",
        h.stderr
    );
    let slot = summary_slot_hash(&h);
    assert!(!slot.is_null(), "the round shows the first brief");

    turn(
        &mut h,
        "s1",
        "t4",
        "q4",
        "a4",
        json!({"cache_expires_at": "2099-01-01T00:01:00Z"}),
    );
    h.fire(&last_add(&h));
    let q3 = short_id(&user("q3"));
    let day = h.transcript_day();
    let mut answer = vec![format!(
        "- person ({day}) garden/state: The garden is in bloom. [asserted] [#{q3}]"
    )];
    // Five lines without any reading: no speaker the form knows.
    for i in 0..5 {
        answer.push(format!(
            "- speaker ({day}) item_{i}/value: something said [asserted] [#{q3}]"
        ));
    }
    h.answer(&answer.join("\n"), "stop");
    assert_eq!(
        h.rows("SELECT COUNT(*) FROM summaries")[0][0],
        json!(1),
        "one kept line of six is no brief: no second summary"
    );
    assert_eq!(
        summary_slot_hash(&h),
        slot,
        "the window keeps the old brief"
    );
    assert_eq!(
        rejected_marks(&h),
        vec![json!({"kept": 1, "dropped": 5})],
        "the rejection is said, with its count"
    );
    assert!(
        h.stderr
            .iter()
            .any(|l| l.contains("summary_rejected: most lines not of the form")),
        "the failure names its cause: {:?}",
        h.stderr
    );
    assert_eq!(
        h.state_in("rebuild_running", ROUND_E),
        "",
        "and the rebuild is over"
    );
}
