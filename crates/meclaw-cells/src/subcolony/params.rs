//! `SubcolonyParams`: the facade's birth configuration.
//!
//! One key is required — which filesystem tree the child colony lives in.
//! Everything else has a default, and the defaults are deliberately generous
//! where the child does the work (a child colony may contain an `llm` cell, so a
//! request budget of two minutes is not paranoia) and tight where this process
//! does (a child that has not said hello in half a minute is broken).
//!
//! The split between mutable and immutable is the containment boundary, not
//! convenience: which binary runs, on which tree, with which environment, and
//! which context keys may cross into the child are what the operator wrote the
//! config to constrain. Letting a params MESSAGE change them would hand that
//! decision to whoever can route to this cell.

use meclaw_core::Path;
use serde_json::Value as JsonValue;
use std::path::PathBuf;

/// The context key the facade owns. A config may not map onto it: it is the
/// correlation key a pending request is waiting on, and a second writer would
/// make answers unmatchable.
pub const RESERVED_CONTEXT_KEY: &str = "turn_id";

/// Environment variables the child inherits when the config says nothing.
///
/// Deliberately short, and the reason `env_clear` is on: a child colony gets a
/// process of its own precisely so the parent's secrets are not its secrets.
const DEFAULT_ENV_PASSTHROUGH: [&str; 5] = ["PATH", "HOME", "USER", "LANG", "TERM"];

/// Parsed `subcolony` params after validation. All fields owned.
#[derive(Debug, Clone)]
pub struct SubcolonyParams {
    /// Filesystem root of the child colony, canonicalised.
    pub root: PathBuf,
    /// The binary to run. Configurable so version skew is an operator decision.
    pub command: String,
    /// Environment handed to the child explicitly.
    pub env: Vec<(String, String)>,
    /// Inherited variables that survive the environment wipe.
    pub env_passthrough: Vec<String>,
    /// Explicit parent-context-key → child-context-key mapping. Empty by
    /// default: nothing crosses the boundary unless it is declared.
    pub context_in: Vec<(String, String)>,
    /// Where to route child frames nobody asked for. `None` drops them with a
    /// warning.
    pub emit_to: Option<Path>,
    /// Extra verbatim arguments for the child binary. An escape hatch for
    /// operator flags this cell does not model (`--log-level`, `--env`, ...).
    /// The wire flags are NOT reachable from here: `--stdio-format` is part of
    /// the boundary, not a tuning knob.
    pub extra_args: Vec<String>,
    /// A-timeout for the child's `ready` frame.
    pub boot_timeout_ms: u64,
    /// A-timeout for one correlated request.
    pub request_timeout_ms: u64,
    /// A-timeout around each write to the child.
    pub external_timeout_ms: u64,
    /// A-timeout for `cell.db` calls via `DbConn`.
    pub query_timeout_ms: u64,
    /// Grace between "please stop" and SIGKILL for the child's process group.
    pub kill_grace_ms: u64,
}

/// β: the runtime-overlay projection — everything that may be tuned under a live
/// cell. Timeouts only; they take effect on the NEXT request.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SubcolonyOverlay {
    /// A-timeout for the child's `ready` frame, for the next boot.
    pub boot_timeout_ms: u64,
    /// A-timeout for one correlated request.
    pub request_timeout_ms: u64,
    /// A-timeout around each write to the child.
    pub external_timeout_ms: u64,
    /// A-timeout for cell.db ops.
    pub query_timeout_ms: u64,
}

impl crate::params_overlay::OverlayParams for SubcolonyOverlay {
    const KNOWN_KEYS: &'static [&'static str] = &[
        "root",
        "command",
        "env",
        "env_passthrough",
        "context_in",
        "emit_to",
        "extra_args",
        "kill_grace_ms",
        "boot_timeout_ms",
        "request_timeout_ms",
        "external_timeout_ms",
        "query_timeout_ms",
    ];
    const IMMUTABLE_KEYS: &'static [&'static str] = &[
        "root",
        "command",
        "env",
        "env_passthrough",
        "context_in",
        "emit_to",
        "extra_args",
        "kill_grace_ms",
    ];

    fn parse(raw: &JsonValue) -> Result<Self, String> {
        let obj = raw.as_object().ok_or("params: must be object")?;
        Ok(Self {
            boot_timeout_ms: u64_or(obj, "boot_timeout_ms", 30_000),
            request_timeout_ms: u64_or(obj, "request_timeout_ms", 120_000),
            external_timeout_ms: u64_or(obj, "external_timeout_ms", 30_000),
            query_timeout_ms: u64_or(obj, "query_timeout_ms", 5_000),
        })
    }
}

impl SubcolonyParams {
    /// Parse + validate. Required fields are rejected by name.
    ///
    /// The filesystem check on `root` happens HERE, so a config that validates
    /// cannot fail at spawn time (parser invariant, `CellFactory` docs).
    pub fn parse(v: &JsonValue) -> Result<Self, String> {
        let obj = v.as_object().ok_or("params: must be object")?;

        let root = obj
            .get("root")
            .and_then(|x| x.as_str())
            .ok_or("root: required (filesystem root of the child colony)")?;
        let root =
            std::fs::canonicalize(root).map_err(|e| format!("root: {root:?} unusable: {e}"))?;
        if !root.is_dir() {
            return Err(format!("root: {} is not a directory", root.display()));
        }

        Ok(Self {
            root,
            command: obj
                .get("command")
                .and_then(|x| x.as_str())
                .unwrap_or("meclaw")
                .to_string(),
            env: string_map(obj, "env")?,
            env_passthrough: match obj.get("env_passthrough") {
                None => DEFAULT_ENV_PASSTHROUGH
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                Some(_) => string_array(obj, "env_passthrough")?,
            },
            context_in: context_mapping(obj)?,
            emit_to: obj.get("emit_to").and_then(|x| x.as_str()).map(Path::new),
            extra_args: string_array(obj, "extra_args")?,
            boot_timeout_ms: u64_or(obj, "boot_timeout_ms", 30_000),
            request_timeout_ms: u64_or(obj, "request_timeout_ms", 120_000),
            external_timeout_ms: u64_or(obj, "external_timeout_ms", 30_000),
            query_timeout_ms: u64_or(obj, "query_timeout_ms", 5_000),
            kill_grace_ms: u64_or(obj, "kill_grace_ms", 5_000),
        })
    }
}

/// Parse `context_in` — the only door through the header boundary.
fn context_mapping(
    obj: &serde_json::Map<String, JsonValue>,
) -> Result<Vec<(String, String)>, String> {
    let Some(raw) = obj.get("context_in") else {
        return Ok(Vec::new());
    };
    let map = raw
        .as_object()
        .ok_or("context_in: must be an object of parent-key -> child-key strings")?;
    let mut out = Vec::with_capacity(map.len());
    for (from, to) in map {
        let to = to
            .as_str()
            .ok_or_else(|| format!("context_in: value for {from:?} must be a string"))?;
        if to == RESERVED_CONTEXT_KEY {
            return Err(format!(
                "context_in: {RESERVED_CONTEXT_KEY:?} is reserved by the boundary (it is the correlation key) — map {from:?} onto another name"
            ));
        }
        out.push((from.clone(), to.to_string()));
    }
    Ok(out)
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
