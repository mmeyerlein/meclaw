//! What the topology asks this cell to do.
//!
//! Four verbs, parsed from the tail `tool_call` turn like `store` and `mcp`
//! parse theirs (`docs/cell-types.md` § store: structured JSON args in the
//! tool_call turn). Everything a task may vary lives here; everything about
//! containment lives in `params` and cannot be reached from a message.

use serde_json::Value as JsonValue;

/// Start one harness run.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskRequest {
    /// Dedup key, supplied by the caller and REQUIRED.
    ///
    /// The plan sketched it as optional with a generated fallback. Making it
    /// mandatory is the stronger reading of the same invariant: a generated id
    /// is unique per call, so a retried message would generate a second id and
    /// run the task twice — precisely what the tombstone register exists to
    /// prevent. Whoever asks for a mutation names it.
    pub task_id: String,
    /// What the harness is asked to do.
    pub prompt: String,
    /// Workspace directory, RELATIVE to `params.workspace_root`.
    pub workspace: String,
    /// Continue a previous vendor session instead of starting fresh.
    pub resume_session_id: Option<String>,
    /// Per-task model override.
    pub model: Option<String>,
    /// Per-task turn budget override.
    pub max_turns: Option<u64>,
}

/// Answer a permission question the harness asked.
#[derive(Debug, Clone, PartialEq)]
pub struct AnswerRequest {
    /// Which task the answer belongs to.
    pub task_id: String,
    /// Which question, as carried in the `question` emission.
    pub request_id: String,
    /// Allow or deny.
    pub allow: bool,
    /// Reason, forwarded to the harness on a denial.
    pub message: String,
}

/// The four things a message can ask of a harness cell.
#[derive(Debug, Clone, PartialEq)]
pub enum HarnessCommand {
    /// Run a task.
    Start(TaskRequest),
    /// Answer a permission question.
    Answer(AnswerRequest),
    /// Stop the running task and reap its process group.
    Cancel {
        /// Which task to stop.
        task_id: String,
        /// Why, recorded on the tombstone.
        reason: String,
    },
    /// Report the last known state of a task.
    Status {
        /// Which task to look up.
        task_id: String,
    },
}

impl HarnessCommand {
    /// Parse the `{name, arguments}` pair from a tool call.
    ///
    /// Rejects are by name and by reason: a malformed task request must never
    /// become a half-configured agent run.
    pub fn parse(name: &str, args: &JsonValue) -> Result<Self, String> {
        match name {
            "start_task" => Ok(Self::Start(TaskRequest {
                task_id: non_empty(args, "task_id")?,
                prompt: non_empty(args, "prompt")?,
                workspace: non_empty(args, "workspace")?,
                resume_session_id: opt_str(args, "resume_session_id"),
                model: opt_str(args, "model"),
                max_turns: args["max_turns"].as_u64(),
            })),
            "answer" => Ok(Self::Answer(AnswerRequest {
                task_id: non_empty(args, "task_id")?,
                request_id: non_empty(args, "request_id")?,
                allow: match args["behavior"].as_str() {
                    Some("allow") => true,
                    Some("deny") => false,
                    _ => return Err("behavior: required (\"allow\" or \"deny\")".to_string()),
                },
                message: opt_str(args, "message").unwrap_or_default(),
            })),
            "cancel" => Ok(Self::Cancel {
                task_id: non_empty(args, "task_id")?,
                reason: opt_str(args, "reason").unwrap_or_else(|| "cancelled".to_string()),
            }),
            "status" => Ok(Self::Status {
                task_id: non_empty(args, "task_id")?,
            }),
            other => Err(format!(
                "unknown tool {other:?} (start_task|answer|cancel|status)"
            )),
        }
    }
}

fn non_empty(args: &JsonValue, key: &str) -> Result<String, String> {
    match args[key].as_str() {
        Some(v) if !v.trim().is_empty() => Ok(v.to_string()),
        _ => Err(format!("{key}: required (non-empty string)")),
    }
}

fn opt_str(args: &JsonValue, key: &str) -> Option<String> {
    args[key].as_str().map(str::to_string)
}
