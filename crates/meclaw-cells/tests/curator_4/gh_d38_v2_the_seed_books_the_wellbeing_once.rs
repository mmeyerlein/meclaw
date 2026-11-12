//! GH #1451 (D38 V2, T-7) -- the start window of the next call carries the
//! open wellbeing once, and answers for it once.
//! Since D58 followup (GH #1479) against the hive `assistant` and the curator.
//!
//! WHY this file exists: the pins beside it hold the rule; this one holds
//! the road a call starts on. The person said in a text session that they
//! felt unwell (a fact of the section `memory`, without a confidence, the
//! class `wellbeing` set, GH #1456). It used to be one data line
//! `Wellbeing: {...}` at the head of the seed plus a `wellbeing` mark the seed
//! booked (OR-d38-3.v.2); now the cell `./followup` of `assistant` hands it
//! to talky's curator as a note of scope `once` with a `receipt`: the seed of
//! call 2 carries it ONCE and answers ONE `note_receipt` whose head names the
//! note and the receipt (OR-d58-2.note.1); the curator books no `wellbeing`
//! mark; the seed of call 3 carries none. Off (shipped) no note, no line, no
//! receipt.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/assistant_followup.rs"]
#[allow(unused_imports)]
use crate::assistant_followup; // harness_fold: mod assistant_followup;

use assistant_followup::*;
use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const CLAIM: &str = "Ada feels unwell and slept badly.";

fn unwell() -> Value {
    json!([fact("Ada", "has_symptom", CLAIM, None, true)])
}

/// Pin: call 2 carries it once and answers once; call 3 none.
#[test]
fn the_seed_carries_the_open_wellbeing_once() {
    if !shipped() {
        return;
    }
    let mut a = Assistant::on_at(NOON);
    a.leave("./talky", memory_out("text-1", "t1", unwell()));
    session_ends(&mut a, "./talky", "text-1");
    let sent = live_notes(&a.notes_into("./talky"));
    assert_eq!(sent.len(), 1, "{:?}", a.into);
    let mut h = curator(&[]);
    deliver_notes(&a, "./talky", &mut h);
    let s2 = seed(&mut h, "call-2", "start", 0);
    assert_eq!(count_in(&s2, CLAIM), 1, "{:?}", s2.messages());
    let receipts = h.routed("note_receipt");
    assert_eq!(receipts.len(), 1, "one receipt: {:?}", h.out);
    for k in RECEIPT_KEYS {
        assert!(
            receipts[0].hop.contains_key(k),
            "head {k}: {:?}",
            receipts[0].hop
        );
    }
    assert_eq!(
        field(&receipts[0], "note_id"),
        sent[0]["id"].as_str().unwrap_or("")
    );
    assert_eq!(
        field(&receipts[0], "receipt"),
        sent[0]["receipt"].as_str().unwrap_or("")
    );
    let booked = h.rows("SELECT value FROM marks WHERE kind = 'wellbeing'");
    assert!(
        booked.is_empty(),
        "the curator books no wellbeing: {booked:?}"
    );
    let s3 = seed(&mut h, "call-3", "start", 0);
    assert_eq!(
        count_in(&s3, CLAIM),
        0,
        "asked after once: {:?}",
        s3.messages()
    );
    assert!(h.routed("note_receipt").is_empty(), "{:?}", h.out);
}

/// Pin: off (shipped) no note, no line, no receipt.
#[test]
fn off_the_seed_carries_no_wellbeing() {
    if !shipped() {
        return;
    }
    let mut a = Assistant::with(&[(KNOB_CLOCK, json!(NOON))]);
    a.leave("./talky", memory_out("text-1", "t1", unwell()));
    session_ends(&mut a, "./talky", "text-1");
    assert!(a.into.is_empty(), "{:?}", a.into);
    let mut h = curator(&[]);
    deliver_notes(&a, "./talky", &mut h);
    let s = seed(&mut h, "call-2", "start", 0);
    assert_eq!(count_in(&s, CLAIM), 0, "{:?}", s.messages());
    assert!(h.routed("note_receipt").is_empty());
}
