//! Phase-10-B T6: Smoke-Test fuer die `TimerEvent::Fire` + `TimerReconfig::SetActive`
//! Frame-Typen. Compile-only — beweist, dass die Typen oeffentlich sichtbar
//! sind und die Konstruktoren stimmen.

use chrono::Utc;
use meclaw_cells::timer::io::{TimerEvent, TimerReconfig};
use meclaw_core::Uuid;

#[test]
fn frame_types_compile() {
    let _ev = TimerEvent::Fire {
        schedule_id: Uuid::now_v7(),
        scheduled_at: Utc::now(),
    };
    let _rc: TimerReconfig = TimerReconfig::SetActive(vec![]);
}
