//! GH #1451 (D38 V2, T-7) -- a mentioned wellbeing closes; never past the day.
//! Since D58 followup (GH #1479) against the hive `assistant` and the curator.
//!
//! WHY this file exists: the open wellbeing is asked after ONCE ("ask lightly
//! once, then follow"). It used to close by a `wellbeing` mark the seed
//! booked; now the start window that showed the note answers `note_receipt`,
//! the receipt comes home to the cell `./followup` of `assistant`, and the
//! concern is closed: the same fact said again that day opens nothing. A fact
//! of one day never opens anything on the next: the day is the person's day,
//! from the instance's time zone and the cell's test clock (`now_utc`), never
//! the wall clock; the note the cell sent ends with that day
//! (OR-d58-2.followup.4: the note carries `until` the end of the day, the
//! curator's clock in the pin is `history.now_utc`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/assistant_followup.rs"]
#[allow(unused_imports)]
use crate::assistant_followup; // harness_fold: mod assistant_followup;

use assistant_followup::*;
use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const CLAIM: &str = "Ada feels unwell.";

fn unwell() -> Value {
    json!([fact("Ada", "has_symptom", CLAIM, Some(0.9), true)])
}

/// Pin: after the receipt the concern is closed; said again, nothing new.
#[test]
fn a_mentioned_wellbeing_closes() {
    if !shipped() {
        return;
    }
    let mut a = Assistant::on_at(NOON);
    a.leave("./talky", memory_out("text-1", "t1", unwell()));
    session_ends(&mut a, "./talky", "text-1");
    let mut h = curator(&[]);
    deliver_notes(&a, "./talky", &mut h);
    let s = seed(&mut h, "phone-2", "start", 0);
    assert_eq!(count_in(&s, CLAIM), 1, "{:?}", s.messages());
    a.clear();
    assert_eq!(receipts_home(&mut a, "./talky", &h), 1);
    a.clear();
    a.leave("./talky", memory_out("text-1", "t4", unwell()));
    session_ends(&mut a, "./talky", "text-1");
    for child in COMPOSITES {
        assert!(
            live_notes(&a.notes_into(child)).is_empty(),
            "closed after the mention: {:?}",
            a.into
        );
    }
}

/// Pin: a concern of yesterday is not shown today.
#[test]
fn a_wellbeing_never_outlives_its_day() {
    if !shipped() {
        return;
    }
    let mut a = Assistant::on_at(NOON);
    a.leave("./talky", memory_out("text-1", "t1", unwell()));
    session_ends(&mut a, "./talky", "text-1");
    let mut h = curator(&[("history", "now_utc", json!(NEXT_DAY_NOON))]);
    deliver_notes(&a, "./talky", &mut h);
    let s = seed(&mut h, "phone-2", "start", 0);
    assert_eq!(
        count_in(&s, CLAIM),
        0,
        "not past the day: {:?}",
        s.messages()
    );
    assert!(
        h.routed("note_receipt").is_empty(),
        "nothing shown, nothing receipted: {:?}",
        h.out
    );

    // The day is the person's: 23:30 UTC on the 8th is the 9th in Berlin,
    // whose day ends at 23:00 UTC on the 9th.
    let mut late = Assistant::with(&[
        (KNOB_ON, json!("1")),
        (KNOB_CLOCK, json!("2026-11-08T23:30:00Z")),
        (KNOB_ZONE, json!("Europe/Berlin")),
    ]);
    late.leave("./talky", memory_out("text-1", "t1", unwell()));
    session_ends(&mut late, "./talky", "text-1");
    let notes = live_notes(&late.notes_into("./talky"));
    assert_eq!(notes.len(), 1, "{:?}", late.into);
    let until = chrono::DateTime::parse_from_rfc3339(notes[0]["until"].as_str().unwrap_or(""))
        .unwrap_or_else(|e| panic!("until is RFC 3339 ({e}): {}", notes[0]));
    assert_eq!(
        until.with_timezone(&chrono::Utc).to_rfc3339(),
        "2026-11-09T23:00:00+00:00",
        "{}",
        notes[0]
    );
}
