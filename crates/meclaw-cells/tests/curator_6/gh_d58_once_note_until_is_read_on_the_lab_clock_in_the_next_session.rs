//! GH #1494 (D58 close, followup OR-d58-2.followup.20) -- the turn of the next
//! session reads the `until` of a `once` note on the clock of the run.
//!
//! WHY this file exists: an open concern reaches the next conversation as a
//! note of scope `once` with an `until` (OR-d58-2.followup.4): past it the
//! curator shows it nowhere. The followup pins judged the next session only
//! through its start window, because a turn's window measured `until`
//! against the wall clock alone. The policy reads its time through `now()`,
//! the one clock the corpus prelude of the tests sets (`Hive::set_clock`,
//! `support/curator_target.rs`), so the turn is pinned here on that clock:
//! before `until` the first turn of the next session shows the note, after
//! it the same turn shows nothing and the note stays unused.
//!
//! Runs the shipped curator hive behind the corpus clock; no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;
// harness_fold: #[path = "support/note_once.rs"]
#[allow(unused_imports)]
use crate::note_once; // harness_fold: mod note_once;

use curator_hive::*;
use curator_target::{PRELUDE, T0_US};
use meclaw_core::serde_json::json;
use note_once::*;

const TEXT: &str = "Ask how the person slept -- they felt unwell yesterday.";
const MINUTE_US: i64 = 60_000_000;

fn rfc3339(us: i64) -> String {
    chrono::DateTime::from_timestamp_micros(us)
        .expect("an instant")
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// A curator with `note_once` on whose session `s1` left a `once` note
/// valid for ten minutes from [`T0_US`]; the first turn of session `s2` runs
/// at `turn_at_us` on the corpus clock.
fn next_session_turn(turn_at_us: i64) -> (Hive, Msg) {
    let over: Vec<(&str, &str, meclaw_core::serde_json::Value)> =
        KNOB_CELLS.iter().map(|c| (*c, KNOB, json!("1"))).collect();
    let mut h = Hive::with_prelude(&over, PRELUDE).reporting(notes_profile());
    h.set_clock(T0_US);
    let mut n = once("n1", TEXT, Some("r-1"));
    n["until"] = json!(rfc3339(T0_US + 10 * MINUTE_US));
    post(&mut h, json!([n]));
    assert_eq!(used(&h, "n1"), "", "stored open: {:?}", h.stderr);
    h.set_clock(turn_at_us);
    let c = h.curate(
        "s2",
        "u1",
        0,
        json!([user("Good morning.")]),
        mode("Be brief."),
    );
    (h, c)
}

#[test]
fn before_until_the_turn_of_the_next_session_shows_the_note() {
    if !shipped() {
        return;
    }
    let (_, c) = next_session_turn(T0_US + 5 * MINUTE_US);
    assert!(shows(&c, TEXT), "{:?}", texts(&c));
}

#[test]
fn past_until_the_same_turn_shows_nothing() {
    if !shipped() {
        return;
    }
    let (h, c) = next_session_turn(T0_US + 20 * MINUTE_US);
    assert!(!shows(&c, TEXT), "{:?}", texts(&c));
    assert_eq!(
        used(&h, "n1"),
        "",
        "past its `until` the note is not used up"
    );
    assert!(receipts(&h).is_empty(), "no receipt: {:?}", h.out);
}
