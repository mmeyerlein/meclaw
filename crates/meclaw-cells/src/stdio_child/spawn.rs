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
#[derive(Debug, Clone)]
pub struct ChildSpec {
    /// Program to execute. Not run through a shell.
    pub program: String,
    /// Arguments, passed verbatim.
    pub args: Vec<String>,
    /// Additional environment variables, merged onto the inherited env.
    pub env: Vec<(String, String)>,
    /// Working directory; `None` inherits the colony's.
    pub cwd: Option<PathBuf>,
    /// Grace between "please stop" and SIGKILL when terminating.
    pub kill_grace_ms: u64,
}

/// A running child process plus its line-JSON pipes.
///
/// stderr is deliberately inherited rather than piped: nobody drains a piped
/// stderr in this design, and a full stderr pipe would wedge the child.
pub struct StdioChild {
    pub(crate) child: Child,
    pub(crate) stdin: ChildStdin,
    pub(crate) stdout: ChildLines,
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
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        if let Some(dir) = &spec.cwd {
            cmd.current_dir(dir);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| StdioChildError::Spawn(e.to_string()))?;
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
    pub async fn terminate(mut self, grace: std::time::Duration) -> ChildExit {
        drop(self.stdin);
        match tokio::time::timeout(grace, self.child.wait()).await {
            Ok(Ok(status)) => exit_of(status),
            Ok(Err(_)) => ChildExit::SpawnLost,
            Err(_) => {
                let _ = self.child.start_kill();
                match self.child.wait().await {
                    Ok(status) => exit_of(status),
                    Err(_) => ChildExit::SpawnLost,
                }
            }
        }
    }
}

/// Classify a finished process: a status code if it exited normally, otherwise
/// it was terminated by a signal.
pub(crate) fn exit_of(status: std::process::ExitStatus) -> ChildExit {
    match status.code() {
        Some(c) => ChildExit::Code(c),
        None => ChildExit::Signal,
    }
}
