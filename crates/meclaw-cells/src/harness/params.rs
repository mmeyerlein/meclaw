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
        "model",
        "max_turns",
        "max_budget_usd",
        "startup_timeout_ms",
        "external_timeout_ms",
        "query_timeout_ms",
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
        })
    }
}

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
            env: string_map(obj, "env")?,
            env_passthrough: match obj.get("env_passthrough") {
                None => DEFAULT_ENV_PASSTHROUGH
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                Some(_) => string_array(obj, "env_passthrough")?,
            },
            approval,
            startup_timeout_ms: u64_or(obj, "startup_timeout_ms", 60_000),
            external_timeout_ms: u64_or(obj, "external_timeout_ms", 30_000),
            query_timeout_ms: u64_or(obj, "query_timeout_ms", 5_000),
            kill_grace_ms: u64_or(obj, "kill_grace_ms", 5_000),
        })
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
