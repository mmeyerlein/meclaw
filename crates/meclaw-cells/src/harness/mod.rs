//! P8 — the `harness` cell type: an agent harness (Claude Code in print mode)
//! supervised as a cell.
//!
//! Built on the P7 stdio-child core, with one structural difference from every
//! other long-running cell type: **a dead child is normal here.** For `mcp` the
//! child IS the cell's ability to answer, so its death is a panic; for a
//! harness the child is one task, and its exit is how a task ends. The I/O
//! sub-task therefore cycles — idle, spawn, stream, idle — instead of parking.
//!
//! The other load-bearing difference is non-idempotency. Harness tasks mutate
//! repositories, so a supervisor restart must never re-fire the last one. The
//! task table in `cell.db` is a tombstone register, not a work queue: a row is
//! written BEFORE the child is spawned, and a restart turns every unfinished
//! row into "unknown outcome, inspect the workspace" — never into a new run.
//!
//! See `plans/p8-harness-cell.md` for the ratified design.

/// Which vendor dialect the child speaks.
pub mod adapter;

/// The task tombstone register in `cell.db`.
pub mod db;

/// The Claude Code dialect adapter.
pub mod claude_code;

/// Birth configuration (`params`).
pub mod params;

/// The cell factory.
pub mod factory;

/// The handler side of the cell.
pub mod cell;

/// The five emission shapes.
pub mod emit;

/// Reading the tool call out of a message.
pub mod parse;

/// The I/O sub-task: one child per task.
pub mod io;

/// What a message can ask of this cell.
pub mod task;

pub use adapter::HarnessAdapter;
pub use cell::HarnessCell;
pub use factory::HarnessCellFactory;
pub use params::{ApprovalMode, HarnessOverlay, HarnessParams};
pub use task::{AnswerRequest, HarnessCommand, TaskRequest};
