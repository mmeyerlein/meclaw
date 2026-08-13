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

/// The three central UBF slots plus the two envelope slots every body may
/// carry. A body made only of these carries no op — see [`no_op_object`].
const CARRIER_SLOTS: [&str; 5] = ["messages", "system", "attachments", "header", "meta"];

/// GH #81: the sentence for "no op arrived", which is a different sentence from
/// "the op is missing its id". The reported case had `schedule_id` present one
/// level down, inside a `tool_call` turn, and got `schedule_id: required` back —
/// which reads as "you forgot the id" when the message is "the op does not live
/// here".
fn no_op_object(obj: &serde_json::Map<String, JsonValue>) -> String {
    let slots: Vec<&str> = obj.keys().map(|k| k.as_str()).collect();
    let named = if slots.is_empty() {
        "nothing".to_string()
    } else {
        slots.join(", ")
    };
    if slots.iter().all(|s| CARRIER_SLOTS.contains(s)) {
        format!(
            "no op object at the body top level: the body carries only carrier slots ({named}). \
             A timer op is either the body's own top-level fields (op, schedule_id, ...) or \
             structured JSON args in a tool_call turn"
        )
    } else {
        format!(
            "no op object at the body top level: found {named}, but neither `op` nor \
             `schedule_id`"
        )
    }
}

impl TimerOp {
    /// Parse + validate. On error: a string with a human-readable reason (the
    /// caller emits the error reply from it via `OutputSink`). Cron format errors
    /// carry the prefix `"cron:"` — the handler maps that to
    /// `error_code="invalid_cron"`.
    ///
    /// GH #81: `v` is the op object, which the caller resolved first — either
    /// the body's top level or the JSON args of a `tool_call` turn. The shape
    /// check below runs before any field check, so a body that never carried an
    /// op says that rather than blaming a field.
    pub fn parse(v: &JsonValue) -> Result<Self, String> {
        let obj = v.as_object().ok_or("op-body: must be object")?;
        if !obj.contains_key("op") && !obj.contains_key("schedule_id") {
            return Err(no_op_object(obj));
        }
        let op = obj.get("op").and_then(|x| x.as_str()).unwrap_or("add");
        let id_s = obj
            .get("schedule_id")
            .and_then(|x| x.as_str())
            .ok_or("schedule_id: required (the op object is there, its id is not)")?;
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

/// GH #81: where an op arrived from, and what the answer has to echo.
#[derive(Debug)]
pub struct OpSource {
    /// The op object itself — the body's top level, or a `tool_call` turn's args.
    pub value: JsonValue,
    /// The inbound `tool_call` id, when the op came in as a turn. `None` on the
    /// legacy raw-body path, and that `None` is what keeps that path silent.
    pub tool_call_id: Option<String>,
}

/// Resolves where the op lives in `msg`.
///
/// A body carrying a `tool_call` turn is driven the way every other tool cell
/// is driven, so the turn wins and its parse errors are reported rather than
/// swallowed — falling back to the top level there would answer a malformed
/// turn with "no op object", which points at the wrong place.
pub fn resolve_op_source(
    msg: &meclaw_core::Message,
    body_val: &JsonValue,
) -> Result<OpSource, String> {
    let carries_turn = body_val
        .get("messages")
        .and_then(|v| v.as_array())
        .is_some_and(|turns| {
            turns
                .iter()
                .any(|t| t.get("type").and_then(|v| v.as_str()) == Some("tool_call"))
        });
    if !carries_turn {
        return Ok(OpSource {
            value: body_val.clone(),
            tool_call_id: None,
        });
    }
    let (args, id) = crate::tool::parse_tool_call_args(msg)?;
    Ok(OpSource {
        value: args,
        tool_call_id: id,
    })
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

    /// GH #81: a body with no op at all says so, and names what it does carry.
    #[test]
    fn a_body_of_carrier_slots_only_reports_the_missing_op_object() {
        let err = TimerOp::parse(&json!({
            "messages": [{"origin": "user", "type": "text", "text": "remind me"}]
        }))
        .unwrap_err();
        assert!(
            err.starts_with("no op object at the body top level"),
            "expected the shape, got: {err}"
        );
        assert!(err.contains("messages"), "got: {err}");
        assert!(err.contains("tool_call turn"), "got: {err}");
    }

    /// GH #81: a body carrying cell-specific slots that are not an op says the
    /// other sentence — nothing was recognised, and here is what was there.
    #[test]
    fn a_body_of_foreign_slots_lists_them() {
        let err = TimerOp::parse(&json!({"messages": [], "reminder": "x"})).unwrap_err();
        assert!(
            err.starts_with("no op object at the body top level"),
            "{err}"
        );
        assert!(err.contains("reminder"), "got: {err}");
    }

    /// GH #81: the op object is there, only its id is not — a different repair.
    #[test]
    fn an_op_object_without_an_id_blames_the_field() {
        let err = TimerOp::parse(&json!({"op": "remove"})).unwrap_err();
        assert!(err.starts_with("schedule_id:"), "got: {err}");
    }

    /// GH #81: a `tool_call` turn wins over the top level, and its own parse
    /// errors are reported instead of being answered with "no op object" —
    /// which would point at the wrong level of the message.
    #[test]
    fn resolve_op_source_prefers_the_tool_call_turn_and_keeps_its_errors() {
        use meclaw_core::{Body, MessageBuilder, Path as CorePath};

        let good = json!({
            "messages": [{
                "id": "call-1", "origin": "assistant", "type": "tool_call",
                "text": "{\"op\":\"remove\",\"schedule_id\":\"0190a3f2-0000-7000-8000-000000000001\"}"
            }]
        });
        let msg = MessageBuilder::new(CorePath::new("/t"))
            .body(Body::Inline(good.clone()))
            .build();
        let src = resolve_op_source(&msg, &good).expect("resolve");
        assert_eq!(src.tool_call_id.as_deref(), Some("call-1"));
        assert!(matches!(
            TimerOp::parse(&src.value).unwrap(),
            TimerOp::Remove { .. }
        ));

        let broken = json!({
            "messages": [{
                "id": "call-2", "origin": "assistant", "type": "tool_call",
                "text": "not json at all"
            }]
        });
        let msg = MessageBuilder::new(CorePath::new("/t"))
            .body(Body::Inline(broken.clone()))
            .build();
        let err = resolve_op_source(&msg, &broken).unwrap_err();
        assert!(err.contains("not valid JSON"), "got: {err}");
    }

    /// And a body without any turn keeps going through the top level.
    #[test]
    fn resolve_op_source_falls_back_to_the_body_top_level() {
        use meclaw_core::{Body, MessageBuilder, Path as CorePath};

        let body = json!({
            "messages": [],
            "op": "trigger",
            "schedule_id": "0190a3f2-0000-7000-8000-000000000001"
        });
        let msg = MessageBuilder::new(CorePath::new("/t"))
            .body(Body::Inline(body.clone()))
            .build();
        let src = resolve_op_source(&msg, &body).expect("resolve");
        assert!(src.tool_call_id.is_none());
        assert!(matches!(
            TimerOp::parse(&src.value).unwrap(),
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
