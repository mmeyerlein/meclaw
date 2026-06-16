//! Phase-8 `llm`-Cell module — OpenAI Translate, atomic-emit, with cell.db.
//!
//! Grows incrementally over T2..T26 per plans/archive/phase-8-llm.md.

pub mod cell;
pub mod factory;
pub(crate) mod output;
pub mod params;
pub(crate) mod state;
pub(crate) mod translate;
pub mod wire;

pub use cell::LlmCell;
pub use factory::LlmCellFactory;
pub use params::LlmParams;
