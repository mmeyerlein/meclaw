//! Phase-10-B T6: smoke test for the `TimerEvent::Fire` + `TimerReconfig::SetActive`
//! frame types. Compile-only — proves that the types are publicly visible and the
//! constructors are right.

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
