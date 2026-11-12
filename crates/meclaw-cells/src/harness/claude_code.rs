//! The Claude Code adapter: dialect translation, nothing else.
//!
//! Two directions. Outbound, `build_argv` turns this cell's configuration into
//! a command line. Inbound, `classify` turns one `stream-json` frame into one
//! of this package's own signals. Neither spawns, waits, persists or emits —
//! that is the cell's job, and keeping it out of here is what makes a second
//! adapter cheap.
//!
//! Vocabulary verified against the installed CLI (2.1.219) rather than taken
//! from documentation: `--output-format stream-json`, the `system`/`assistant`/
//! `user`/`result` event types, and the `control_request`/`control_response`
//! pair with subtype `can_use_tool` — each one read off `claude --help` and
//! the binary's own strings, not off a document.

use crate::harness::params::HarnessParams;
use crate::harness::task::TaskRequest;
use serde_json::{Value as JsonValue, json};

/// What one frame from the harness means to this cell.
#[derive(Debug, Clone, PartialEq)]
pub enum HarnessSignal {
    /// The run is up. Carries the identity the audit trail needs.
    Started {
        /// Vendor session id — the resume and transcript key.
        session_id: String,
        /// The model the harness actually chose, as reported by itself. Read
        /// back rather than assumed: a config claim is not evidence.
        model: Option<String>,
    },
    /// The harness is working. Not persisted, only forwarded.
    Progress {
        /// `text` or `tool`.
        phase: &'static str,
        /// Which tool, when the phase is `tool`.
        tool_name: Option<String>,
        /// A short human-readable excerpt.
        text: Option<String>,
    },
    /// The harness asks permission for a tool call.
    Question {
        /// Correlation id the answer must carry back.
        request_id: String,
        /// The tool it wants to use.
        tool_name: String,
        /// The arguments it wants to use it with.
        input: JsonValue,
    },
    /// The run ended and reported its own outcome.
    Finished {
        /// Whether the harness considers the run successful.
        ok: bool,
        /// Turns consumed, if reported.
        num_turns: Option<u64>,
        /// Money spent, if reported.
        cost_usd: Option<f64>,
        /// The harness's own summary. Prose, never trusted as a fact about the
        /// repository — verification is a follow-up topology step.
        text: Option<String>,
    },
    /// A frame this adapter does not model. Ignoring beats guessing: the
    /// vendor grows event types faster than an adapter can grow arms.
    Ignored,
}

/// What one child is started with: the effective values of the task that
/// spawns it, resolved once against the cell's params.
///
/// A child outlives its first task in the persistent mode, so the argv can no
/// longer be built from a task: it is built from these start values, and two
/// tasks with equal start values can share one child (D57 V1, OR-57-16/26).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChildStart {
    /// Effective model: the task's override, else the cell's.
    pub model: Option<String>,
    /// Effective turn budget: the task's override, else the cell's.
    pub max_turns: Option<u64>,
    /// Money budget per child process (the CLI counts per process).
    pub max_budget_usd: Option<f64>,
    /// The vendor session to continue, if any.
    pub resume_session_id: Option<String>,
}

impl ChildStart {
    /// Resolve a task's start values. Task settings win over cell settings,
    /// and anything nobody configured stays absent.
    pub fn for_task(params: &HarnessParams, task: &TaskRequest) -> Self {
        Self {
            model: task.model.clone().or_else(|| params.model.clone()),
            max_turns: task.max_turns.or(params.max_turns),
            max_budget_usd: params.max_budget_usd,
            resume_session_id: task.resume_session_id.clone(),
        }
    }
}

/// Build the command line for one child.
///
/// The prompt is NOT on it: every task travels as a `user` frame on stdin
/// ([`build_user_turn`]), so one child can take many tasks. The fixed head
/// asks for the streaming dialect in both directions, routes permission
/// questions to stdin/stdout (without `--permission-prompt-tool stdio` the
/// CLI denies on its own and the cell never sees the question, D57 S0 2), and
/// loads no MCP server the operator did not name (without
/// `--strict-mcp-config` account connectors loaded despite an empty tool
/// list, D57 S0 10), and reads only the workspace's project settings
/// (`--setting-sources project`, #1489). Nothing else is invented: no model,
/// no budget.
pub fn build_argv(params: &HarnessParams, start: &ChildStart) -> Vec<String> {
    let mut argv: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        // The streaming format is only fully populated in verbose mode.
        "--verbose",
        "--permission-prompt-tool",
        "stdio",
        "--strict-mcp-config",
        // Only the workspace's project settings: without it the child loaded
        // the account's user settings and instruction file (≈ 7.8k tokens of
        // foreign context per run, another reply language; #1489).
        "--setting-sources",
        "project",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    if let Some(model) = &start.model {
        argv.push("--model".to_string());
        argv.push(model.clone());
    }
    if let Some(mode) = &params.permission_mode {
        argv.push("--permission-mode".to_string());
        argv.push(mode.clone());
    }
    if let Some(turns) = start.max_turns {
        argv.push("--max-turns".to_string());
        argv.push(turns.to_string());
    }
    if let Some(budget) = start.max_budget_usd {
        argv.push("--max-budget-usd".to_string());
        argv.push(budget.to_string());
    }
    if !params.allowed_tools.is_empty() {
        argv.push("--allowedTools".to_string());
        argv.push(params.allowed_tools.join(","));
    }
    if let Some(session) = &start.resume_session_id {
        argv.push("--resume".to_string());
        argv.push(session.clone());
    }
    argv.extend(params.extra_args.iter().cloned());
    argv
}

