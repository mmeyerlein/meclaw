//! Phase-9 code-Cell modules.

pub mod cell;
/// The warm/resident runner's child tasks: one process each.
pub(crate) mod child;
pub mod factory;
/// The embedded Python harness and its frame contract.
pub(crate) mod harness;
pub mod params;
/// The warm/resident runner pool: broker task plus `PoolHandle`.
pub(crate) mod pool;
pub mod script_file;
pub mod wire;
pub use cell::CodeCell;
pub use factory::CodeCellFactory;
pub use params::{CodeParams, RunnerMode, Script};
