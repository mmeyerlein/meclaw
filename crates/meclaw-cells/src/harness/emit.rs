//! The five emission shapes of the `harness` cell type.
//!
//! Two lanes, for a structural reason. `accepted` and `error` answer a message
//! and therefore travel on the `OutputSink`, inside the requester's trace.
//! `progress`, `question` and `result` happen while no message is being
//! handled, so they can only go out through the `OriginSink` — which by
//! contract starts a fresh trace with no parent (`docs/meclaw-overview.md`
//! § source cells). That is why `accepted` exists at all: it is the one place
//! the topology learns the `task_id` that correlates the origin lane.
//!
//! Body discipline (UBF): the turn object allows no extra properties, so
//! everything structural lives in the `header` slot and the turn carries only
//! human-readable text.

use crate::harness::db::TaskRow;
use meclaw_core::serde_json::{Map, Value as JsonValue, json};
use meclaw_core::{CellOutput, Message, OriginSink, OutputSink, Path};

/// Everything known about a finished task at emission time.
#[derive(Debug, Clone)]
pub struct TaskOutcome {
    /// The task this is about.
    pub task_id: String,
    /// Vendor session id, if the harness got far enough to report one.
    pub session_id: Option<String>,
    /// `ok` | `error` | `crashed` | `cancelled` | `unknown`.
    pub status: &'static str,
    /// The workspace the task ran in — OUR record, not the harness's claim.
    pub workspace: String,
    /// Wall-clock duration of the run.
    pub duration_ms: u64,
    /// Turns consumed, as reported by the harness.
    pub num_turns: Option<u64>,
    /// Money spent, as reported by the harness.
    pub cost_usd: Option<f64>,
    /// The model the harness reported using.
    pub model: Option<String>,
    /// Set for every non-`ok` status.
    pub error_code: Option<&'static str>,
    /// The harness's own summary. Prose, never a fact about the repository.
    pub text: String,
}

impl TaskOutcome {
    /// Build the outcome of a task that a restart interrupted.
    ///
    /// Everything except the workspace is unknown by construction — that is
    /// the whole point of the emission: the topology is told where to look,
    /// not what happened.
    pub fn unknown_from_row(row: &TaskRow) -> Self {
        Self {
            task_id: row.task_id.clone(),
            session_id: row.session_id.clone(),
            status: "unknown",
            workspace: row.workspace.clone(),
            duration_ms: 0,
            num_turns: None,
            cost_usd: None,
            model: None,
            error_code: Some("unknown_outcome"),
            text: "interrupted by a cell restart; the outcome is unknown — inspect the workspace"
                .to_string(),
        }
    }
}

/// Acknowledge a started task, in the requester's trace.
pub async fn accepted(sink: &OutputSink, msg: &Message, task_id: &str, call_id: &str) {
    let text = json!({ "task_id": task_id }).to_string();
    accepted_with_text(sink, msg, task_id, call_id, &text).await;
}

/// The same acknowledgement, with a payload of its own — used by the status
/// lookup, which answers with the task row rather than just its id.
pub async fn accepted_with_text(
    sink: &OutputSink,
    msg: &Message,
    task_id: &str,
    call_id: &str,
    text: &str,
) {
    let target = msg.reply_to.clone().unwrap_or_else(|| msg.target.clone());
    let body = json!({
        "header": {"harness_event": "accepted", "task_id": task_id},
        "messages": [{
            "origin": "tool", "type": "tool_result", "text": text, "id": call_id
        }]
    });
    let _ = sink
        .push(CellOutput {
            target,
            content: body,
        })
        .await;
}

/// Report a message-driven failure, in the requester's trace.
pub async fn error(
    sink: &OutputSink,
    msg: &Message,
    call_id: &str,
    error_code: &str,
    detail: &str,
) {
    let target = msg.reply_to.clone().unwrap_or_else(|| msg.target.clone());
    let body = json!({
        "header": {"harness_event": "error", "error_code": error_code},
        "messages": [{
            "origin": "tool", "type": "tool_result", "text": detail, "id": call_id
        }]
    });
    let _ = sink
        .push(CellOutput {
            target,
            content: body,
        })
        .await;
}

