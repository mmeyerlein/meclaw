//! Phase-10 slice B: the `timer` cell. A long-running double task on the 10-A
//! substrate. See `docs/cell-types.md` § `timer` (l.378-453).

pub mod cell;
pub mod db;
pub mod emit;
pub mod factory;
pub mod io;
pub mod op;
pub mod params;
pub mod schedule;

pub use factory::TimerCellFactory;
