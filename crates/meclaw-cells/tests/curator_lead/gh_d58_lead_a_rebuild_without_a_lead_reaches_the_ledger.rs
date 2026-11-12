//! D58 lead (GH #1474, contract point 3) -- a rebuild without a lead reaches
//! the ledger.
//!
//! WHY this file exists: since GH #1447 the bundle of a rebuild (`rb-data`)
//! travels through the switch whenever the summarizer's state line is on
//! (`summary_state_cond`, `state_lines` above 0): the lead reads the
//! session's open errand and hands the bundle back, and only its answer
//! sends it to the ledger. A composite without a lead has nobody to hand
//! it back -- the bundle would end as a dead letter at the curator's
//! boundary and the window would never be condensed. Without a lead the
//! bundle goes to the ledger directly, with no errand open; the summarizer
//! gets its request with the state line, and no lane to a lead leaves.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`) without a lead;
//! no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_lead.rs"]
#[allow(unused_imports)]
use crate::curator_lead; // harness_fold: mod curator_lead;

use curator_hive::*;
use curator_lead::*;
use meclaw_core::serde_json::json;

const FACT: &str = "Hannah lives in Porto.";

#[test]
fn gh_d58_lead_a_rebuild_without_a_lead_reaches_the_ledger() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    // The knobs of `gh_d38_c1_lead_state_lives_in_its_own_cell`, whose last
    // case sends the bundle through the lead.
    let mut h = bare(&[
        ("policy", "keep_recent", json!(1)),
        ("policy", "summary_form", json!("brief")),
        ("policy", "summary_state_cond", json!("1")),
        ("policy", "state_lines", json!(8)),
    ]);
    turn(&mut h, "s1", "t1", "q1", "a1", json!({}));
    // One fact the person set today, so the state line has something to
    // say: the summarizer's line lists `promised`, `open`, `set_today` and
    // never the running topic (`STATE_COND_PARTS`,
    // `gh_d38_z3_state_cond_keeps_the_listed_lines`).
    h.mark(
        backdated(5),
        "s1",
        "t1",
        "fact",
        &canonical(&json!({"subject": "Hannah", "predicate": "lives_in",
                           "claim": FACT, "source": "person", "stance": "asserted"})),
    );
    turn(&mut h, "s1", "t2", "q2", "a2", json!({}));
    turn(
        &mut h,
        "s1",
        "t3",
        "q3",
        "a3",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
    );
    h.out.clear();
    let steps_before = h.ledger_steps.len();
    h.fire(&last_add(&h));
    assert!(
        to_lead(&h).is_empty(),
        "the rebuild asks no lead: {:?}",
        to_lead(&h)
    );
    assert!(
        h.ledger_steps[steps_before..]
            .iter()
            .any(|(sender, phase)| sender == "policy" && phase == "rb-data"),
        "the rebuild's bundle reaches the ledger: {:?} {:?}",
        &h.ledger_steps[steps_before..],
        h.stderr
    );
    assert!(
        !h.summ.is_empty(),
        "the summarizer gets its request: {:?}",
        h.stderr
    );
    let req = h.answer("- person (2026-11-05) garden/state: In bloom.", "stop");
    let transcript = req.messages()[0]["text"]
        .as_str()
        .expect("a transcript")
        .to_string();
    let line = transcript
        .lines()
        .find(|l| l.starts_with("[state -- "))
        .unwrap_or_else(|| panic!("the state line rides behind the transcript: {transcript}"))
        .to_string();
    assert!(
        line.contains(&format!("set today: {FACT}")),
        "the state line of the bundle that reached the ledger: {line:?}"
    );
    assert!(
        !line.contains("open: a consult"),
        "without a lead no errand is open: {line:?}"
    );
    assert!(
        h.lead_ops.is_empty(),
        "without a lead nobody touches its store: {:?}",
        h.lead_ops
    );
}
