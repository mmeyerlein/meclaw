//! Phase-10 slice C: the `proxy` cell. A long-running double task on the 10-A
//! substrate. Telegram bridge via long poll (W2). See
//! `docs/cell-types.md` § `proxy` (Z.348–376) inkl.
//! § „Inbound-Fehlerpfade" (Z.374, Commit `1dc081a`).

pub mod cell;
pub mod db;
pub mod emit;
pub mod factory;
pub mod io;
pub mod params;
pub mod platform;
pub mod slack;
pub mod telegram;

pub use factory::ProxyCellFactory;
