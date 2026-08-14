//! Phase-8 `llm`-Cell module — OpenAI Translate, atomic-emit, with cell.db.
//!
//! Grows incrementally over T2..T26 per plans/archive/phase-8-llm.md.

pub mod auth;
pub mod cell;
pub mod factory;
pub(crate) mod latency;
pub(crate) mod output;
pub mod params;
pub(crate) mod seed;
pub(crate) mod state;
pub(crate) mod system_gate;
pub mod token_broker;
pub(crate) mod translate;
pub(crate) mod translate_responses;
pub mod wire;

pub use cell::LlmCell;
pub use factory::LlmCellFactory;
pub use params::{AuthMode, LlmParams, WireDialect};