/// One task as the `user` frame of the `stream-json` input dialect.
pub fn build_user_turn(prompt: &str) -> JsonValue {
    json!({"type": "user", "message": {"role": "user", "content": prompt}})
}

/// The context size of the model call behind one `assistant` frame.
///
/// `message.usage` of an assistant frame is per call, so input plus both
/// cache counters is the context that call carried; `result.usage` is a sum
/// over the turn's calls and says nothing about the context (D57 S0 11). A
/// frame of a sub-agent (`parent_tool_use_id` set) has a context of its own
/// and does not count (OR-57-27).
pub fn call_context_tokens(frame: &JsonValue) -> Option<u64> {
    if frame["type"] != "assistant" || !frame["parent_tool_use_id"].is_null() {
        return None;
    }
    let usage = frame["message"]["usage"].as_object()?;
    let field = |k: &str| usage.get(k).and_then(JsonValue::as_u64).unwrap_or(0);
    Some(
        field("input_tokens")
            + field("cache_creation_input_tokens")
            + field("cache_read_input_tokens"),
    )
}

/// Translate one `stream-json` frame into a signal.
pub fn classify(frame: &JsonValue) -> HarnessSignal {
    match frame["type"].as_str() {
        Some("system") if frame["subtype"] == "init" => match frame["session_id"].as_str() {
            Some(session_id) => HarnessSignal::Started {
                session_id: session_id.to_string(),
                model: frame["model"].as_str().map(str::to_string),
            },
            None => HarnessSignal::Ignored,
        },
        Some("assistant") => classify_assistant(frame),
        Some("result") => HarnessSignal::Finished {
            // `is_error` decides, not the subtype: a run that hit its turn
            // limit still writes a result event, and reporting that as success
            // would be the worst kind of wrong.
            ok: !frame["is_error"].as_bool().unwrap_or(false),
            num_turns: frame["num_turns"].as_u64(),
            cost_usd: frame["total_cost_usd"].as_f64(),
            // A run that never got going (a `--resume` of an unknown session,
            // measured on CLI 2.1.322) carries no `result` text, only
            // `errors`; the first error is then the turn's text (OR-57-25).
            text: frame["result"]
                .as_str()
                .or_else(|| frame["errors"][0].as_str())
                .map(str::to_string),
        },
        Some("control_request") if frame["request"]["subtype"] == "can_use_tool" => {
            match frame["request_id"].as_str() {
                Some(request_id) => HarnessSignal::Question {
                    request_id: request_id.to_string(),
                    tool_name: frame["request"]["tool_name"]
                        .as_str()
                        .unwrap_or("<unknown>")
                        .to_string(),
                    input: frame["request"]["input"].clone(),
                },
                None => HarnessSignal::Ignored,
            }
        }
        _ => HarnessSignal::Ignored,
    }
}

/// An assistant turn is either talk or an intent to touch the workspace. The
/// second kind is what an operator actually wants to see stream past.
fn classify_assistant(frame: &JsonValue) -> HarnessSignal {
    let Some(blocks) = frame["message"]["content"].as_array() else {
        return HarnessSignal::Ignored;
    };
    for block in blocks {
        match block["type"].as_str() {
            Some("tool_use") => {
                return HarnessSignal::Progress {
                    phase: "tool",
                    tool_name: block["name"].as_str().map(str::to_string),
                    text: block["input"]["command"].as_str().map(str::to_string),
                };
            }
            Some("text") => {
                return HarnessSignal::Progress {
                    phase: "text",
                    tool_name: None,
                    text: block["text"].as_str().map(str::to_string),
                };
            }
            _ => continue,
        }
    }
    HarnessSignal::Ignored
}

/// Build the answer to a `can_use_tool` request, in the vendor's shape.
pub fn build_control_response(request_id: &str, allow: bool, message: &str) -> JsonValue {
    let decision = if allow {
        json!({"behavior": "allow"})
    } else {
        json!({"behavior": "deny", "message": message})
    };
    json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": decision
        }
    })
}
