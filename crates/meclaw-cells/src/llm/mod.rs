//! Phase-8 `llm`-Cell module — OpenAI Translate, atomic-emit, with cell.db.
//!
//! Grown incrementally over the phase-8 task series T2..T26; the shipped
//! surface is documented in `docs/cell-types.md` § `llm`.

pub mod auth;
pub mod cell;
pub(crate) mod continuation;
pub mod factory;
pub(crate) mod latency;
pub(crate) mod output;
pub(crate) mod package;
pub mod params;
pub(crate) mod sanitize;
pub(crate) mod seed;
pub(crate) mod state;
pub(crate) mod system_gate;
pub mod token_broker;
pub(crate) mod tool_scope;
pub(crate) mod translate;
pub(crate) mod translate_decisions;
pub(crate) mod translate_responses;
pub(crate) mod window;
pub mod wire;

pub use cell::LlmCell;
pub use factory::LlmCellFactory;
/// GH #890: the hop keys the cell writes, read by the template sweep
/// `gh890_every_llm_cell_declares_what_output_writes`.
pub use output::HOP_KEYS;
pub use params::{AuthMode, LlmParams, WireDialect};
