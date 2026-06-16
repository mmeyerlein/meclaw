//! Phase-10-B: Mailbox-Op-Parser. Konsumiert den `body.Inline(JsonValue)`
//! der Op-Message. `op` ist optional mit Default `"add"` (cell-types.md
//! Z.398). Cron-Expression wird BEREITS AM EINGANG validiert (Korrektur A,
//! identischer `CronParser`-Build wie `run_io`/`params`), damit der Handler
//! in `cell.rs` einen `invalid_cron`-Error-Code aus dem `"cron:"`-Prefix
//! ableiten kann.

use crate::timer::schedule::{ScheduleKind, ScheduleRow};
use chrono::{DateTime, Utc};
use croner::parser::{CronParser, Seconds};
use meclaw_core::{Path, Uuid};
use serde_json::Value as JsonValue;

/// Geparste Mailbox-Op. Strikt typisiert: `Add` traegt die volle Row,
/// `Modify` nur Aenderungs-Felder, `Remove` nur die id.
#[derive(Debug)]
pub enum TimerOp {
    /// INSERT in `cell.db.schedules`. Caller wirft `schedule_id_exists`
    /// bei PK-Konflikt.
    Add(ScheduleRow),
    /// UPDATE existierender Row. `new_cron` XOR `new_at` darf NICHT den
    /// Schedule-Typ wechseln (Handler-Check in `handle`).
    Modify {
        /// PK der zu modifizierenden Row.
        schedule_id: Uuid,
        /// Optional: neuer `schedule_name`.
        new_name: Option<String>,
        /// Optional: neuer Cron-Ausdruck (bereits format-validiert).
        new_cron: Option<String>,
        /// Optional: neuer `at`-Zeitpunkt (UTC).
        new_at: Option<DateTime<Utc>>,
        /// Optional: neuer `emit_to`-Pfad.
        new_emit_to: Option<String>,
    },
    /// Soft-Delete: setzt `status='removed'` (No-Delete-konform).
    Remove {
        /// PK der zu entfernenden Row.
        schedule_id: Uuid,
    },
}

impl TimerOp {
    /// Parse + Validate. Bei Fehler: String mit menschenlesbarer Begruendung
    /// (Caller emittiert daraus die Error-Reply via `OutputSink`). Cron-
    /// Format-Fehler tragen den Prefix `"cron:"` — der Handler mappt das
    /// auf `error_code="invalid_cron"`.
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
