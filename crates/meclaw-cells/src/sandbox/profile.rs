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

/// The resource caps of a restricted cell, enforced through a delegated
/// cgroup v2 sub-cgroup (GH #85).
///
/// Every cap is optional and every declared one is enforced; a `limits` block
/// that declares none of them is rejected rather than treated as a default,
/// for the same reason the whole key was refused before enforcement existed: a
/// cap nobody enforces tells the operator a comforting lie.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResourceLimits {
    /// `memory.max` in bytes. The child is OOM-killed when it exceeds this.
    pub memory_max_bytes: Option<u64>,
    /// `pids.max`: how many tasks the child and its descendants may hold at
    /// once. The answer to a fork bomb.
    pub pids_max: Option<u64>,
    /// `cpu.max` as a percentage of ONE core, so `200` means two whole cores.
    /// Written against a fixed 100 ms period.
    pub cpu_max_percent: Option<u64>,
}

impl ResourceLimits {
    /// Whether this block caps anything at all.
    fn is_empty(&self) -> bool {
        self.memory_max_bytes.is_none() && self.pids_max.is_none() && self.cpu_max_percent.is_none()
    }
}

/// The syscall axes a restricted cell may close, enforced through a
/// seccomp-bpf filter (GH #85).
///
/// Each field is "this axis is denied". The axes are the gaps Landlock leaves
/// open: it is a filesystem LSM and says nothing about `ptrace`, about signals
/// to processes outside the sandbox, or about raw sockets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyscallPolicy {
    /// Deny `ptrace` and its two memory-peeking relatives
    /// (`process_vm_readv`/`process_vm_writev`).
    pub deny_ptrace: bool,
    /// Deny `AF_PACKET` sockets and `SOCK_RAW` of any family.
    pub deny_raw_sockets: bool,
    /// Deny every signal whose target is not the sandboxed process itself.
    pub deny_foreign_signals: bool,
}

impl SyscallPolicy {
    /// Whether this policy closes anything at all.
    fn is_empty(&self) -> bool {
        !self.deny_ptrace && !self.deny_raw_sockets && !self.deny_foreign_signals
    }
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
        /// Resource caps, `None` when the operator declared none.
        limits: Option<ResourceLimits>,
        /// Syscall filter axes, `None` when the operator declared none.
        syscalls: Option<SyscallPolicy>,
    },
}

/// Keys accepted inside `params.sandbox`. Closed set.
const SANDBOX_KEYS: [&str; 5] = ["trust", "network", "filesystem", "limits", "syscalls"];
/// Keys accepted inside `params.sandbox.filesystem`. Closed set.
const FS_KEYS: [&str; 3] = ["read", "write", "runtime"];
/// Keys accepted inside `params.sandbox.limits`. Closed set.
const LIMIT_KEYS: [&str; 3] = ["memory_max_bytes", "pids_max", "cpu_max_percent"];
/// Keys accepted inside `params.sandbox.syscalls`. Closed set.
const SYSCALL_KEYS: [&str; 3] = ["ptrace", "raw_sockets", "foreign_signals"];

impl SandboxProfile {
    /// Parse the `sandbox` block out of a raw `params` object.
    ///
    /// Returns `Ok(None)` when no `sandbox` key is present, which still means
    /// the unsandboxed behaviour. The default-deny cut of GH #85 does NOT sit
    /// here: it fills the block in at INSTANTIATION time, in
    /// `meclaw_colony::mutation::stage`, so that a cell born from a template
    /// arrives with a profile it can be read off its own `config.json`. This
    /// parser stays the pre-cut reader on purpose, because that is what makes
    /// the cut prospective -- a hand-written tree already on disk keeps running
    /// exactly as it did.
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

        let trust = obj
            .get("trust")
            .and_then(|v| v.as_str())
            .ok_or("params.sandbox.trust is required and must be \"restricted\" or \"trusted\"")?;

