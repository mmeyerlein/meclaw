//! `HarnessParams`: the cell's birth configuration.
//!
//! Three keys are required — which vendor dialect to speak, where to emit, and
//! which directory tree tasks may work in. Everything else has a default, with
//! two deliberate exceptions: the model and the turn budget are never invented
//! here. A model belongs in the topology's `${VAR}` indices (standing rule), and
//! a budget that the operator did not set must not be conjured by a cell.

use crate::harness::adapter::HarnessAdapter;
use meclaw_core::Path;
use serde_json::Value as JsonValue;
use std::path::PathBuf;

/// Whether the harness may ask the topology for permission decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalMode {
    /// The harness never asks: it runs inside its configured permission mode,
    /// and a question it asks anyway is answered with "no" (and reported).
    Off,
    /// Questions become messages into the topology, and the answer travels
    /// back. A task then waits for as long as the topology takes to decide —
    /// exactly as a human prompt would.
    Channel,
}

/// The life cycle of the harness child (D57 V4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMode {
    /// One child per task, closed after its `result` (the default).
    PerTask,
    /// One child carries a vendor session over many tasks; each task is one
    /// turn of it. Idle, rotation and the decision table decide when it ends.
    Persistent,
}

/// The variable through which the Claude Code CLI adds headers to its LLM
/// requests (`Name: value` lines, newline-separated).
pub const CUSTOM_HEADERS_ENV: &str = "ANTHROPIC_CUSTOM_HEADERS";

/// The environment variables a child inherits when the config says nothing.
///
/// Deliberately short: `HOME` is not optional (the harness keeps its
/// credentials there), `PATH` is needed to find the tools it runs, the rest is
/// locale and terminal hygiene. Everything else — every token, every key the
/// colony happens to hold — stays behind.
const DEFAULT_ENV_PASSTHROUGH: [&str; 5] = ["PATH", "HOME", "USER", "LANG", "TERM"];

/// Parsed harness params after validation. All fields owned.
#[derive(Debug, Clone)]
pub struct HarnessParams {
    /// Which vendor dialect the child speaks.
    pub adapter: HarnessAdapter,
    /// The binary to run. Defaults to the adapter's own command name.
    pub command: String,
    /// Routing target for every origin emission (progress/question/result/
    /// error). Same role as `proxy.emit_to`.
    pub emit_to: Path,
    /// Canonical directory below which every task workspace must live. A task
    /// names its workspace relative to this root and cannot escape it.
    pub workspace_root: PathBuf,
    /// Model id, passed through verbatim. `None` leaves the choice to the
    /// harness's own configuration — and the effective model is then READ BACK
    /// from its init event rather than assumed.
    pub model: Option<String>,
    /// Vendor permission mode, passed through verbatim.
    pub permission_mode: Option<String>,
    /// Turn budget per task, if the operator set one.
    pub max_turns: Option<u64>,
    /// Money budget per task, if the operator set one.
    pub max_budget_usd: Option<f64>,
    /// Tools the harness may use. Empty = do not restrict.
    pub allowed_tools: Vec<String>,
    /// Extra verbatim arguments, an escape hatch for vendor flags this cell
    /// does not model.
    pub extra_args: Vec<String>,
    /// Environment variables handed to the child explicitly.
    pub env: Vec<(String, String)>,
    /// Inherited variables that survive the environment wipe.
    pub env_passthrough: Vec<String>,
    /// Whether permission questions reach the topology (D6).
    pub approval: ApprovalMode,
    /// A-timeout for the child's first frame after spawn. A harness that never
    /// says hello is broken; one that thinks for ten minutes is not.
    pub startup_timeout_ms: u64,
    /// A-timeout around each write to the child.
    pub external_timeout_ms: u64,
    /// A-timeout for `cell.db` calls via `DbConn`.
    pub query_timeout_ms: u64,
    /// Grace between "please stop" and SIGKILL for the child's process group.
    pub kill_grace_ms: u64,
    /// Process sandbox for the harness child (GH #85). A colony never spawns
    /// one with `None` since GH #1235: the boot and every instantiation put
    /// the default-deny block in when the params declare none. `None` is the
    /// unsandboxed child of a direct construction (tests), contained only by
    /// `env_clear`, the passthrough allow-list and the cwd clamp.
    pub sandbox: Option<crate::sandbox::SandboxProfile>,
    /// App id title for the child's LLM requests (AGENTS rule 15, OR-57-8):
    /// sent as `X-Title` through `ANTHROPIC_CUSTOM_HEADERS`. Unset = not sent.
    pub x_title: Option<String>,
    /// App id referer, sent as `HTTP-Referer` the same way. Unset = not sent.
    pub http_referer: Option<String>,
    /// Child life cycle: one per task, or one session over many (D57 V4).
    pub session: SessionMode,
    /// `persistent` only: an idle child closes after this long; the session
    /// stays open and the next task resumes it.
    pub session_idle_ms: u64,
    /// `persistent` only: the session ends once the context of the turn's
    /// last model call reaches this many tokens. Unset = never.
    pub session_max_context_tokens: Option<u64>,
    /// `persistent` only: the session ends once its cost reaches this many
    /// USD. Unset = never.
    pub session_max_usd: Option<f64>,
}

