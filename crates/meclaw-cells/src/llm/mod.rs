//! Phase-8 `llm`-Cell module — OpenAI Translate, atomic-emit, with cell.db.
//!
//! Grown incrementally over the phase-8 task series T2..T26; the shipped
//! surface is documented in `docs/cell-types.md` § `llm`.

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
