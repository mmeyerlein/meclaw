//! P7 — the shared stdio-child core: spawn a child process, speak
//! newline-delimited JSON with it, and supervise its lifetime inside the
//! long-running dual-task pattern.
//!
//! First consumer is the `mcp` cell's stdio transport; the `harness` cell type
//! and sub-colonies are the planned next ones. The core is deliberately
//! protocol-agnostic: JSON-RPC knowledge (request ids, `initialize`) lives in
//! the consumer, correlation is injected as a closure.
//!
//! The design is ratified; its consumers are documented in
//! `docs/cell-types.md` (`mcp`, `harness`, `subcolony`).

/// Error and exit classification for the child-process layer.
pub mod error;

/// Line-JSON framing on top of the child's pipes.
pub mod frame;

/// Process-group signalling for descendant reaping (unix only).
#[cfg(unix)]
pub(crate) mod reap;

/// The endless serve loop for the I/O sub-task.
pub mod serve;

/// Spawning and owning the child process (`ChildSpec`, `StdioChild`).
pub mod spawn;

pub use error::{ChildExit, StdioChildError};
pub use frame::Frame;
pub use serve::{ChildCommand, ChildEvent, CorrelationKey, ServeConfig};
pub use spawn::{ChildLines, ChildSpec, StdioChild};