/// β: the `harness` runtime-overlay projection — everything that may be tuned
/// under a live cell.
///
/// The line between mutable and immutable here is the containment boundary,
/// not convenience. Binary, environment, workspace root, permission mode,
/// allowed tools and the approval mode are what the operator wrote the config
/// to constrain; letting a params MESSAGE change them would hand that decision
/// to whoever can route to the cell. Budgets, timeouts and the model are
/// tuning, and take effect on the NEXT task — never on the running one.
///
/// A minimal projection (not the full params) for the same reason as `mcp`'s:
/// the immutable identity never changes, so the overlay round-trip does not
/// need to reconstruct it. `KNOWN_KEYS` still lists the immutable keys, so an
/// update touching one is a loud `Immutable` reject rather than a vaguer
/// `Unknown`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct HarnessOverlay {
    /// Model id for the next task. Mutable, path A (handler side).
    pub model: Option<String>,
    /// Turn budget for the next task. Mutable, path A.
    pub max_turns: Option<u64>,
    /// Money budget for the next task. Mutable, path A.
    pub max_budget_usd: Option<f64>,
    /// A-timeout for the child's first frame. Mutable, path A.
    pub startup_timeout_ms: u64,
    /// A-timeout around writes to the child. Mutable, path A.
    pub external_timeout_ms: u64,
    /// A-timeout for cell.db ops. Mutable, path C (DbConn, live).
    pub query_timeout_ms: u64,
    /// Idle close of a persistent child. Mutable, from the next turn on.
    pub session_idle_ms: u64,
    /// Context rotation limit. Mutable, from the next turn on.
    pub session_max_context_tokens: Option<u64>,
    /// Cost rotation limit. Mutable, from the next turn on.
    pub session_max_usd: Option<f64>,
}

impl HarnessOverlay {
    /// The tunables of `params` — the `current` of a runtime params update.
    pub fn of(params: &HarnessParams) -> Self {
        Self {
            model: params.model.clone(),
            max_turns: params.max_turns,
            max_budget_usd: params.max_budget_usd,
            startup_timeout_ms: params.startup_timeout_ms,
            external_timeout_ms: params.external_timeout_ms,
            query_timeout_ms: params.query_timeout_ms,
            session_idle_ms: params.session_idle_ms,
            session_max_context_tokens: params.session_max_context_tokens,
            session_max_usd: params.session_max_usd,
        }
    }

    /// Carry the tunables into `params`. The one path for both the runtime
    /// params update (`cell.rs`) and the β restore (`factory.rs`): the review
    /// of d57 s1 found the same nine lines kept by hand in three places and
    /// none of them pinned (Minor 3). The destructuring makes a new field a
    /// compile error here instead of a silently dropped key.
    pub fn apply_to(self, params: &mut HarnessParams) {
        let Self {
            model,
            max_turns,
            max_budget_usd,
            startup_timeout_ms,
            external_timeout_ms,
            query_timeout_ms,
            session_idle_ms,
            session_max_context_tokens,
            session_max_usd,
        } = self;
        params.model = model;
        params.max_turns = max_turns;
        params.max_budget_usd = max_budget_usd;
        params.startup_timeout_ms = startup_timeout_ms;
        params.external_timeout_ms = external_timeout_ms;
        params.query_timeout_ms = query_timeout_ms;
        params.session_idle_ms = session_idle_ms;
        params.session_max_context_tokens = session_max_context_tokens;
        params.session_max_usd = session_max_usd;
    }
}

