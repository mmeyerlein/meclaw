//! Phase-10-B: the schedule data model. `ScheduleKind` is the XOR disjunction
//! cron vs. at (cell-types.md l.425-429). `ScheduleRow` is the handler's view of
//! `cell.db.schedules`; `ActiveSchedule` is the I/O-local working copy.

use chrono::{DateTime, Utc};
use meclaw_core::{Path, Uuid};
use serde_json::{Map, Value as JsonValue};

/// Schedule type: cron (repeating, 6-field Quartz) OR at (one-shot, UTC).
/// Exclusive per spec — `modify` must not switch the type.
#[derive(Debug, Clone)]
pub enum ScheduleKind {
    /// Repeating cron schedule (6-field Quartz, second granularity).
    Cron(String),
    /// One-shot `at` schedule, UTC datetime.
    At(DateTime<Utc>),
}

/// Full row from `cell.db.schedules`. The handler's view.
#[derive(Debug, Clone)]
pub struct ScheduleRow {
    /// Unique PK (UUID v7).
    pub schedule_id: Uuid,
    /// Non-unique label, set by the caller in the `add`/`modify` op.
    pub schedule_name: String,
    /// Cron XOR at; `modify` does not switch the type.
    pub kind: ScheduleKind,
    /// Routing target for fire emissions.
    pub emit_to: Path,
    /// UBF body for fire emissions.
    pub emit_body: JsonValue,
    /// Optional header map; auto-set headers override colliding keys.
    pub emit_headers: Map<String, JsonValue>,
    /// Lifecycle status: `active`/`completed`/`removed`.
    pub status: String,
    /// Iteration counter, relevant only for repeating schedules.
    pub iteration_n: u64,
}

/// I/O-local working copy. The I/O sub-task holds exactly what it needs for
/// `find_next_occurrence` + `sleep_until` — no `emit_*` fields.
#[derive(Debug, Clone)]
pub struct ActiveSchedule {
    /// PK of the underlying row in `cell.db.schedules`.
    pub schedule_id: Uuid,
    /// Cron XOR at — the source for `find_next_occurrence`/sleep_until.
    pub kind: ScheduleKind,
}
