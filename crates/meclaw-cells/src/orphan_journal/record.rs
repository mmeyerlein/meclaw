//! The journal record and the fold that turns a file of records into the list
//! of children that might still be alive.
//!
//! One line of JSON per state change, never an in-place edit: the file is
//! append-only both because that is the only shape an fsync makes crash-safe
//! and because `{root}` is under the No-Delete policy (see the module docs of
//! [`super`] and the track ruling in the receipt).

use serde::{Deserialize, Serialize};

/// What a record says about the child it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordState {
    /// The child was spawned and, as far as this daemon knows, is running.
    Spawned,
    /// The child was waited for by its own daemon — the ordinary path.
    Exited,
    /// A boot found it alive, verified its identity and killed it.
    Reaped,
    /// A boot decided NOT to touch it (pid reuse, unknown identity, or a
    /// living owner). The `note` says which.
    Skipped,
}

/// One line of the journal.
///
/// The identity fields are `Option` because `/proc` can refuse to answer: a
/// record without `start_id` is unverifiable and is therefore never reaped.
/// That is the conservative direction — an unkillable leak is a nuisance, a
/// wrongly killed stranger is a bug.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalRecord {
    /// OS pid of the tool child.
    pub pid: u32,
    /// `starttime` of the child, the pid-reuse discriminator. `None` = unknown.
    #[serde(default)]
    pub start_id: Option<u64>,
    /// Executable name of the child, the second identity signal.
    #[serde(default)]
    pub comm: Option<String>,
    /// Process group to sweep, when the child leads one of its own.
    #[serde(default)]
    pub pgid: Option<u32>,
    /// Which cell spawned it — diagnostics only, never a kill criterion.
    pub cell_path: String,
    /// Unix seconds at spawn time.
    pub spawned_at: i64,
    /// The state this record reports.
    pub state: RecordState,
    /// pid of the daemon that owns the child.
    pub daemon_pid: u32,
    /// `starttime` of that daemon — so a boot can tell "my predecessor died"
    /// from "a second daemon is running right now".
    #[serde(default)]
    pub daemon_start_id: Option<u64>,
    /// Free-text reason, set on `Skipped`/`Reaped` records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl JournalRecord {
    /// The identity key: pid PLUS start time. Two different processes can share
    /// a pid over time, never the pair.
    pub fn key(&self) -> (u32, Option<u64>) {
        (self.pid, self.start_id)
    }
}

/// Fold a journal into the children whose last word was "spawned".
///
/// Later records win, so an `exited`/`reaped`/`skipped` line retires the
/// `spawned` line above it without anything being rewritten or deleted. Order
/// of the result follows first appearance, which keeps boot logs stable.
pub fn survivors(records: &[JournalRecord]) -> Vec<JournalRecord> {
    let mut order: Vec<(u32, Option<u64>)> = Vec::new();
    let mut latest: std::collections::HashMap<(u32, Option<u64>), JournalRecord> =
        std::collections::HashMap::new();
    for rec in records {
        let key = rec.key();
        if latest.insert(key, rec.clone()).is_none() {
            order.push(key);
        }
    }
    order
        .into_iter()
        .filter_map(|k| latest.remove(&k))
        .filter(|r| r.state == RecordState::Spawned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(pid: u32, start_id: u64, state: RecordState) -> JournalRecord {
        JournalRecord {
            pid,
            start_id: Some(start_id),
            comm: Some("sleep".into()),
            pgid: None,
            cell_path: "/tools/bash".into(),
            spawned_at: 1_700_000_000,
            state,
            daemon_pid: 1,
            daemon_start_id: Some(7),
            note: None,
        }
    }

    #[test]
    fn a_record_round_trips_through_one_json_line() {
        let r = rec(4711, 99, RecordState::Spawned);
        let line = serde_json::to_string(&r).expect("serialises");
        assert!(!line.contains('\n'), "a record must stay one line: {line}");
        assert!(line.contains("\"state\":\"spawned\""), "line was {line}");
        let back: JournalRecord = serde_json::from_str(&line).expect("round trip");
        assert_eq!(back, r);
    }

    #[test]
    fn an_exited_record_retires_the_spawned_one_above_it() {
        let recs = vec![
            rec(10, 100, RecordState::Spawned),
            rec(11, 101, RecordState::Spawned),
            rec(10, 100, RecordState::Exited),
        ];
        let alive = survivors(&recs);
        assert_eq!(alive.len(), 1, "only pid 11 is unaccounted for: {alive:?}");
        assert_eq!(alive[0].pid, 11);
    }

    #[test]
    fn the_same_pid_with_a_new_start_id_is_a_different_child() {
        let recs = vec![
            rec(10, 100, RecordState::Spawned),
            rec(10, 100, RecordState::Exited),
            rec(10, 200, RecordState::Spawned),
        ];
        let alive = survivors(&recs);
        assert_eq!(alive.len(), 1, "the second incarnation survives: {alive:?}");
        assert_eq!(alive[0].start_id, Some(200));
    }

    #[test]
    fn a_reaped_or_skipped_record_also_retires_a_child() {
        for state in [RecordState::Reaped, RecordState::Skipped] {
            let recs = vec![rec(10, 100, RecordState::Spawned), rec(10, 100, state)];
            assert!(
                survivors(&recs).is_empty(),
                "{state:?} must retire the entry"
            );
        }
    }
}
