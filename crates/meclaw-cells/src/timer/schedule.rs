//! Phase-10-B: Schedule-Datenmodell. `ScheduleKind` ist die XOR-Disjunktion
//! cron vs. at (cell-types.md Z.425–429). `ScheduleRow` ist die Handler-Sicht
//! aus `cell.db.schedules`; `ActiveSchedule` ist die I/O-lokale Arbeitskopie.

use chrono::{DateTime, Utc};
use meclaw_core::{Path, Uuid};
use serde_json::{Map, Value as JsonValue};

/// Schedule-Typ: cron (repeating, 6-Feld-Quartz) ODER at (einmalig, UTC).
/// Exklusiv per Spec — `modify` darf den Typ nicht wechseln.
#[derive(Debug, Clone)]
pub enum ScheduleKind {
    /// Repeating Cron-Schedule (6-Feld-Quartz, Sekunden-Granularitaet).
    Cron(String),
    /// Einmaliger `at`-Schedule, UTC-DateTime.
    At(DateTime<Utc>),
}

/// Full row aus `cell.db.schedules`. Handler-Sicht.
#[derive(Debug, Clone)]
pub struct ScheduleRow {
    /// Eindeutiger PK (UUID v7).
    pub schedule_id: Uuid,
    /// Non-unique Label, vom Caller in `add`/`modify`-Op gesetzt.
    pub schedule_name: String,
    /// Cron XOR At; `modify` wechselt den Typ nicht.
    pub kind: ScheduleKind,
    /// Routing-Ziel fuer Fire-Emits.
    pub emit_to: Path,
    /// UBF-Body fuer Fire-Emits.
    pub emit_body: JsonValue,
    /// Optionale Header-Map; Auto-Set-Header ueberschreiben kollidierende Keys.
    pub emit_headers: Map<String, JsonValue>,
    /// Lifecycle-Status: `active`/`completed`/`removed`.
    pub status: String,
    /// Iteration-Counter, nur fuer repeating Schedules relevant.
    pub iteration_n: u64,
}

/// I/O-lokale Arbeitskopie. Der I/O-Sub-Task haelt genau das, was er fuer
/// `find_next_occurrence` + `sleep_until` braucht — keine `emit_*`-Felder.
#[derive(Debug, Clone)]
pub struct ActiveSchedule {
    /// PK der zugrundeliegenden Row in `cell.db.schedules`.
    pub schedule_id: Uuid,
    /// Cron XOR At — Quelle fuer `find_next_occurrence`/sleep_until.
    pub kind: ScheduleKind,
}
