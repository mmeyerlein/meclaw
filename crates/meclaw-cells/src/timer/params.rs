//! Phase-10-B: `TimerParams::parse`.
//!
//! Validierung von `params.schedules`-Seed + `query_timeout_ms`.
//! Cron XOR at strikt (cell-types.md Z.425–429).
//! Default `query_timeout_ms` = 5000 (GIVENs).

use crate::timer::schedule::{ScheduleKind, ScheduleRow};
use chrono::{DateTime, Utc};
use croner::parser::{CronParser, Seconds};
use meclaw_core::{Path, Uuid};
use serde_json::Value as JsonValue;

/// Geparste Timer-Params nach Validierung.
#[derive(Debug)]
pub struct TimerParams {
    /// Seed-Rows fuer `params.schedules` (leer wenn nicht im Input).
    pub schedules: Vec<ScheduleRow>,
    /// Operation-Timeout (A) fuer DB-Calls. Default 5000 ms.
    pub query_timeout_ms: u64,
}

/// β: the `timer` runtime-overlay surface is **exactly** `query_timeout_ms`.
///
/// `schedules` are NOT overlay-managed — they change via the `add`/`modify`/
/// `remove` ops (+ `SetActive` to the I/O-task), not via params-updates, and
/// carry live state (`status`/`iteration_n`) in `cell.db` that must never be
/// reset by a params round-trip. So the overlay type is a minimal projection:
/// a params-update touching anything but `query_timeout_ms` (e.g. `schedules`)
/// is an `Unknown` reject. Immutable set is empty (Ruling).
#[derive(Debug, Clone, serde::Serialize)]
pub struct TimerOverlay {
    /// Operation-Timeout (A) for cell.db ops. Mutable, Weg C (sofort-live).
    pub query_timeout_ms: u64,
}

impl crate::params_overlay::OverlayParams for TimerOverlay {
    const KNOWN_KEYS: &'static [&'static str] = &["query_timeout_ms"];
    const IMMUTABLE_KEYS: &'static [&'static str] = &[];
    fn parse(raw: &JsonValue) -> Result<Self, String> {
        let obj = raw.as_object().ok_or("params: must be object")?;
        let query_timeout_ms = obj
            .get("query_timeout_ms")
            .and_then(|x| x.as_u64())
            .unwrap_or(5000);
        Ok(Self { query_timeout_ms })
    }
}

impl TimerParams {
    /// Parse + Validate. Cron XOR at strikt; `emit_to` + `emit_body` Pflicht.
    /// Default-`query_timeout_ms` = 5000.
    pub fn parse(v: &JsonValue) -> Result<Self, String> {
        let obj = v.as_object().ok_or("params: must be object")?;
        let query_timeout_ms = obj
            .get("query_timeout_ms")
            .and_then(|x| x.as_u64())
            .unwrap_or(5000);

        let mut schedules = Vec::new();
        if let Some(arr) = obj.get("schedules") {
            let arr = arr.as_array().ok_or("params.schedules: must be array")?;
            for (i, item) in arr.iter().enumerate() {
                let row =
                    parse_seed_entry(item).map_err(|e| format!("params.schedules[{i}]: {e}"))?;
                schedules.push(row);
            }
        }
        Ok(Self {
            schedules,
            query_timeout_ms,
        })
    }
}

/// Parsen + Validieren eines einzelnen `schedules[]`-Eintrags. Cron-Format
/// wird per `CronParser` validiert (Korrektur A) — ungueltiges Muster fuehrt
/// zu Seed-Reject (Factory rejected `validate_params` → Spawn-Fehler).
fn parse_seed_entry(v: &JsonValue) -> Result<ScheduleRow, String> {
    let obj = v.as_object().ok_or("entry: must be object")?;
    let id_s = obj
        .get("schedule_id")
        .and_then(|x| x.as_str())
        .ok_or("schedule_id: required (UUID v7 string)")?;
    let schedule_id = Uuid::parse_str(id_s).map_err(|e| format!("schedule_id: {e}"))?;
    let schedule_name = obj
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
        (None, Some(a)) => {
            let t = a
                .parse::<DateTime<Utc>>()
                .map_err(|e| format!("at: must be RFC-3339-Z UTC: {e}"))?;
            ScheduleKind::At(t)
        }
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
    Ok(ScheduleRow {
        schedule_id,
        schedule_name,
        kind,
        emit_to: Path::new(emit_to),
        emit_body,
        emit_headers,
        status: "active".into(),
        iteration_n: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_empty_params_yields_no_seed_and_default_timeout() {
        let p = TimerParams::parse(&json!({})).unwrap();
        assert!(p.schedules.is_empty());
        assert_eq!(p.query_timeout_ms, 5000);
    }

    #[test]
    fn parse_seed_with_cron_and_emit_to_round_trips() {
        let p = TimerParams::parse(&json!({
            "schedules": [
                { "schedule_id": "0190a3f2-0000-7000-8000-000000000001",
                  "schedule_name": "daily",
                  "cron": "0 0 9 * * *",
                  "emit_to": "/x",
                  "emit_body": { "messages": [] },
                  "emit_headers": {} }
            ]
        }))
        .unwrap();
        assert_eq!(p.schedules.len(), 1);
    }

    #[test]
    fn parse_rejects_cron_and_at_together() {
        let err = TimerParams::parse(&json!({
            "schedules": [
                { "schedule_id": "0190a3f2-0000-7000-8000-000000000001",
                  "schedule_name": "x", "cron": "0 * * * * *", "at": "2099-01-01T00:00:00Z",
                  "emit_to": "/x", "emit_body": {} }
            ]
        }))
        .unwrap_err();
        assert!(err.contains("cron") && err.contains("at"));
    }

    #[test]
    fn parse_rejects_invalid_cron_expression() {
        let err = TimerParams::parse(&json!({
            "schedules": [
                { "schedule_id": "0190a3f2-0000-7000-8000-000000000001",
                  "schedule_name": "x", "cron": "not a cron",
                  "emit_to": "/x", "emit_body": {} }
            ]
        }))
        .unwrap_err();
        assert!(err.contains("cron"), "expected cron-error, got: {err}");
    }
}
