//! Spawning and owning the child process.
//!
//! `StdioChild` owns all three halves that matter — the `Child` handle, the
//! stdin writer and a buffered stdout reader — so exactly one task can own the
//! process. In the dual-task pattern that task is the I/O sub-task; the
//! handler never touches a pipe (see the plan's ownership section).

use crate::stdio_child::error::{ChildExit, StdioChildError};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};

/// The child's stdout as a cancel-safe line stream. `Lines::next_line` is the
/// only read primitive here that survives being dropped in a `select!` arm.
pub type ChildLines = Lines<BufReader<ChildStdout>>;

/// How to start a line-JSON child process.
///
/// `Default` gives the inert shape (no program, everything off); consumers fill
/// in what they need. The two boolean switches were added in P8 for the
/// `harness` cell type and both default to the pre-P8 behaviour, so `mcp` is
/// unaffected.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChildSpec {
    /// Program to execute. Not run through a shell.
    pub program: String,
    /// Arguments, passed verbatim.
    pub args: Vec<String>,
    /// Additional environment variables, merged onto the inherited env — or,
    /// with `env_clear`, the ONLY environment the child gets.
    pub env: Vec<(String, String)>,
    /// Working directory; `None` inherits the colony's.
    pub cwd: Option<PathBuf>,
    /// Grace between "please stop" and SIGKILL when terminating.
    pub kill_grace_ms: u64,
    /// Start the child in its OWN process group, so its descendants can be
    /// reaped as a unit. Required for children that spawn process trees of
    /// their own (an agent harness does); pointless for a well-behaved single
    /// process, and not free — it also detaches the child from the parent's
    /// terminal signals.
    pub process_group: bool,
    /// Wipe the inherited environment before applying `env`. The containment
    /// answer to "a child process reads the colony's secrets": with this on,
    /// the child sees exactly what was handed to it and nothing else.
    pub env_clear: bool,
}

/// A running child process plus its line-JSON pipes.
///
/// stderr is deliberately inherited rather than piped: nobody drains a piped
/// stderr in this design, and a full stderr pipe would wedge the child.
pub struct StdioChild {
    pub(crate) child: Child,
    pub(crate) stdin: ChildStdin,
    pub(crate) stdout: ChildLines,
    /// Reaps the child's process group on every teardown path, including the
    /// ones that never reach `terminate`.
    pub(crate) guard: ProcessGroupGuard,
}

/// Kills the child's process group when it goes out of scope.
///
/// `kill_on_drop` covers the direct child; this covers its descendants. The
/// distinction matters on the paths where nothing is awaited any more — a
/// cancelled I/O task, a panicking peer, colony shutdown. `Drop` is the only
/// code that still runs there, and `killpg` is a synchronous syscall, so it
/// works inside one.
#[derive(Debug)]
pub(crate) struct ProcessGroupGuard {
    /// The group id, captured at spawn time because `Child::id()` goes `None`
    /// once the process is reaped — which is exactly when it is still needed.
    /// `None` means "no group of its own, nothing to sweep".
    pgid: Option<u32>,
}

impl ProcessGroupGuard {
    /// The group this guard would sweep.
    pub(crate) fn pgid(&self) -> Option<u32> {
        self.pgid
    }

    /// Give up ownership of the sweep, after the group has just been reaped
    /// deliberately. Without this the guard could fire a second `SIGKILL` at a
    /// group id that the kernel has meanwhile handed to somebody else.
    pub(crate) fn disarm(&mut self) {
        self.pgid = None;
    }
}

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        signal_group(self.pgid, reap_signals::SIGKILL);
    }
}

