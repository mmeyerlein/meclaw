//! `meclaw-testing`: fixtures and helpers for unit, integration, and phase-demo tests.

pub mod bootstrap_apply;
mod colony_handle;
pub mod factories;
mod message_builder;
pub mod mock_http;
pub mod mock_slack;
pub mod mocks;
mod test_root;
pub mod topologies;
pub mod wait;

pub use colony_handle::{ColonyHandle, spawn_colony_task_at};
pub use factories::EmitOnceMockCellFactory;
pub use message_builder::MessageBuilder;
pub use mocks::EmitOnceMockCell;
pub use test_root::TestRoot;
