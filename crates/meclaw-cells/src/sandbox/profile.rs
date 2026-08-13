//! The `params.sandbox` schema: pure parsing and validation, no syscalls.
//!
//! Key sets are CLOSED. An unknown key is an error, never a silently ignored
//! typo: `"netwrok": "deny"` must not read as "no network key, so use the
//! default". At a security boundary a forgiving parser is the worst property a
//! parser can have.

use meclaw_core::JsonValue;
use std::path::PathBuf;

/// Whether the sandboxed child may reach the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkPolicy {
    /// The child keeps the daemon's network namespace.
    Allow,
    /// The child runs in a fresh network namespace: only a down `lo`.
    Deny,
}

/// The declared filesystem view of a restricted cell.
///
/// `read` and `write` are absolute paths, each granted recursively (Landlock
/// `path_beneath`). `runtime` adds the interpreter/loader set so that a runner
/// binary can start at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilesystemProfile {
    /// Absolute paths the child may read and execute, recursively.
    pub read: Vec<PathBuf>,
    /// Absolute paths the child may read, write and create under, recursively.
    pub write: Vec<PathBuf>,
    /// Add the standard runtime set (`/usr`, `/lib`, `/etc`, `/proc`, the usual
    /// device nodes, ...). Defaults to `true`; without it no interpreter runs.
    pub runtime: bool,
}

/// A parsed `params.sandbox` block.
///
/// Illegal states are unrepresentable on purpose: `Trusted` carries no
/// restriction fields, because a profile that says both "trust me" and "but
/// only these paths" leaves the reader guessing which half wins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxProfile {
    /// The escape hatch for an explicitly trusted local cell. No enforcement.
    Trusted,
    /// An enforced profile.
    Restricted {
        /// Network policy for the child.
        network: NetworkPolicy,
        /// The declared filesystem view.
        filesystem: FilesystemProfile,
    },
}

/// Keys accepted inside `params.sandbox`. Closed set.
const SANDBOX_KEYS: [&str; 5] = ["trust", "network", "filesystem", "limits", "syscalls"];
/// Keys accepted inside `params.sandbox.filesystem`. Closed set.
const FS_KEYS: [&str; 3] = ["read", "write", "runtime"];

impl SandboxProfile {
    /// Parse the `sandbox` block out of a raw `params` object.
    ///
    /// Returns `Ok(None)` when no `sandbox` key is present: that is the legacy,
    /// unsandboxed behaviour and it is deliberately still the default (the
    /// switch to default-deny for template-sourced cells is GH #85).
    ///
    /// Returns `Err(operator-readable message)` for every malformed profile.
    /// This runs in `CellFactory::validate_params`, so a broken profile is a
    /// boot error, not a runtime surprise.
    pub fn parse(raw: &JsonValue) -> Result<Option<Self>, String> {
        let Some(sb) = raw.get("sandbox") else {
            return Ok(None);
        };
        let obj = sb
            .as_object()
            .ok_or("params.sandbox must be a JSON object")?;

        for key in obj.keys() {
            if !SANDBOX_KEYS.contains(&key.as_str()) {
                return Err(format!(
                    "params.sandbox: unknown key {key:?} (allowed: {})",
                    SANDBOX_KEYS.join(", ")
                ));
            }
        }

        // Reserved phase-2 keys: named in the schema, refused at load. Silently
        // ignoring them would let a config claim a cap that nothing enforces.
        for reserved in ["limits", "syscalls"] {
            if obj.contains_key(reserved) {
                return Err(format!(
                    "params.sandbox.{reserved} is reserved but NOT enforced yet \
                     (cgroup v2 caps and the seccomp filter are phase 2, tracked in GH #85). \
                     Remove the key rather than rely on it."
                ));
            }
        }

        let trust = obj
            .get("trust")
            .and_then(|v| v.as_str())
            .ok_or("params.sandbox.trust is required and must be \"restricted\" or \"trusted\"")?;

        match trust {
            "trusted" => {
                for contradiction in ["network", "filesystem"] {
                    if obj.contains_key(contradiction) {
                        return Err(format!(
                            "params.sandbox: trust \"trusted\" is the no-enforcement escape hatch \
                             and must not carry {contradiction:?}; use trust \"restricted\" to \
                             declare restrictions"
                        ));
                    }
                }
                Ok(Some(SandboxProfile::Trusted))
            }
            "restricted" => {
                let network = match obj.get("network") {
                    None => NetworkPolicy::Deny,
                    Some(v) => match v.as_str() {
                        Some("deny") => NetworkPolicy::Deny,
                        Some("allow") => NetworkPolicy::Allow,
                        other => {
                            return Err(format!(
                                "params.sandbox.network must be \"deny\" or \"allow\", got {}",
                                render(other, v)
                            ));
                        }
                    },
                };
                let fs_raw = obj.get("filesystem").ok_or(
                    "params.sandbox.filesystem is required under trust \"restricted\" \
                     (default-deny means naming what is allowed; use {\"read\":[\"/\"],\
                     \"write\":[\"/\"]} to allow everything explicitly)",
                )?;
                let filesystem = parse_filesystem(fs_raw)?;
                Ok(Some(SandboxProfile::Restricted {
                    network,
                    filesystem,
                }))
            }
            other => Err(format!(
                "params.sandbox.trust must be \"restricted\" or \"trusted\", got {other:?}"
            )),
        }
    }
}