        match trust {
            "trusted" => {
                for contradiction in ["network", "filesystem", "limits", "syscalls"] {
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
                let limits = obj.get("limits").map(parse_limits).transpose()?;
                let syscalls = obj.get("syscalls").map(parse_syscalls).transpose()?;
                Ok(Some(SandboxProfile::Restricted {
                    network,
                    filesystem,
                    limits,
                    syscalls,
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

/// Parse `params.sandbox.limits` (GH #85).
///
/// Every cap is optional, but the block as a whole must cap something: an
/// empty `limits` object would read as "capped" while enforcing nothing.
fn parse_limits(raw: &JsonValue) -> Result<ResourceLimits, String> {
    let obj = raw
        .as_object()
        .ok_or("params.sandbox.limits must be a JSON object")?;
    for key in obj.keys() {
        if !LIMIT_KEYS.contains(&key.as_str()) {
            return Err(format!(
                "params.sandbox.limits: unknown key {key:?} (allowed: {})",
                LIMIT_KEYS.join(", ")
            ));
        }
    }
    let out = ResourceLimits {
        memory_max_bytes: parse_positive(obj.get("memory_max_bytes"), "memory_max_bytes")?,
        pids_max: parse_positive(obj.get("pids_max"), "pids_max")?,
        cpu_max_percent: parse_positive(obj.get("cpu_max_percent"), "cpu_max_percent")?,
    };
    if out.is_empty() {
        return Err(format!(
            "params.sandbox.limits caps nothing: declare at least one of {}",
            LIMIT_KEYS.join(", ")
        ));
    }
    Ok(out)
}

/// One cap value. Zero and negative are rejected: a `0` cap would either mean
/// "no limit" or "cannot run at all" depending on the controller, and a
/// boundary must not depend on which one the reader assumed.
fn parse_positive(raw: Option<&JsonValue>, field: &str) -> Result<Option<u64>, String> {
    let Some(v) = raw else {
        return Ok(None);
    };
    let n = v.as_u64().ok_or_else(|| {
        format!("params.sandbox.limits.{field} must be a positive integer, got {v}")
    })?;
    if n == 0 {
        return Err(format!(
            "params.sandbox.limits.{field} must be greater than zero \
             (omit the key to leave this resource uncapped)"
        ));
    }
    Ok(Some(n))
}

/// Parse `params.sandbox.syscalls` (GH #85).
///
/// Naming the block means "filter this process", so an axis that is not
/// mentioned is DENIED. That is the same discipline `filesystem` follows:
/// default-deny means writing down what is allowed, and an axis you want to
/// keep open you spell out as `"allow"` and can see that you did.
fn parse_syscalls(raw: &JsonValue) -> Result<SyscallPolicy, String> {
    let obj = raw
        .as_object()
        .ok_or("params.sandbox.syscalls must be a JSON object")?;
    for key in obj.keys() {
        if !SYSCALL_KEYS.contains(&key.as_str()) {
            return Err(format!(
                "params.sandbox.syscalls: unknown key {key:?} (allowed: {})",
                SYSCALL_KEYS.join(", ")
            ));
        }
    }
    let out = SyscallPolicy {
        deny_ptrace: parse_axis(obj.get("ptrace"), "ptrace")?,
        deny_raw_sockets: parse_axis(obj.get("raw_sockets"), "raw_sockets")?,
        deny_foreign_signals: parse_axis(obj.get("foreign_signals"), "foreign_signals")?,
    };
    if out.is_empty() {
        return Err(format!(
            "params.sandbox.syscalls denies nothing: every axis is \"allow\", so no filter \
             would be installed at all; remove the block instead (allowed keys: {})",
            SYSCALL_KEYS.join(", ")
        ));
    }
    Ok(out)
}

/// One syscall axis. Absent means denied; the only way out is an explicit
/// `"allow"`.
fn parse_axis(raw: Option<&JsonValue>, field: &str) -> Result<bool, String> {
    match raw {
        None => Ok(true),
        Some(v) => match v.as_str() {
            Some("deny") => Ok(true),
            Some("allow") => Ok(false),
            other => Err(format!(
                "params.sandbox.syscalls.{field} must be \"deny\" or \"allow\", got {}",
                render(other, v)
            )),
        },
    }
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
