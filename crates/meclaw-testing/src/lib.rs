//! `meclaw-testing`: fixtures and helpers for unit, integration, and phase-demo tests.
//!
//! Internal crate — the public contract is the HTTP API and the template DSL;
//! no SemVer guarantee on Rust items. See README.md § Stability.

pub mod bootstrap_apply;
pub mod code_wire;
mod colony_handle;
pub mod factories;
mod message_builder;
pub mod mock_http;
pub mod mock_slack;
pub mod mocks;
mod test_root;
pub mod topologies;
pub mod wait;

pub use code_wire::{code_stdin, code_stdin_bytes};
pub use colony_handle::{ColonyHandle, spawn_colony_task_at};
pub use factories::EmitOnceMockCellFactory;
pub use factories::{SPAWN_REFUSAL, SpawnRefusesCellFactory};
pub use message_builder::MessageBuilder;
pub use mocks::EmitOnceMockCell;
pub use test_root::TestRoot;