impl crate::params_overlay::OverlayParams for HarnessOverlay {
    const KNOWN_KEYS: &'static [&'static str] = &[
        "adapter",
        "command",
        "emit_to",
        "workspace_root",
        "env",
        "env_passthrough",
        "permission_mode",
        "allowed_tools",
        "extra_args",
        "approval",
        "kill_grace_ms",
        "sandbox",
        "x_title",
        "http_referer",
        "session",
        "model",
        "max_turns",
        "max_budget_usd",
        "startup_timeout_ms",
        "external_timeout_ms",
        "query_timeout_ms",
        "session_idle_ms",
        "session_max_context_tokens",
        "session_max_usd",
    ];
    const IMMUTABLE_KEYS: &'static [&'static str] = &[
        "adapter",
        "command",
        "emit_to",
        "workspace_root",
        "env",
        "env_passthrough",
        "permission_mode",
        "allowed_tools",
        "extra_args",
        "approval",
        "kill_grace_ms",
        // A security boundary a message can move is not a boundary, so the
        // sandbox is immutable by name rather than merely unknown (GH #85).
        "sandbox",
        // The app id names who sends the child's requests; the operator
        // writes it, a message does not (OR-57-8).
        "x_title",
        "http_referer",
        // A running session cannot change its life cycle under itself.
        "session",
    ];

    fn parse(raw: &JsonValue) -> Result<Self, String> {
        let obj = raw.as_object().ok_or("params: must be object")?;
        Ok(Self {
            model: opt_string(obj, "model"),
            max_turns: obj.get("max_turns").and_then(|x| x.as_u64()),
            max_budget_usd: obj.get("max_budget_usd").and_then(|x| x.as_f64()),
            startup_timeout_ms: u64_or(obj, "startup_timeout_ms", 60_000),
            external_timeout_ms: u64_or(obj, "external_timeout_ms", 30_000),
            query_timeout_ms: u64_or(obj, "query_timeout_ms", 5_000),
            session_idle_ms: u64_or(obj, "session_idle_ms", DEFAULT_SESSION_IDLE_MS),
            session_max_context_tokens: obj
                .get("session_max_context_tokens")
                .and_then(|x| x.as_u64()),
            session_max_usd: obj.get("session_max_usd").and_then(|x| x.as_f64()),
        })
    }
}

/// Default idle close of a persistent child: 30 minutes (D57 V4).
const DEFAULT_SESSION_IDLE_MS: u64 = 1_800_000;

