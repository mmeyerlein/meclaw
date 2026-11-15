//! GH #1539 (d58-3 nachlese, oc9) -- an answer of the summarizer that ends
//! inside its last line (one line of the form, then a cut-off line without a
//! source pointer `[#<id>]`) is no brief: it is cut off, not whole, the old
//! window stays, stderr names the cut (`summary_rejected: cut off inside a
//! line`), and the `summary_rejected` mark says how many lines were kept and
//! dropped. The counter case: a dropped last line WITH its pointer (a made-up
//! day) is a whole line -- the brief stands, as one kept against one dropped
//! stood before. Two whole lines and a cut one are cut off as well: the
//! shape of the last line decides, not the count.
//!
//! WHY this file exists: in the summary bank of d58-3 one answer (role talky,
//! wall `over_aim_state`) was one line and a line cut off mid-sentence; with
//! one kept and one dropped the policy took it as a brief, and one line stood
//! in for a transcript of 32k tokens. The shape of the last line, no meaning.
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

/// A hive with a first brief in its window and the second rebuild
/// asked for; the slot hash of the first brief.
fn second_rebuild() -> (Hive, Value) {
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
    (h, slot)
}

#[test]
fn a_brief_cut_off_inside_a_line_leaves_the_old_window() {
    if !shipped() {
        return;
    }
    let (mut h, slot) = second_rebuild();
    let q3 = short_id(&user("q3"));
    let day = h.transcript_day();
    // One line of the form, then the answer ends inside its second line:
    // no status, no source pointer.
    let answer = [
        format!("- person ({day}) garden/state: The garden is in bloom. [asserted] [#{q3}]"),
        format!("- person ({day}) choir/course_room: the course of the choir is now in"),
    ];
    h.answer(&answer.join("\n"), "stop");
    assert_eq!(
        h.rows("SELECT COUNT(*) FROM summaries")[0][0],
        json!(1),
        "a brief cut off inside its last line is no brief: no second summary"
    );
    assert_eq!(
        summary_slot_hash(&h),
        slot,
        "the window keeps the old brief"
    );
    assert_eq!(
        rejected_marks(&h),
        vec![json!({"kept": 1, "dropped": 1})],
        "the rejection is said, with its count"
    );
    assert!(
        h.stderr
            .iter()
            .any(|l| l.contains("summary_rejected: cut off inside a line")),
        "the failure names its cause: {:?}",
        h.stderr
    );
    assert_eq!(
        h.state_in("rebuild_running", ROUND_E),
        "",
        "and the rebuild is over"
    );
}

#[test]
fn a_whole_last_line_with_its_pointer_keeps_the_brief() {
    if !shipped() {
        return;
    }
    let (mut h, slot) = second_rebuild();
    let q3 = short_id(&user("q3"));
    let day = h.transcript_day();
    // One line of the form, then a whole line with its source pointer whose
    // day the transcript never showed: dropped, but not cut off.
    let answer = [
        format!("- person ({day}) garden/state: The garden is in bloom. [asserted] [#{q3}]"),
        format!(
            "- person (2001-01-01) choir/course_room: The choir sings on Fridays. [asserted] [#{q3}]"
        ),
    ];
    h.answer(&answer.join("\n"), "stop");
    assert_eq!(
        h.rows("SELECT COUNT(*) FROM summaries")[0][0],
        json!(2),
        "a whole last line with its pointer is no cut: the brief stands {:?}",
        h.stderr
    );
    assert_ne!(
        summary_slot_hash(&h),
        slot,
        "the window shows the new brief"
    );
    assert_eq!(
        rejected_marks(&h),
        vec![json!({"kept": 1, "dropped": 1})],
        "the dropped line is said, with its count"
    );
    assert!(
        !h.stderr.iter().any(|l| l.contains("summary_rejected")),
        "no rejection: {:?}",
        h.stderr
    );
    assert_eq!(
        h.state_in("rebuild_running", ROUND_E),
        "",
        "and the rebuild is over"
    );
}

#[test]
fn two_lines_and_a_cut_one_still_leave_the_old_window() {
    if !shipped() {
        return;
    }
    let (mut h, slot) = second_rebuild();
    let q3 = short_id(&user("q3"));
    let day = h.transcript_day();
    // Two lines of the form, then the answer ends inside its third line:
    // more kept than dropped, still cut off -- the shape of the last line,
    // not the count, says it.
    let answer = [
        format!("- person ({day}) garden/state: The garden is in bloom. [asserted] [#{q3}]"),
        format!(
            "- person ({day}) garden/gate: The gate of the garden is green. [asserted] [#{q3}]"
        ),
        format!("- person ({day}) choir/course_room: the course of the choir is now in"),
    ];
    h.answer(&answer.join("\n"), "stop");
    assert_eq!(
        h.rows("SELECT COUNT(*) FROM summaries")[0][0],
        json!(1),
        "two whole lines and a cut one are no brief: no second summary {:?}",
        h.stderr
    );
    assert_eq!(
        summary_slot_hash(&h),
        slot,
        "the window keeps the old brief"
    );
    assert_eq!(
        rejected_marks(&h),
        vec![json!({"kept": 2, "dropped": 1})],
        "the rejection is said, with its count"
    );
    assert!(
        h.stderr
            .iter()
            .any(|l| l.contains("summary_rejected: cut off inside a line")),
        "the failure names its cause: {:?}",
        h.stderr
    );
    assert_eq!(
        h.state_in("rebuild_running", ROUND_E),
        "",
        "and the rebuild is over"
    );
}
