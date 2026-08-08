//! Error and exit classification for the stdio-child core.
//!
//! No `thiserror` here: `meclaw-cells` does not depend on it, and the
//! established shape in this crate is a plain enum plus a `detail()` string
//! (see `mcp::wire::McpError` and `params_overlay`). `detail()` is what ends
//! up in a cell's error message, so it stays short and cause-carrying.

/// How a child process's life ended, as observed by the core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildExit {
    /// The process exited with this status code.
    Code(i32),
    /// The process was terminated by a signal (no status code available).
    Signal,
    /// Stdout hit EOF; the process may still be winding down.
    Eof,
    /// `wait()` failed — the child's fate could not be observed.
    SpawnLost,
}

impl ChildExit {
    /// Short human-readable form, embedded in cell error messages.
    pub fn detail(&self) -> String {
        match self {
            ChildExit::Code(c) => format!("exit code {c}"),
            ChildExit::Signal => "killed by signal".to_string(),
            ChildExit::Eof => "stdout closed (eof)".to_string(),
            ChildExit::SpawnLost => "fate unobservable".to_string(),
        }
    }
}

/// Failure modes of the stdio-child core.
#[derive(Debug)]
pub enum StdioChildError {
    /// The process could not be started (missing program, bad cwd, …).
    Spawn(String),
    /// Writing a frame to the child's stdin failed.
    Write(String),
    /// Reading from the child's stdout failed.
    Read(String),
    /// An A-timeout elapsed around a bounded operation.
    Timeout,
    /// The child died while a correlated request was still outstanding.
    ChildGone(ChildExit),
}

impl StdioChildError {
    /// Short human-readable form, embedded in cell error messages.
    pub fn detail(&self) -> String {
        match self {
            StdioChildError::Spawn(e) => format!("spawn failed: {e}"),
            StdioChildError::Write(e) => format!("write failed: {e}"),
            StdioChildError::Read(e) => format!("read failed: {e}"),
            StdioChildError::Timeout => "timeout".to_string(),
            StdioChildError::ChildGone(x) => format!("child gone: {}", x.detail()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_detail_names_the_class_and_carries_the_cause() {
        assert_eq!(
            StdioChildError::Spawn("no such file".into()).detail(),
            "spawn failed: no such file"
        );
        assert_eq!(
            StdioChildError::Write("broken pipe".into()).detail(),
            "write failed: broken pipe"
        );
        assert_eq!(
            StdioChildError::Read("bad utf8".into()).detail(),
            "read failed: bad utf8"
        );
        assert_eq!(StdioChildError::Timeout.detail(), "timeout");
        assert_eq!(
            StdioChildError::ChildGone(ChildExit::Code(3)).detail(),
            "child gone: exit code 3"
        );
    }

    #[test]
    fn child_exit_detail_distinguishes_the_four_ends() {
        assert_eq!(ChildExit::Code(0).detail(), "exit code 0");
        assert_eq!(ChildExit::Signal.detail(), "killed by signal");
        assert_eq!(ChildExit::Eof.detail(), "stdout closed (eof)");
        assert_eq!(ChildExit::SpawnLost.detail(), "fate unobservable");
    }
}