/// Report that the harness is working.
#[allow(clippy::too_many_arguments)]
pub async fn progress(
    sink: &OriginSink,
    target: Path,
    task_id: &str,
    session_id: Option<&str>,
    phase: &str,
    tool_name: Option<&str>,
    text: Option<&str>,
) {
    let mut header = Map::new();
    header.insert("harness_event".into(), json!("progress"));
    header.insert("task_id".into(), json!(task_id));
    header.insert("phase".into(), json!(phase));
    insert_opt(&mut header, "session_id", session_id.map(JsonValue::from));
    insert_opt(&mut header, "tool_name", tool_name.map(JsonValue::from));

    let body = json!({
        "header": header,
        "messages": [{"origin": "assistant", "type": "text", "text": text.unwrap_or("")}]
    });
    let _ = sink
        .emit(CellOutput {
            target,
            content: body,
        })
        .await;
}

/// Ask the topology for a permission decision.
pub async fn question(
    sink: &OriginSink,
    target: Path,
    task_id: &str,
    session_id: Option<&str>,
    request_id: &str,
    tool_name: &str,
    input: &JsonValue,
) {
    let mut header = Map::new();
    header.insert("harness_event".into(), json!("question"));
    header.insert("task_id".into(), json!(task_id));
    header.insert("request_id".into(), json!(request_id));
    header.insert("tool_name".into(), json!(tool_name));
    insert_opt(&mut header, "session_id", session_id.map(JsonValue::from));

    let body = json!({
        "header": header,
        "messages": [{"origin": "assistant", "type": "text", "text": input.to_string()}]
    });
    let _ = sink
        .emit(CellOutput {
            target,
            content: body,
        })
        .await;
}

/// Report how a task ended.
///
/// The header carries observations only: the workspace we assigned, the status
/// we decided, and the numbers the harness reported about ITSELF (turns, cost,
/// model, session). It deliberately carries no `branch`, `commit` or
/// `files_changed` — those would be the harness's claims dressed as structured
/// data. Whoever needs them runs `git` in a follow-up step.
pub async fn result(sink: &OriginSink, target: Path, outcome: &TaskOutcome) {
    let mut header = Map::new();
    header.insert("harness_event".into(), json!("result"));
    header.insert("task_id".into(), json!(outcome.task_id));
    header.insert("status".into(), json!(outcome.status));
    header.insert("workspace".into(), json!(outcome.workspace));
    header.insert("duration_ms".into(), json!(outcome.duration_ms));
    insert_opt(
        &mut header,
        "session_id",
        outcome.session_id.as_deref().map(JsonValue::from),
    );
    insert_opt(
        &mut header,
        "num_turns",
        outcome.num_turns.map(JsonValue::from),
    );
    insert_opt(
        &mut header,
        "cost_usd",
        outcome.cost_usd.map(JsonValue::from),
    );
    insert_opt(
        &mut header,
        "model",
        outcome.model.as_deref().map(JsonValue::from),
    );
    insert_opt(
        &mut header,
        "error_code",
        outcome.error_code.map(JsonValue::from),
    );

    let body = json!({
        "header": header,
        "messages": [{"origin": "assistant", "type": "text", "text": outcome.text}]
    });
    let _ = sink
        .emit(CellOutput {
            target,
            content: body,
        })
        .await;
}

/// Add a header field only when there is something to say. An absent number
/// must stay absent: a zero would read as a measurement.
fn insert_opt(header: &mut Map<String, JsonValue>, key: &str, value: Option<JsonValue>) {
    if let Some(v) = value {
        header.insert(key.to_string(), v);
    }
}
