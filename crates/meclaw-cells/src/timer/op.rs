//! Phase-10-B: the mailbox op parser. Consumes the op message's
//! `body.Inline(JsonValue)`. `op` is optional with the default `"add"`
//! (cell-types.md l.398). The cron expression is validated ALREADY AT THE
//! ENTRANCE (correction A, the same `CronParser` build as `run_io`/`params`), so
//! that the handler in `cell.rs` can derive an `invalid_cron` error code from the
//! `"cron:"` prefix.

use crate::timer::schedule::{ScheduleKind, ScheduleRow};
use chrono::{DateTime, Utc};
use croner::parser::{CronParser, Seconds};
use meclaw_core::{Path, Uuid};
use serde_json::Value as JsonValue;

/// Parsed mailbox op. Strictly typed: `Add` carries the full row, `Modify` only
/// the changed fields, `Remove` only the id.
#[derive(Debug)]
pub enum TimerOp {
    /// INSERT into `cell.db.schedules`. The caller raises `schedule_id_exists`
    /// on a PK conflict.
    Add(ScheduleRow),
    /// UPDATE an existing row. `new_cron` XOR `new_at` must NOT switch the
    /// schedule type (handler check in `handle`).
    Modify {
        /// PK of the row to modify.
        schedule_id: Uuid,
        /// Optional: new `schedule_name`.
        new_name: Option<String>,
        /// Optional: new cron expression (already format-validated).
        new_cron: Option<String>,
        /// Optional: new `at` point in time (UTC).
        new_at: Option<DateTime<Utc>>,
        /// Optional: new `emit_to` path.
        new_emit_to: Option<String>,
    },
    /// Soft delete: sets `status='removed'` (no-delete conformant).
    Remove {
        /// PK of the row to remove.
        schedule_id: Uuid,
    },
    /// Fire an existing schedule ONCE, now, without touching its plan (GH #17).
    /// The handler checks that the row exists and is active, then hands the
    /// firing to the I/O task, which pushes the same `Fire` frame the sleep arm
    /// pushes. The schedule keeps its cron; a one-shot keeps its `at`.
    Trigger {
        /// PK of the row to fire.
        schedule_id: Uuid,
    },
}

impl TimerOp {
    /// Parse + validate. On error: a string with a human-readable reason (the
    /// caller emits the error reply from it via `OutputSink`). Cron format errors
    /// carry the prefix `"cron:"` — the handler maps that to
    /// `error_code="invalid_cron"`.
    pub fn parse(v: &JsonValue) -> Result<Self, String> {
        let obj = v.as_object().ok_or("op-body: must be object")?;
        let op = obj.get("op").and_then(|x| x.as_str()).unwrap_or("add");
        let id_s = obj
            .get("schedule_id")
            .and_then(|x| x.as_str())
            .ok_or("schedule_id: required")?;
        let schedule_id = Uuid::parse_str(id_s).map_err(|e| format!("schedule_id: {e}"))?;
        match op {
            "remove" => Ok(TimerOp::Remove { schedule_id }),
            // `trigger` carries nothing but the id: it fires the schedule as it
            // stands rather than describing a new one (GH #17). A trigger that
            // took fields would be a modify with a different name.
            "trigger" => Ok(TimerOp::Trigger { schedule_id }),
            "modify" => parse_modify(obj, schedule_id),
            "add" => parse_add(obj, schedule_id),
            other => Err(format!("op: unknown value {other:?}")),
        }
    }
}

fn parse_modify(
    obj: &serde_json::Map<String, JsonValue>,
    schedule_id: Uuid,
) -> Result<TimerOp, String> {
    let new_cron = obj.get("cron").and_then(|x| x.as_str()).map(String::from);
    if let Some(ref c) = new_cron {
        let parser = CronParser::builder().seconds(Seconds::Required).build();
        parser
            .parse(c)
            .map_err(|e| format!("cron: invalid expression: {e}"))?;
    }
    let new_at = match obj.get("at").and_then(|x| x.as_str()) {
        Some(s) => Some(s.parse().map_err(|e| format!("at: {e}"))?),
        None => None,
    };
    Ok(TimerOp::Modify {
        schedule_id,
        new_name: obj
            .get("schedule_name")
            .and_then(|x| x.as_str())
            .map(String::from),
        new_cron,
        new_at,
        new_emit_to: obj
            .get("emit_to")
            .and_then(|x| x.as_str())
            .map(String::from),
    })
}

