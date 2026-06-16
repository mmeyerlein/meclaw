//! Phase-10 Slice C: `proxy`-Cell. Long-Running Doppel-Task auf dem
//! 10-A-Substrat. Telegram-Bridge via Long-Poll (W2). Siehe
//! `docs/cell-types.md` § `proxy` (Z.348–376) inkl.
//! § „Inbound-Fehlerpfade" (Z.374, Commit `1dc081a`).

pub mod cell;
pub mod db;
pub mod emit;
pub mod factory;
pub mod io;
pub mod params;
pub mod telegram;

pub use factory::ProxyCellFactory;