impl HarnessParams {
    /// Parse + validate. Required fields are rejected by name.
    ///
    /// This is the single parse path behind both `validate_params` and
    /// `spawn_cell` (parser invariant, `CellFactory` docs): the filesystem
    /// checks on `workspace_root` happen HERE, so a config that validates
    /// cannot fail at spawn time.
    pub fn parse(v: &JsonValue) -> Result<Self, String> {
        let obj = v.as_object().ok_or("params: must be object")?;

        let adapter_name = obj
            .get("adapter")
            .and_then(|x| x.as_str())
            .ok_or("adapter: required (\"claude-code\")")?;
        let adapter = HarnessAdapter::parse(adapter_name)?;

        let emit_to = obj
            .get("emit_to")
            .and_then(|x| x.as_str())
            .ok_or("emit_to: required (routing target for harness emissions)")?;
        let emit_to = Path::new(emit_to);

        let workspace_root = obj
            .get("workspace_root")
            .and_then(|x| x.as_str())
            .ok_or("workspace_root: required (directory every task workspace lives under)")?;
        let workspace_root = std::fs::canonicalize(workspace_root)
            .map_err(|e| format!("workspace_root: {workspace_root:?} unusable: {e}"))?;
        if !workspace_root.is_dir() {
            return Err(format!(
                "workspace_root: {} is not a directory",
                workspace_root.display()
            ));
        }

        let approval = match obj.get("approval").and_then(|x| x.as_str()) {
            None | Some("off") => ApprovalMode::Off,
            Some("channel") => ApprovalMode::Channel,
            Some(other) => return Err(format!("approval: unknown value {other:?} (off|channel)")),
        };

        let session = match obj.get("session").and_then(|x| x.as_str()) {
            None | Some("per_task") => SessionMode::PerTask,
            Some("persistent") => SessionMode::Persistent,
            Some(other) => {
                return Err(format!(
                    "session: unknown value {other:?} (per_task|persistent)"
                ));
            }
        };

        let env = string_map(obj, "env")?;
        let env_passthrough = match obj.get("env_passthrough") {
            None => DEFAULT_ENV_PASSTHROUGH
                .iter()
                .map(|s| s.to_string())
                .collect(),
            Some(_) => string_array(obj, "env_passthrough")?,
        };
        let x_title = opt_string(obj, "x_title");
        let http_referer = opt_string(obj, "http_referer");
        if x_title.is_some() || http_referer.is_some() {
            // Two sources for one variable: one of them would silently lose
            // (OR-57-8). A passed-through variable is a second source just
            // like an explicit one (`child_spec` writes both into the env).
            let source = if env.iter().any(|(k, _)| k == CUSTOM_HEADERS_ENV) {
                Some(format!("env.{CUSTOM_HEADERS_ENV}"))
            } else if env_passthrough.iter().any(|k| k == CUSTOM_HEADERS_ENV) {
                Some(format!("env_passthrough entry {CUSTOM_HEADERS_ENV}"))
            } else {
                None
            };
            if let Some(source) = source {
                return Err(format!(
                    "x_title/http_referer: cannot be combined with {source} \
                     (both set the child's custom headers)"
                ));
            }
        }

        Ok(Self {
            command: obj
                .get("command")
                .and_then(|x| x.as_str())
                .unwrap_or(adapter.default_command())
                .to_string(),
            adapter,
            emit_to,
            workspace_root,
            model: opt_string(obj, "model"),
            permission_mode: opt_string(obj, "permission_mode"),
            max_turns: obj.get("max_turns").and_then(|x| x.as_u64()),
            max_budget_usd: obj.get("max_budget_usd").and_then(|x| x.as_f64()),
            allowed_tools: string_array(obj, "allowed_tools")?,
            extra_args: string_array(obj, "extra_args")?,
            env,
            env_passthrough,
            approval,
            startup_timeout_ms: u64_or(obj, "startup_timeout_ms", 60_000),
            external_timeout_ms: u64_or(obj, "external_timeout_ms", 30_000),
            query_timeout_ms: u64_or(obj, "query_timeout_ms", 5_000),
            kill_grace_ms: u64_or(obj, "kill_grace_ms", 5_000),
            // Read from the BIRTH params only: `parse` is the single path
            // behind `validate_params` and `spawn_cell`, and the runtime
            // overlay lists `sandbox` as immutable.
            sandbox: crate::sandbox::SandboxProfile::parse(v)?,
            x_title,
            http_referer,
            session,
            session_idle_ms: u64_or(obj, "session_idle_ms", DEFAULT_SESSION_IDLE_MS),
            session_max_context_tokens: obj
                .get("session_max_context_tokens")
                .and_then(|x| x.as_u64()),
            session_max_usd: obj.get("session_max_usd").and_then(|x| x.as_f64()),
        })
    }
}

impl HarnessParams {
    /// The app id as the value of [`CUSTOM_HEADERS_ENV`]: one line per set
    /// key, `None` when neither is set (B6, OR-57-8).
    pub fn custom_headers(&self) -> Option<String> {
        let lines: Vec<String> = [
            ("X-Title", &self.x_title),
            ("HTTP-Referer", &self.http_referer),
        ]
        .iter()
        .filter_map(|(name, value)| value.as_ref().map(|v| format!("{name}: {v}")))
        .collect();
        (!lines.is_empty()).then(|| lines.join("\n"))
    }
}

fn opt_string(obj: &serde_json::Map<String, JsonValue>, key: &str) -> Option<String> {
    obj.get(key).and_then(|x| x.as_str()).map(|s| s.to_string())
}

fn u64_or(obj: &serde_json::Map<String, JsonValue>, key: &str, default: u64) -> u64 {
    obj.get(key).and_then(|x| x.as_u64()).unwrap_or(default)
}

fn string_array(
    obj: &serde_json::Map<String, JsonValue>,
    key: &str,
) -> Result<Vec<String>, String> {
    match obj.get(key) {
        None => Ok(Vec::new()),
        Some(JsonValue::Array(a)) => a
            .iter()
            .map(|x| {
                x.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("{key}: must be an array of strings"))
            })
            .collect(),
        Some(_) => Err(format!("{key}: must be an array of strings")),
    }
}

fn string_map(
    obj: &serde_json::Map<String, JsonValue>,
    key: &str,
) -> Result<Vec<(String, String)>, String> {
    match obj.get(key) {
        None => Ok(Vec::new()),
        Some(JsonValue::Object(m)) => m
            .iter()
            .map(|(k, v)| {
                v.as_str()
                    .map(|s| (k.clone(), s.to_string()))
                    .ok_or_else(|| format!("{key}: values must be strings"))
            })
            .collect(),
        Some(_) => Err(format!("{key}: must be an object of string values")),
    }
}