fn parse_add(
    obj: &serde_json::Map<String, JsonValue>,
    schedule_id: Uuid,
) -> Result<TimerOp, String> {
    let name = obj
        .get("schedule_name")
        .and_then(|x| x.as_str())
        .ok_or("schedule_name: required")?
        .to_string();
    let cron = obj.get("cron").and_then(|x| x.as_str());
    let at = obj.get("at").and_then(|x| x.as_str());
    let kind = match (cron, at) {
        (Some(c), None) => {
            let parser = CronParser::builder().seconds(Seconds::Required).build();
            parser
                .parse(c)
                .map_err(|e| format!("cron: invalid expression: {e}"))?;
            ScheduleKind::Cron(c.to_string())
        }
        (None, Some(a)) => ScheduleKind::At(a.parse().map_err(|e| format!("at: {e}"))?),
        (Some(_), Some(_)) => return Err("cron XOR at — exclusive".into()),
        (None, None) => return Err("one of cron|at required".into()),
    };
    let emit_to = obj
        .get("emit_to")
        .and_then(|x| x.as_str())
        .ok_or("emit_to: required")?;
    let emit_body = obj.get("emit_body").cloned().ok_or("emit_body: required")?;
    let emit_headers = obj
        .get("emit_headers")
        .and_then(|x| x.as_object())
        .cloned()
        .unwrap_or_default();
    Ok(TimerOp::Add(ScheduleRow {
        schedule_id,
        schedule_name: name,
        kind,
        emit_to: Path::new(emit_to),
        emit_body,
        emit_headers,
        status: "active".into(),
        iteration_n: 0,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn op_defaults_to_add_when_omitted() {
        let parsed = TimerOp::parse(&json!({
            "schedule_id": "0190a3f2-0000-7000-8000-000000000001",
            "schedule_name": "x",
            "cron": "*/1 * * * * *",
            "emit_to": "/x",
            "emit_body": {}
        }))
        .unwrap();
        assert!(matches!(parsed, TimerOp::Add(_)));
    }

    #[test]
    fn op_modify_carries_optional_fields() {
        let parsed = TimerOp::parse(&json!({
            "op": "modify",
            "schedule_id": "0190a3f2-0000-7000-8000-000000000001",
            "cron": "*/5 * * * * *"
        }))
        .unwrap();
        assert!(matches!(
            parsed,
            TimerOp::Modify { ref new_cron, .. } if new_cron.is_some()
        ));
    }

    #[test]
    fn op_remove_requires_only_id() {
        let parsed = TimerOp::parse(&json!({
            "op": "remove",
            "schedule_id": "0190a3f2-0000-7000-8000-000000000001"
        }))
        .unwrap();
        assert!(matches!(parsed, TimerOp::Remove { .. }));
    }

    #[test]
    fn op_trigger_requires_only_id() {
        let parsed = TimerOp::parse(&json!({
            "op": "trigger",
            "schedule_id": "0190a3f2-0000-7000-8000-000000000001"
        }))
        .unwrap();
        assert!(matches!(parsed, TimerOp::Trigger { .. }));
    }

    /// GH #17: an op body that reaches the cell over the HTTP ingress carries a
    /// central UBF slot (`messages: []`, the honest statement that a control
    /// message has no turns) next to the op's own top-level slots. The parser
    /// must read the op past that slot, otherwise the documented envelope shape
    /// and the mailbox disagree.
    #[test]
    fn op_parses_next_to_a_central_ubf_slot() {
        let body = json!({
            "messages": [],
            "op": "trigger",
            "schedule_id": "0190a3f2-0000-7000-8000-000000000001"
        });
        meclaw_core::validate_ubf_body(&body).expect("the op envelope must be valid UBF");
        assert!(matches!(
            TimerOp::parse(&body).unwrap(),
            TimerOp::Trigger { .. }
        ));
    }

    #[test]
    fn op_add_rejects_invalid_cron_expression() {
        let err = TimerOp::parse(&json!({
            "op": "add",
            "schedule_id": "0190a3f2-0000-7000-8000-000000000001",
            "schedule_name": "x",
            "cron": "not a cron",
            "emit_to": "/x",
            "emit_body": {}
        }))
        .unwrap_err();
        assert!(err.starts_with("cron:"), "expected cron-prefix, got: {err}");
    }

    #[test]
    fn op_modify_with_new_cron_validates_format() {
        let err = TimerOp::parse(&json!({
            "op": "modify",
            "schedule_id": "0190a3f2-0000-7000-8000-000000000001",
            "cron": "not a cron"
        }))
        .unwrap_err();
        assert!(err.starts_with("cron:"), "expected cron-prefix, got: {err}");
    }
}
