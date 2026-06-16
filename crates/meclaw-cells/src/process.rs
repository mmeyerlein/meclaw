//! Shared subprocess helper for `bash` and `code` cells.
//!
//! `with_killing_timeout` kills the child if `timeout` elapses
//! (tokio::process::Child wird beim Drop NICHT automatisch gekillt —
//! Pipe via `.stdout.take()`/`.stderr.take()`, `child.wait()` unter
//! `tokio::time::timeout`, bei Elapsed `start_kill() + wait().await`
//! zum Reapen).
//!
//! Used by `bash::*` (Phase 7) and `code::*` (Phase 9).

use tokio::io::AsyncReadExt;

/// Output of a successful subprocess run.
#[derive(Debug)]
pub(crate) struct KillingTimeoutOutput {
    /// `i32` because `ExitStatus::code()` returns `i32`. `-1` for abnormal
    /// termination (signal-killed, etc.) — convention per Brainstorm-Decision 1.6.
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Error modes of the `with_killing_timeout` wrapper.
#[derive(Debug)]
pub(crate) enum KillingTimeoutErr {
    /// Timeout — child was killed and reaped.
    Elapsed,
    /// I/O error during read/wait.
    Io(std::io::Error),
}

/// Waits for `child` with a timeout. On elapsed: kill-and-reap (child is
/// guaranteed not runnable after return). Pipes are read concurrently so that
/// full pipe buffers cannot block the child.
///
/// **Not via `wait_with_output()`**: that consumes `Child` making kill-on-
/// elapsed impossible. Instead: `take()` the pipes, parallel `read_to_end`
/// futures, `child.wait()` (`&mut self`) under `timeout`.
pub(crate) async fn with_killing_timeout(
    mut child: tokio::process::Child,
    timeout: std::time::Duration,
) -> Result<KillingTimeoutOutput, KillingTimeoutErr> {
    let mut stdout_pipe = child
        .stdout
        .take()
        .expect("stdout piped (must use Stdio::piped())");
    let mut stderr_pipe = child
        .stderr
        .take()
        .expect("stderr piped (must use Stdio::piped())");

    let stdout_fut = async move {
        let mut buf = Vec::new();
        stdout_pipe.read_to_end(&mut buf).await.map(|_| buf)
    };
    let stderr_fut = async move {
        let mut buf = Vec::new();
        stderr_pipe.read_to_end(&mut buf).await.map(|_| buf)
    };

    // NICHT 'async move' — combined borrows &mut child via child.wait()
    // over the try_join lifetime. When tokio::time::timeout drops the future
    // on Elapsed, the borrow ends and child.start_kill() is reachable again.
    let combined = async {
        let (status, stdout, stderr) = tokio::try_join!(child.wait(), stdout_fut, stderr_fut)?;
        Ok::<(std::process::ExitStatus, Vec<u8>, Vec<u8>), std::io::Error>((status, stdout, stderr))
    };

    match tokio::time::timeout(timeout, combined).await {
        Ok(Ok((status, stdout, stderr))) => {
            let exit_code = status.code().unwrap_or(-1);
            Ok(KillingTimeoutOutput {
                exit_code,
                stdout,
                stderr,
            })
        }
        Ok(Err(e)) => Err(KillingTimeoutErr::Io(e)),
        Err(_elapsed) => {
            // try_join future is dropped → child borrow is gone → kill is reachable.
            let _ = child.start_kill();
            let _ = child.wait().await;
            Err(KillingTimeoutErr::Elapsed)
        }
    }
}
