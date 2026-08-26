//! Line-JSON framing: one `\n`-terminated line carries exactly one JSON value.
//!
//! The contract is deliberately forgiving on input and strict on output:
//! - written frames are `serde_json::to_string` + `\n` + flush,
//! - blank input lines are skipped (some children pad their output),
//! - a line that is not JSON becomes `Frame::Malformed` rather than an error,
//!   because harnesses like to print banners on stdout and a banner must not
//!   kill the connection.
//!
//! The pipe halves are addressed through free functions, not through
//! `&mut StdioChild`: the serve loop needs to hold a read future and a write
//! future at the same time inside one `select!`, which two methods on the same
//! struct could not provide (two mutable borrows).

use crate::stdio_child::error::StdioChildError;
use crate::stdio_child::spawn::{ChildLines, StdioChild};
use serde_json::Value as JsonValue;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::ChildStdin;

/// One line read from the child.
#[derive(Debug, Clone)]
pub enum Frame {
    /// A well-formed JSON value.
    Json(JsonValue),
    /// A line that did not parse as JSON, kept verbatim for diagnostics.
    Malformed(String),
}

/// Write one JSON frame. No timeout — the caller owns the A-timeout, so it is
/// applied exactly once even when a request wraps write and wait together.
pub(crate) async fn write_line(
    stdin: &mut ChildStdin,
    value: &JsonValue,
) -> Result<(), StdioChildError> {
    let mut line = serde_json::to_string(value)
        .map_err(|e| StdioChildError::Write(format!("serialize: {e}")))?;
    line.push('\n');
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|e| StdioChildError::Write(e.to_string()))?;
    stdin
        .flush()
        .await
        .map_err(|e| StdioChildError::Write(e.to_string()))
}

/// Read the next frame. `Ok(None)` is EOF, which is a lifecycle signal here,
/// not an error.
///
/// Built on `Lines::next_line`, which tokio documents as **cancellation
/// safe** — load-bearing, because the serve loop drops this future every time
/// the command arm of its `select!` wins. `AsyncBufReadExt::read_line` is
/// explicitly NOT cancel safe and would lose partial lines here.
pub(crate) async fn next_frame(lines: &mut ChildLines) -> Result<Option<Frame>, StdioChildError> {
    loop {
        let line = lines
            .next_line()
            .await
            .map_err(|e| StdioChildError::Read(e.to_string()))?;
        let Some(line) = line else { return Ok(None) };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        return Ok(Some(match serde_json::from_str::<JsonValue>(trimmed) {
            Ok(v) => Frame::Json(v),
            Err(_) => Frame::Malformed(trimmed.to_string()),
        }));
    }
}

impl StdioChild {
    /// Write one JSON frame to the child's stdin.
    ///
    /// Wrapped in an A-timeout (CONTRIBUTING.md rule 12): a child that never drains
    /// its stdin would otherwise block the writer once the pipe buffer fills.
    pub async fn write_frame(
        &mut self,
        value: &JsonValue,
        timeout: Duration,
    ) -> Result<(), StdioChildError> {
        tokio::time::timeout(timeout, write_line(&mut self.stdin, value))
            .await
            .map_err(|_| StdioChildError::Timeout)?
    }

    /// Write one ALREADY-SERIALIZED JSON document as a frame.
    ///
    /// No timeout of its own -- like [`write_line`], the caller owns the
    /// A-timeout. It exists so a consumer that already holds the bytes does not
    /// have to parse them into a `Value` only to serialize them again: on the
    /// `code` cell's warm path that round trip would copy every message body
    /// twice on the very hop the mode exists to shorten.
    pub async fn write_raw(&mut self, line: &str) -> Result<(), StdioChildError> {
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| StdioChildError::Write(e.to_string()))?;
        self.stdin
            .write_all(b"\n")
            .await
            .map_err(|e| StdioChildError::Write(e.to_string()))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| StdioChildError::Write(e.to_string()))
    }

    /// Read the next frame from the child's stdout.
    ///
    /// Deliberately NOT timeout-wrapped: see the plan's rule-12 classification
    /// (the bounded operation is the caller's correlated request, not this
    /// stream read — the read IS the liveness signal).
    pub async fn read_frame(&mut self) -> Result<Option<Frame>, StdioChildError> {
        next_frame(&mut self.stdout).await
    }

    /// Write one request and read frames until `matches` accepts one.
    ///
    /// The synchronous request/response shape used during a protocol
    /// handshake, before the serve loop takes over. Frames that do not match
    /// are skipped (a foreign answer, a banner); EOF before a match means the
    /// child died on us. The A-timeout (rule 12) spans write **and** wait,
    /// because that is the operation the caller is blocked on.
    ///
    /// Deviation from the plan sketch: the plan passed a `CorrelationKey` plus
    /// a correlator; a single predicate says the same thing with one argument
    /// less, and the serve loop keeps the key-based map where it is needed.
    pub async fn request_once<F>(
        &mut self,
        request: &JsonValue,
        matches: F,
        timeout: Duration,
    ) -> Result<JsonValue, StdioChildError>
    where
        F: Fn(&JsonValue) -> bool,
    {
        let exchange = async {
            write_line(&mut self.stdin, request).await?;
            loop {
                match next_frame(&mut self.stdout).await? {
                    Some(Frame::Json(v)) if matches(&v) => return Ok(v),
                    Some(_) => continue,
                    None => {
                        return Err(StdioChildError::ChildGone(
                            crate::stdio_child::error::ChildExit::Eof,
                        ));
                    }
                }
            }
        };
        match tokio::time::timeout(timeout, exchange).await {
            Ok(r) => r,
            Err(_) => Err(StdioChildError::Timeout),
        }
    }
}