impl StdioChild {
    /// Start the process. Synchronous on purpose: cell factories must stay
    /// await-free between the cell.db open and the task spawn (phase-5
    /// restart-barrier tripwire).
    pub fn spawn(spec: &ChildSpec) -> Result<Self, StdioChildError> {
        let mut cmd = tokio::process::Command::new(&spec.program);
        cmd.args(&spec.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            // Backstop for every teardown path we do not walk explicitly
            // (task abort on mailbox close, peer panic, colony exit).
            //
            // Load-bearing, verified by mutation: with `false`, an aborted I/O
            // task leaves the child running AND — because stderr is inherited
            // — holding the parent's stderr pipe open, which wedges the whole
            // test run instead of merely leaking a process.
            .kill_on_drop(true);
        // Containment before configuration: wiping first and applying `env`
        // afterwards is what makes the explicit list exhaustive.
        if spec.env_clear {
            cmd.env_clear();
        }
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        if let Some(dir) = &spec.cwd {
            cmd.current_dir(dir);
        }
        // `0` means "become the leader of a new group whose id is your own
        // pid" — which is what makes the group addressable from here.
        #[cfg(unix)]
        if spec.process_group {
            cmd.process_group(0);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| StdioChildError::Spawn(e.to_string()))?;
        let pgid = if spec.process_group { child.id() } else { None };
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| StdioChildError::Spawn("stdin not piped".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| StdioChildError::Spawn("stdout not piped".into()))?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
            guard: ProcessGroupGuard { pgid },
        })
    }

    /// OS process id while the child is running; `None` once it was reaped.
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// End the child and reap it. Returns only once the process is gone.
    ///
    /// Three stages, escalating: close stdin (a well-behaved line-JSON child
    /// exits on EOF), wait for `grace`, then SIGKILL and wait unconditionally.
    /// The final `wait()` is what turns a killed process into a reaped one —
    /// without it we would leave a zombie behind.
    ///
    /// With a process group every stage addresses the GROUP instead of the one
    /// process, and a final `SIGKILL` sweep follows. The sweep is not
    /// redundant: a child that exits cleanly on stdin EOF takes none of its
    /// descendants with it, so without it a harness's shell jobs would outlive
    /// the cell — the whole point of the group.
    pub async fn terminate(mut self, grace: std::time::Duration) -> ChildExit {
        drop(self.stdin);
        let pgid = self.guard.pgid();
        signal_group(pgid, reap_signals::SIGTERM);
        let exit = match tokio::time::timeout(grace, self.child.wait()).await {
            Ok(Ok(status)) => exit_of(status),
            Ok(Err(_)) => ChildExit::SpawnLost,
            Err(_) => {
                match pgid {
                    Some(_) => signal_group(pgid, reap_signals::SIGKILL),
                    None => {
                        let _ = self.child.start_kill();
                    }
                }
                match self.child.wait().await {
                    Ok(status) => exit_of(status),
                    Err(_) => ChildExit::SpawnLost,
                }
            }
        };
        // Sweep whatever the leader left behind. Safe against pid reuse: a pid
        // is not handed out again while it still names a live process group,
        // and if the group is empty the call simply reports ESRCH.
        signal_group(pgid, reap_signals::SIGKILL);
        self.guard.disarm();
        exit
    }
}

/// Signal a process group, ignoring "already gone". A no-op without a group
/// and on non-unix targets.
pub(crate) fn signal_group(pgid: Option<u32>, signal: i32) {
    #[cfg(unix)]
    if let Some(pgid) = pgid {
        let _ = crate::stdio_child::reap::killpg(pgid, signal);
    }
    #[cfg(not(unix))]
    let _ = (pgid, signal);
}

/// The two signals used for group teardown, behind a unix-only indirection so
/// the call sites stay free of `cfg` noise.
pub(crate) mod reap_signals {
    #[cfg(unix)]
    pub(crate) const SIGTERM: i32 = crate::stdio_child::reap::SIGTERM;
    #[cfg(unix)]
    pub(crate) const SIGKILL: i32 = crate::stdio_child::reap::SIGKILL;
    #[cfg(not(unix))]
    pub(crate) const SIGTERM: i32 = 15;
    #[cfg(not(unix))]
    pub(crate) const SIGKILL: i32 = 9;
}

/// Classify a finished process: a status code if it exited normally, otherwise
/// it was terminated by a signal.
pub(crate) fn exit_of(status: std::process::ExitStatus) -> ChildExit {
    match status.code() {
        Some(c) => ChildExit::Code(c),
        None => ChildExit::Signal,
    }
}