/// Render a rejected value for an error message: the string if it is one, the
/// JSON otherwise. Keeps the offending token quoted in the operator's line.
fn render(as_str: Option<&str>, raw: &JsonValue) -> String {
    match as_str {
        Some(s) => format!("{s:?}"),
        None => raw.to_string(),
    }
}

/// Parse `params.sandbox.filesystem`.
fn parse_filesystem(raw: &JsonValue) -> Result<FilesystemProfile, String> {
    let obj = raw
        .as_object()
        .ok_or("params.sandbox.filesystem must be a JSON object")?;
    for key in obj.keys() {
        if !FS_KEYS.contains(&key.as_str()) {
            return Err(format!(
                "params.sandbox.filesystem: unknown key {key:?} (allowed: {})",
                FS_KEYS.join(", ")
            ));
        }
    }
    let read = parse_paths(obj.get("read"), "read")?;
    let write = parse_paths(obj.get("write"), "write")?;
    let runtime = match obj.get("runtime") {
        None => true,
        Some(v) => v
            .as_bool()
            .ok_or("params.sandbox.filesystem.runtime must be a boolean")?,
    };
    if read.is_empty() && write.is_empty() && !runtime {
        return Err(
            "params.sandbox.filesystem grants nothing: with no read, no write and \
                    runtime false not even the runner binary is reachable"
                .into(),
        );
    }
    Ok(FilesystemProfile {
        read,
        write,
        runtime,
    })
}

/// Parse one path list. Absolute paths only: a relative path would resolve
/// against whatever cwd the daemon happens to have, which is not a boundary
/// anybody can reason about.
fn parse_paths(raw: Option<&JsonValue>, field: &str) -> Result<Vec<PathBuf>, String> {
    let Some(v) = raw else {
        return Ok(Vec::new());
    };
    let arr = v
        .as_array()
        .ok_or_else(|| format!("params.sandbox.filesystem.{field} must be an array of paths"))?;
    let mut out = Vec::with_capacity(arr.len());
    for entry in arr {
        let s = entry.as_str().ok_or_else(|| {
            format!("params.sandbox.filesystem.{field}: every entry must be a string, got {entry}")
        })?;
        if s.is_empty() {
            return Err(format!(
                "params.sandbox.filesystem.{field}: empty path entry"
            ));
        }
        if !s.starts_with('/') {
            return Err(format!(
                "params.sandbox.filesystem.{field}: {s:?} must be an absolute path"
            ));
        }
        out.push(PathBuf::from(s));
    }
    Ok(out)
}
