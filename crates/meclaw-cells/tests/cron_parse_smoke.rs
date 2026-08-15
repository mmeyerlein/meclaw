//! Smoke test: proves that `croner` 3.x works with `chrono` UTC + 6-field Quartz
//! + second granularity. A precursor for slice 10-B (`timer`).
//!
//! A fixed `DateTime<Utc>` — NO `Utc::now()`, which would be time-flaky.

use chrono::{TimeZone, Utc};
use croner::parser::{CronParser, Seconds};

#[test]
fn six_field_quartz_finds_next_daily_nine_oclock() {
    let parser = CronParser::builder().seconds(Seconds::Required).build();
    let cron = parser
        .parse("0 0 9 * * *")
        .expect("valid 6-field Quartz pattern");

    let from = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
    let next = cron
        .find_next_occurrence(&from, false)
        .expect("next occurrence exists in the future");
    let expected = Utc.with_ymd_and_hms(2026, 6, 1, 9, 0, 0).unwrap();

    assert_eq!(next, expected);
}

#[test]
fn six_field_quartz_supports_seconds_granularity() {
    let parser = CronParser::builder().seconds(Seconds::Required).build();
    let cron = parser
        .parse("*/5 * * * * *")
        .expect("valid 6-field every-5-seconds pattern");

    let from = Utc.with_ymd_and_hms(2026, 6, 1, 9, 0, 0).unwrap();
    let next = cron
        .find_next_occurrence(&from, false)
        .expect("next occurrence exists in the future");
    let expected = Utc.with_ymd_and_hms(2026, 6, 1, 9, 0, 5).unwrap();

    assert_eq!(next, expected);
}
