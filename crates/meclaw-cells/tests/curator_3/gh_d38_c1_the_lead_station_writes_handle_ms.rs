//! GH #1447 (D38 C1, R-38-4 Nr. 6) -- the station `lead` books its time.
//!
//! WHY this file exists: latency is KEY on the phone, and battery P6 compares
//! every station of a call with R039. A new station that books no time cannot
//! be compared, so the lead's station writes `handle_ms` into its mark
//! (`kind` `lead`, one per call). This pin asks only that the number is
//! there and is a number -- never what it is (AGENTS.md, time in tests: a
//! wall-clock value is never a verdict).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

#[test]
fn every_call_books_the_lead_station_with_its_handle_ms() {
    if !shipped() {
        return;
    }
    assert!(
        lead_shipped(),
        "templates/curator/lead/config.json is missing: the lead cell is not shipped (GH #1447)"
    );
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    let call = turn(&mut h, "s1", "t1", "hello", "hi", json!({}));
    let id = call.hop["curator_call"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let marks: Vec<Value> = h
        .rows("SELECT value FROM marks WHERE kind = 'lead'")
        .into_iter()
        .filter_map(|r| {
            r[0].as_str()
                .and_then(|s| meclaw_core::serde_json::from_str(s).ok())
        })
        .collect();
    let mine: Vec<&Value> = marks.iter().filter(|m| m["call"] == json!(id)).collect();
    assert_eq!(mine.len(), 1, "one `lead` mark for call {id}: {marks:?}");
    assert!(
        mine[0]["handle_ms"].is_u64(),
        "the lead's mark carries its handle_ms as a number: {}",
        mine[0]
    );
}
