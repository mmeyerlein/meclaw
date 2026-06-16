//! Phase-10 Slice B: `timer`-Cell. Long-Running Doppel-Task auf dem
//! 10-A-Substrat. Siehe `docs/cell-types.md` § `timer` (Z.378–453).

pub mod cell;
pub mod db;
pub mod emit;
pub mod factory;
pub mod io;
pub mod op;
pub mod params;
pub mod schedule;

pub use factory::TimerCellFactory;
